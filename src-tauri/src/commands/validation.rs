//! W16 Proposal Validation Gate.
//!
//! `validate_build_proposal` runs deterministic structural checks against
//! a freshly parsed `BuildProposal` before the UI receives a preview. The
//! agent loop re-injects unresolved issues as a synthetic
//! `validate_proposal` tool result so the model can self-correct on its
//! retry turn.
//!
//! Heuristics here intentionally err on the side of false positives for
//! `HardcodedLiteralValue` — a noisy retry is cheap; a quietly broken
//! widget that ships to the dashboard is expensive.

use std::collections::{BTreeMap, HashSet};

use serde_json::Value;

use crate::models::chat::ChatMessage;
use crate::models::dashboard::{
    BuildDatasourcePlan, BuildDatasourcePlanKind, BuildProposal, BuildWidgetProposal,
    BuildWidgetType, Dashboard,
};
use crate::models::pipeline::PipelineStep;
use crate::models::validation::{UnusedSourceMentionEntry, ValidationIssue};
use crate::modules::parameter_engine::{detect_cycle, ResolvedParameters};

/// W48: minimum identity a resolved source mention needs to participate
/// in proposal validation. The chat layer materialises this from
/// [`crate::models::chat::SourceMention`] by looking up the actual
/// `DatasourceDefinition` / `Workflow` row before invoking validation.
#[derive(Debug, Clone)]
pub struct MentionedSource {
    pub label: String,
    pub datasource_definition_id: Option<String>,
    pub workflow_id: Option<String>,
}

/// W33: lift `parse_build_proposal` out of `commands::chat` so the eval
/// suite can exercise the exact same prose-extraction path the agent
/// loop uses. Direct JSON deserialisation is tried first; falling
/// through to the first/last `{}` extraction matches the historical
/// behavior of providers that wrap proposals in markdown prose.
pub fn parse_build_proposal_content(content: &str) -> Option<BuildProposal> {
    if let Ok(direct) = serde_json::from_str::<BuildProposal>(content) {
        return Some(direct);
    }
    let start = content.find('{')?;
    let end = content.rfind('}')?;
    if end <= start {
        return None;
    }
    let snippet = &content[start..=end];
    let value = serde_json::from_str::<Value>(snippet).ok()?;
    serde_json::from_value(value).ok()
}

/// Issue-kinds that require a successful `dry_run_widget` tool call in
/// the same chat session before the final proposal. Tables only require
/// dry-run when they aggregate; we conservatively require it for every
/// table because we cannot see the pipeline kinds from outside.
fn requires_dry_run(kind: &BuildWidgetType) -> bool {
    matches!(
        kind,
        BuildWidgetType::Stat
            | BuildWidgetType::Gauge
            | BuildWidgetType::BarGauge
            | BuildWidgetType::StatusGrid
            | BuildWidgetType::Table
            | BuildWidgetType::Gallery
    )
}

fn widget_kind_label(kind: &BuildWidgetType) -> &'static str {
    match kind {
        BuildWidgetType::Chart => "chart",
        BuildWidgetType::Text => "text",
        BuildWidgetType::Table => "table",
        BuildWidgetType::Image => "image",
        BuildWidgetType::Gauge => "gauge",
        BuildWidgetType::Stat => "stat",
        BuildWidgetType::Logs => "logs",
        BuildWidgetType::BarGauge => "bar_gauge",
        BuildWidgetType::StatusGrid => "status_grid",
        BuildWidgetType::Heatmap => "heatmap",
        BuildWidgetType::Gallery => "gallery",
    }
}

/// Run the full proposal validation gate. The dashboard reference is the
/// dashboard the proposal will be applied against; `None` means the
/// proposal targets a new dashboard, so `replace_widget_id` checks
/// degrade to "any string is unknown".
///
/// W38: `target_widget_ids`, when `Some`, restricts edits to those
/// widget ids. Proposals that replace or remove widget ids outside the
/// target set produce `OffTargetWidgetReplace` / `OffTargetWidgetRemove`
/// issues. `None` (or an empty slice) disables the targeted-edit gate
/// — the agent may touch any widget on the dashboard.
pub fn validate_build_proposal(
    proposal: &BuildProposal,
    dashboard: Option<&Dashboard>,
    transcript: &[ChatMessage],
    target_widget_ids: Option<&[String]>,
) -> Vec<ValidationIssue> {
    validate_build_proposal_full(proposal, dashboard, transcript, target_widget_ids, None)
}

/// W48: extended entry point. Same checks as
/// [`validate_build_proposal`] plus mention-coverage enforcement: every
/// entry in `mentioned_sources` must be referenced by at least one
/// widget in the proposal (directly via `datasource_definition_id` /
/// `workflow_id`, via a shared key matching the source workflow, or as
/// a compose input).
pub fn validate_build_proposal_full(
    proposal: &BuildProposal,
    dashboard: Option<&Dashboard>,
    transcript: &[ChatMessage],
    target_widget_ids: Option<&[String]>,
    mentioned_sources: Option<&[MentionedSource]>,
) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();

    // Shared key uniqueness.
    let mut seen_keys: HashSet<&str> = HashSet::new();
    for shared in &proposal.shared_datasources {
        if !seen_keys.insert(shared.key.as_str()) {
            issues.push(ValidationIssue::DuplicateSharedKey {
                key: shared.key.clone(),
            });
        }
        // W39: shared HTTP sources go through the same safety gate as
        // widget-inline ones so a future fan-out cannot smuggle an unsafe
        // URL in via the shared key alone.
        if matches!(shared.kind, BuildDatasourcePlanKind::BuiltinTool)
            && shared.tool_name.as_deref() == Some("http_request")
        {
            if let Some(args) = shared.arguments.as_ref() {
                if let Err(e) = crate::modules::tool_engine::validate_http_request_arguments(args) {
                    issues.push(ValidationIssue::UnsafeHttpDatasource {
                        widget_index: 0,
                        widget_title: shared.key.clone(),
                        source_kind: "shared".to_string(),
                        reason: e.to_string(),
                    });
                }
            } else {
                issues.push(ValidationIssue::UnsafeHttpDatasource {
                    widget_index: 0,
                    widget_title: shared.key.clone(),
                    source_kind: "shared".to_string(),
                    reason: "http_request shared source is missing the arguments object"
                        .to_string(),
                });
            }
        }
    }
    let declared_shared_keys: HashSet<&str> = proposal
        .shared_datasources
        .iter()
        .map(|s| s.key.as_str())
        .collect();

    // Dashboard widget ids for replace_widget_id checks.
    let existing_widget_ids: HashSet<String> = dashboard
        .map(|d| d.layout.iter().map(|w| w.id().to_string()).collect())
        .unwrap_or_default();

    // Bucket dry_run_widget tool calls by widget title (the only stable
    // identifier we have before apply).
    let dry_run_titles = collect_dry_run_titles(transcript);

    // W25: union of parameter names declared on the proposal + already on
    // the dashboard. Widgets can reference either set.
    let mut declared_params: HashSet<String> =
        proposal.parameters.iter().map(|p| p.name.clone()).collect();
    if let Some(d) = dashboard {
        for p in &d.parameters {
            declared_params.insert(p.name.clone());
        }
    }

    // W25: cycle check across the union of dashboard + proposal parameters.
    let mut merged_params: Vec<_> = dashboard.map(|d| d.parameters.clone()).unwrap_or_default();
    for p in &proposal.parameters {
        if let Some(existing) = merged_params.iter_mut().find(|q| q.id == p.id) {
            *existing = p.clone();
        } else {
            merged_params.push(p.clone());
        }
    }
    if let Some(cycle) = detect_cycle(&merged_params) {
        issues.push(ValidationIssue::ParameterCycle { cycle });
    }

    // W38: build the target set as a fast lookup. `None`/empty == no
    // mention scope, so the off-target gates degrade to no-ops.
    let target_set: Option<HashSet<&str>> = target_widget_ids
        .filter(|ids| !ids.is_empty())
        .map(|ids| ids.iter().map(|id| id.as_str()).collect());

    for (index, widget) in proposal.widgets.iter().enumerate() {
        let widget_index = index as u32;
        let widget_title = widget.title.clone();

        validate_widget_datasource(
            widget_index,
            &widget_title,
            widget,
            &declared_shared_keys,
            &mut issues,
        );

        validate_widget_parameter_refs(
            widget_index,
            &widget_title,
            widget,
            &declared_params,
            &mut issues,
        );

        if let Some(replace_id) = widget.replace_widget_id.as_ref() {
            if !existing_widget_ids.contains(replace_id) {
                issues.push(ValidationIssue::UnknownReplaceWidgetId {
                    widget_index,
                    widget_title: widget_title.clone(),
                    replace_widget_id: replace_id.clone(),
                });
            }
            if let Some(targets) = target_set.as_ref() {
                if !targets.contains(replace_id.as_str()) {
                    issues.push(ValidationIssue::OffTargetWidgetReplace {
                        widget_index,
                        widget_title: widget_title.clone(),
                        replace_widget_id: replace_id.clone(),
                    });
                }
            }
        }

        validate_widget_pipeline(widget_index, &widget_title, widget, &mut issues);
        validate_text_widget_markdown(widget_index, &widget_title, widget, &mut issues);
        validate_no_hardcoded_value(widget_index, &widget_title, widget, &mut issues);
        validate_no_hardcoded_gallery_items(widget_index, &widget_title, widget, &mut issues);
        validate_layout_fields(widget_index, &widget_title, widget, &mut issues);

        if requires_dry_run(&widget.widget_type) {
            let normalized = normalize_dry_run_title(&widget_title);
            if !dry_run_titles.contains(widget_title.trim())
                && !dry_run_titles.contains(widget_title.trim().to_lowercase().as_str())
                && !dry_run_titles.contains(normalized.as_str())
            {
                issues.push(ValidationIssue::MissingDryRunEvidence {
                    widget_index,
                    widget_title: widget_title.clone(),
                    widget_kind: widget_kind_label(&widget.widget_type).to_string(),
                });
            }
        }
    }

    // W38: removal must also respect the mention scope when one is set.
    if let Some(targets) = target_set.as_ref() {
        for remove_id in &proposal.remove_widget_ids {
            if !targets.contains(remove_id.as_str()) {
                issues.push(ValidationIssue::OffTargetWidgetRemove {
                    remove_widget_id: remove_id.clone(),
                });
            }
        }
    }

    // W48: every mentioned source must be referenced by at least one
    // widget. Sharing the dashboard with the agent is not enough — the
    // user explicitly named these sources and the resulting widget must
    // use them or the agent has misunderstood the request.
    if let Some(sources) = mentioned_sources.filter(|m| !m.is_empty()) {
        let referenced = collect_referenced_sources(proposal);
        let mut missing = Vec::new();
        for source in sources {
            if !mention_is_referenced(source, &referenced) {
                missing.push(UnusedSourceMentionEntry {
                    label: source.label.clone(),
                    datasource_definition_id: source.datasource_definition_id.clone(),
                    workflow_id: source.workflow_id.clone(),
                });
            }
        }
        if !missing.is_empty() {
            issues.push(ValidationIssue::UnusedSourceMention { missing });
        }
    }

    issues
}

#[derive(Debug, Default)]
struct ReferencedSources {
    definition_ids: HashSet<String>,
    workflow_ids: HashSet<String>,
    shared_keys: HashSet<String>,
}

fn collect_referenced_sources(proposal: &BuildProposal) -> ReferencedSources {
    let mut refs = ReferencedSources::default();
    for shared in &proposal.shared_datasources {
        refs.shared_keys.insert(shared.key.clone());
    }
    for widget in &proposal.widgets {
        if let Some(plan) = widget.datasource_plan.as_ref() {
            collect_plan_sources(plan, &mut refs);
        }
    }
    refs
}

fn collect_plan_sources(plan: &BuildDatasourcePlan, refs: &mut ReferencedSources) {
    if let Some(id) = plan.source_key.as_ref() {
        refs.shared_keys.insert(id.clone());
    }
    if let Some(inputs) = plan.inputs.as_ref() {
        for inner in inputs.values() {
            collect_plan_sources(inner, refs);
        }
    }
    // Arguments may carry an inline binding hint produced by the apply
    // path (see `chat::resolve_source_mentions`), but the agent itself
    // also routinely emits raw `datasource_definition_id` / `workflow_id`
    // fields inside arguments for tool-driven plans. Walk the arguments
    // JSON looking for either, so the validator does not falsely flag
    // valid reuse.
    if let Some(args) = plan.arguments.as_ref() {
        walk_json_for_ids(args, refs);
    }
}

fn walk_json_for_ids(value: &Value, refs: &mut ReferencedSources) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                if matches!(key.as_str(), "datasource_definition_id") {
                    if let Some(id) = inner.as_str() {
                        refs.definition_ids.insert(id.to_string());
                    }
                }
                if matches!(key.as_str(), "workflow_id") {
                    if let Some(id) = inner.as_str() {
                        refs.workflow_ids.insert(id.to_string());
                    }
                }
                walk_json_for_ids(inner, refs);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk_json_for_ids(item, refs);
            }
        }
        _ => {}
    }
}

fn mention_is_referenced(source: &MentionedSource, refs: &ReferencedSources) -> bool {
    if let Some(def_id) = source.datasource_definition_id.as_deref() {
        if refs.definition_ids.contains(def_id) {
            return true;
        }
    }
    if let Some(wf_id) = source.workflow_id.as_deref() {
        if refs.workflow_ids.contains(wf_id) {
            return true;
        }
    }
    false
}

fn validate_widget_datasource(
    widget_index: u32,
    widget_title: &str,
    widget: &BuildWidgetProposal,
    declared_shared_keys: &HashSet<&str>,
    issues: &mut Vec<ValidationIssue>,
) {
    let plan = match widget.datasource_plan.as_ref() {
        Some(plan) => plan,
        None => {
            issues.push(ValidationIssue::MissingDatasourcePlan {
                widget_index,
                widget_title: widget_title.to_string(),
            });
            return;
        }
    };

    if matches!(plan.kind, BuildDatasourcePlanKind::Shared) {
        match plan.source_key.as_ref() {
            Some(key) if !declared_shared_keys.contains(key.as_str()) => {
                issues.push(ValidationIssue::UnknownSourceKey {
                    widget_index,
                    widget_title: widget_title.to_string(),
                    source_key: key.clone(),
                });
            }
            None => {
                issues.push(ValidationIssue::UnknownSourceKey {
                    widget_index,
                    widget_title: widget_title.to_string(),
                    source_key: String::new(),
                });
            }
            _ => {}
        }
    }
    // W39: inline http_request datasources go through the safety gate too,
    // so the agent gets typed feedback on retry rather than discovering
    // the problem at apply time.
    if matches!(plan.kind, BuildDatasourcePlanKind::BuiltinTool)
        && plan.tool_name.as_deref() == Some("http_request")
    {
        if let Some(args) = plan.arguments.as_ref() {
            if let Err(e) = crate::modules::tool_engine::validate_http_request_arguments(args) {
                issues.push(ValidationIssue::UnsafeHttpDatasource {
                    widget_index,
                    widget_title: widget_title.to_string(),
                    source_kind: "widget".to_string(),
                    reason: e.to_string(),
                });
            }
        } else {
            issues.push(ValidationIssue::UnsafeHttpDatasource {
                widget_index,
                widget_title: widget_title.to_string(),
                source_kind: "widget".to_string(),
                reason: "http_request datasource is missing the arguments object".to_string(),
            });
        }
    }
    if matches!(plan.kind, BuildDatasourcePlanKind::Compose) {
        // Compose plans must declare at least one input; nested compose is
        // illegal. Inner `kind: shared` references must point at declared
        // shared_datasources entries.
        let inputs_empty = plan.inputs.as_ref().map(|m| m.is_empty()).unwrap_or(true);
        if inputs_empty {
            issues.push(ValidationIssue::MissingDatasourcePlan {
                widget_index,
                widget_title: widget_title.to_string(),
            });
            return;
        }
        if let Some(inputs) = &plan.inputs {
            for (key, inner) in inputs.iter() {
                if matches!(inner.kind, BuildDatasourcePlanKind::Compose) {
                    issues.push(ValidationIssue::PipelineSchemaInvalid {
                        widget_index,
                        widget_title: widget_title.to_string(),
                        error: format!(
                            "compose input '{}' uses kind='compose'; nested compose is not supported",
                            key
                        ),
                    });
                }
                if matches!(inner.kind, BuildDatasourcePlanKind::Shared) {
                    match inner.source_key.as_ref() {
                        Some(sk) if !declared_shared_keys.contains(sk.as_str()) => {
                            issues.push(ValidationIssue::UnknownSourceKey {
                                widget_index,
                                widget_title: widget_title.to_string(),
                                source_key: sk.clone(),
                            });
                        }
                        None => {
                            issues.push(ValidationIssue::UnknownSourceKey {
                                widget_index,
                                widget_title: widget_title.to_string(),
                                source_key: String::new(),
                            });
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

fn validate_widget_pipeline(
    widget_index: u32,
    widget_title: &str,
    widget: &BuildWidgetProposal,
    issues: &mut Vec<ValidationIssue>,
) {
    let pipeline = match widget.datasource_plan.as_ref() {
        Some(plan) => &plan.pipeline,
        None => return,
    };
    if pipeline.is_empty() {
        return;
    }
    // PipelineStep already deserialised to the typed enum, so re-serialise
    // + parse round-trip catches anything `serde_json::from_value` left in
    // an inconsistent state (e.g., a custom step that lost its variant tag).
    if let Err(error) = serde_json::to_value(pipeline)
        .and_then(|value| serde_json::from_value::<Vec<PipelineStep>>(value))
    {
        issues.push(ValidationIssue::PipelineSchemaInvalid {
            widget_index,
            widget_title: widget_title.to_string(),
            error: error.to_string(),
        });
    }
}

/// W25: every `$param` token referenced in a widget's datasource_plan
/// arguments or pipeline config must resolve to a declared parameter.
fn validate_widget_parameter_refs(
    widget_index: u32,
    widget_title: &str,
    widget: &BuildWidgetProposal,
    declared_params: &HashSet<String>,
    issues: &mut Vec<ValidationIssue>,
) {
    let Some(plan) = widget.datasource_plan.as_ref() else {
        return;
    };
    let mut referenced = std::collections::BTreeSet::new();
    collect_plan_parameter_refs(plan, &mut referenced);
    for name in referenced {
        if !declared_params.contains(&name) {
            issues.push(ValidationIssue::UnknownParameterReference {
                widget_index,
                widget_title: widget_title.to_string(),
                param_name: name,
            });
        }
    }
}

fn collect_plan_parameter_refs(
    plan: &crate::models::dashboard::BuildDatasourcePlan,
    referenced: &mut std::collections::BTreeSet<String>,
) {
    if let Some(args) = &plan.arguments {
        referenced.extend(ResolvedParameters::referenced_names(args));
    }
    if !plan.pipeline.is_empty() {
        if let Ok(pipeline_json) = serde_json::to_value(&plan.pipeline) {
            referenced.extend(ResolvedParameters::referenced_names(&pipeline_json));
        }
    }
    if let Some(prompt) = &plan.prompt {
        referenced.extend(ResolvedParameters::referenced_names(&Value::String(
            prompt.clone(),
        )));
    }
    if let Some(inputs) = &plan.inputs {
        for inner in inputs.values() {
            collect_plan_parameter_refs(inner, referenced);
        }
    }
}

fn validate_text_widget_markdown(
    widget_index: u32,
    widget_title: &str,
    widget: &BuildWidgetProposal,
    issues: &mut Vec<ValidationIssue>,
) {
    if !matches!(widget.widget_type, BuildWidgetType::Text) {
        return;
    }
    let text_candidates = collect_text_strings(widget);
    for candidate in text_candidates {
        let trimmed = candidate.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
            continue;
        }
        if serde_json::from_str::<Value>(trimmed).is_ok() {
            issues.push(ValidationIssue::TextWidgetContainsRawJson {
                widget_index,
                widget_title: widget_title.to_string(),
            });
            return;
        }
    }
}

fn collect_text_strings(widget: &BuildWidgetProposal) -> Vec<String> {
    let mut out = Vec::new();
    if let Value::String(text) = &widget.data {
        out.push(text.clone());
    }
    if let Some(Value::Object(map)) = widget.config.as_ref().map(|v| v as &Value) {
        for key in ["content", "markdown", "text", "body"] {
            if let Some(Value::String(text)) = map.get(key) {
                out.push(text.clone());
            }
        }
    }
    out
}

/// W44: a gallery widget that bakes an array of image items into `data`
/// AND has no pipeline producing items will silently render the
/// hardcoded preview on every refresh. Reject so the agent re-proposes
/// with an actual datasource-driven pipeline.
fn validate_no_hardcoded_gallery_items(
    widget_index: u32,
    widget_title: &str,
    widget: &BuildWidgetProposal,
    issues: &mut Vec<ValidationIssue>,
) {
    if !matches!(widget.widget_type, BuildWidgetType::Gallery) {
        return;
    }
    let items_array = match &widget.data {
        Value::Array(arr) => Some(arr),
        Value::Object(obj) => obj
            .get("items")
            .or_else(|| obj.get("images"))
            .and_then(Value::as_array),
        _ => None,
    };
    let Some(items) = items_array else {
        return;
    };
    let looks_like_image = items.iter().any(|item| {
        if let Value::String(s) = item {
            !s.trim().is_empty()
        } else if let Some(obj) = item.as_object() {
            ["src", "url", "image", "path", "thumbnail"]
                .iter()
                .any(|k| {
                    obj.get(*k)
                        .and_then(Value::as_str)
                        .is_some_and(|s| !s.trim().is_empty())
                })
        } else {
            false
        }
    });
    if !looks_like_image {
        return;
    }
    let pipeline_produces_items = widget.datasource_plan.as_ref().is_some_and(|plan| {
        !plan.pipeline.is_empty()
            || plan.source_key.is_some()
            || plan.inputs.as_ref().is_some_and(|m| !m.is_empty())
            || plan.output_path.is_some()
    });
    if pipeline_produces_items {
        return;
    }
    issues.push(ValidationIssue::HardcodedGalleryItems {
        widget_index,
        widget_title: widget_title.to_string(),
        item_count: items.len() as u32,
    });
}

/// W45: surface layout-field mistakes before apply. Auto-pack is the
/// only placement model for new widgets, so explicit `x`/`y` is always
/// wrong; mixing `size_preset` and raw `w`/`h` is ambiguous, so we
/// force the agent to pick one. Existing widgets being replaced via
/// `replace_widget_id` are skipped — their position is inherited from
/// the slot they overwrite, not from the proposal.
fn validate_layout_fields(
    widget_index: u32,
    widget_title: &str,
    widget: &BuildWidgetProposal,
    issues: &mut Vec<ValidationIssue>,
) {
    if widget.replace_widget_id.is_some() {
        return;
    }
    if widget.x.is_some() || widget.y.is_some() {
        issues.push(ValidationIssue::ProposedExplicitCoordinates {
            widget_index,
            widget_title: widget_title.to_string(),
        });
    }
    if widget.size_preset.is_some() && (widget.w.is_some() || widget.h.is_some()) {
        issues.push(ValidationIssue::ConflictingLayoutFields {
            widget_index,
            widget_title: widget_title.to_string(),
        });
    }
}

fn validate_no_hardcoded_value(
    widget_index: u32,
    widget_title: &str,
    widget: &BuildWidgetProposal,
    issues: &mut Vec<ValidationIssue>,
) {
    if !matches!(
        widget.widget_type,
        BuildWidgetType::Stat | BuildWidgetType::Gauge | BuildWidgetType::BarGauge
    ) {
        return;
    }
    let Some(Value::Object(config)) = widget.config.as_ref().map(|v| v as &Value) else {
        return;
    };
    // `value` baked in the config without a pipeline producing it = hard
    // failure. A literal data field on the widget is OK as long as a
    // pipeline exists to overwrite it on every refresh.
    if let Some(Value::Number(_)) = config.get("value") {
        let has_pipeline = widget.datasource_plan.as_ref().is_some_and(|plan| {
            !plan.pipeline.is_empty()
                || plan.source_key.is_some()
                || plan.inputs.as_ref().is_some_and(|m| !m.is_empty())
        });
        if !has_pipeline {
            issues.push(ValidationIssue::HardcodedLiteralValue {
                widget_index,
                widget_title: widget_title.to_string(),
                path: "config.value".to_string(),
            });
        }
    }
    if let Some(Value::Number(_)) = config.get("min") {
        // min/max are config knobs — fine.
    }
}

fn collect_dry_run_titles(transcript: &[ChatMessage]) -> HashSet<String> {
    let mut titles = HashSet::new();
    let mut call_id_to_titles: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for message in transcript {
        if let Some(calls) = &message.tool_calls {
            for call in calls {
                if call.name != "dry_run_widget" {
                    continue;
                }
                let claimed = extract_dry_run_widget_titles(&call.arguments);
                if !claimed.is_empty() {
                    call_id_to_titles.insert(call.id.clone(), claimed);
                }
            }
        }
        if let Some(results) = &message.tool_results {
            for result in results {
                if result.error.is_some() {
                    continue;
                }
                if !is_dry_run_result_ok(&result.result) {
                    continue;
                }
                if let Some(claimed) = call_id_to_titles.get(&result.tool_call_id) {
                    for title in claimed {
                        titles.insert(title.clone());
                        titles.insert(title.to_lowercase());
                        // Punctuation-insensitive form so a widget titled
                        // "Tokyo · Temperature" matches a dry-run titled
                        // "Tokyo temperature".
                        titles.insert(normalize_dry_run_title(title));
                    }
                }
            }
        }
    }

    titles
}

/// Lowercase + strip everything that's not alphanumeric. Lets dry-run
/// titles cover near-identical widget titles that differ only in
/// separators ("·" vs space), spacing, or punctuation.
fn normalize_dry_run_title(title: &str) -> String {
    title
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn extract_dry_run_widget_titles(arguments: &Value) -> Vec<String> {
    let mut out = Vec::new();
    // The agent passes the widget proposal either as `arguments.proposal`
    // or as `arguments.widget` (legacy) or flattened at the top level.
    let proposal = arguments
        .get("proposal")
        .or_else(|| arguments.get("widget"))
        .unwrap_or(arguments);
    if let Some(title) = proposal.get("title").and_then(|v| v.as_str()) {
        let trimmed = title.trim();
        if !trimmed.is_empty() {
            out.push(trimmed.to_string());
        }
    }
    // `titles_covered` lets a single dry-run stand in for several
    // near-identical widget titles that share the same pipeline shape.
    if let Some(extra) = arguments.get("titles_covered").and_then(|v| v.as_array()) {
        for entry in extra {
            if let Some(s) = entry.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    out.push(trimmed.to_string());
                }
            }
        }
    }
    out
}

fn is_dry_run_result_ok(result: &Value) -> bool {
    // dry_run_widget returns either a populated payload (ok) or
    // `{ "status": "error" }` when execute_chat_tool wrapped a failure.
    if result
        .get("status")
        .and_then(|v| v.as_str())
        .is_some_and(|status| status == "error")
    {
        return false;
    }
    // Treat any other object/array/string with content as a success.
    !matches!(result, Value::Null)
}

/// Render a Markdown bullet list of issues for the synthetic retry
/// feedback message handed back to the agent.
pub fn format_issues_for_agent(issues: &[ValidationIssue]) -> String {
    if issues.is_empty() {
        return "(no issues)".to_string();
    }
    let mut out = String::new();
    for issue in issues {
        out.push_str("- ");
        out.push_str(&issue.summary());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::dashboard::{
        BuildDatasourcePlan, BuildDatasourcePlanKind, BuildProposal, BuildWidgetProposal,
        BuildWidgetType, DashboardParameter, DashboardParameterKind, ParameterValue,
    };
    use serde_json::json;

    fn proposal_with_widget(
        plan: BuildDatasourcePlan,
        params: Vec<DashboardParameter>,
    ) -> BuildProposal {
        BuildProposal {
            id: "p1".into(),
            title: "T".into(),
            summary: None,
            dashboard_name: None,
            dashboard_description: None,
            widgets: vec![BuildWidgetProposal {
                widget_type: BuildWidgetType::Stat,
                title: "Active count".into(),
                data: json!({"value": 0}),
                datasource_plan: Some(plan),
                config: None,
                x: None,
                y: None,
                w: None,
                h: None,
                replace_widget_id: None,
                size_preset: None,
                layout_pattern: None,
            }],
            remove_widget_ids: Vec::new(),
            shared_datasources: Vec::new(),
            parameters: params,
        }
    }

    fn param(name: &str, kind: DashboardParameterKind) -> DashboardParameter {
        DashboardParameter {
            id: name.into(),
            name: name.into(),
            label: name.into(),
            kind,
            multi: false,
            include_all: false,
            default: None,
            depends_on: Vec::new(),
            description: None,
        }
    }

    #[test]
    fn unknown_parameter_reference_flagged() {
        let plan = BuildDatasourcePlan {
            kind: BuildDatasourcePlanKind::McpTool,
            tool_name: Some("test_tool".into()),
            server_id: Some("test_server".into()),
            arguments: Some(json!({"project": "$project"})),
            prompt: None,
            output_path: None,
            refresh_cron: None,
            pipeline: Vec::new(),
            source_key: None,
            inputs: None,
        };
        let proposal = proposal_with_widget(plan, Vec::new());
        let issues = validate_build_proposal(&proposal, None, &[], None);
        assert!(issues
            .iter()
            .any(|i| matches!(i, ValidationIssue::UnknownParameterReference { param_name, .. } if param_name == "project")));
    }

    #[test]
    fn declared_parameter_resolves_without_issue() {
        let plan = BuildDatasourcePlan {
            kind: BuildDatasourcePlanKind::McpTool,
            tool_name: Some("test_tool".into()),
            server_id: Some("test_server".into()),
            arguments: Some(json!({"project": "$project"})),
            prompt: None,
            output_path: None,
            refresh_cron: None,
            pipeline: Vec::new(),
            source_key: None,
            inputs: None,
        };
        let project = param(
            "project",
            DashboardParameterKind::StaticList {
                options: Vec::new(),
            },
        );
        let proposal = proposal_with_widget(plan, vec![project]);
        let issues = validate_build_proposal(&proposal, None, &[], None);
        assert!(!issues
            .iter()
            .any(|i| matches!(i, ValidationIssue::UnknownParameterReference { .. })));
    }

    #[test]
    fn parameter_cycle_flagged() {
        let mut a = param(
            "env",
            DashboardParameterKind::Constant {
                value: ParameterValue::String("prod".into()),
            },
        );
        a.depends_on = vec!["service".into()];
        let mut b = param(
            "service",
            DashboardParameterKind::Constant {
                value: ParameterValue::String("api".into()),
            },
        );
        b.depends_on = vec!["env".into()];
        let plan = BuildDatasourcePlan {
            kind: BuildDatasourcePlanKind::McpTool,
            tool_name: Some("t".into()),
            server_id: Some("s".into()),
            arguments: None,
            prompt: None,
            output_path: None,
            refresh_cron: None,
            pipeline: Vec::new(),
            source_key: None,
            inputs: None,
        };
        let proposal = proposal_with_widget(plan, vec![a, b]);
        let issues = validate_build_proposal(&proposal, None, &[], None);
        assert!(issues
            .iter()
            .any(|i| matches!(i, ValidationIssue::ParameterCycle { .. })));
    }

    fn targeted_proposal(replace_id: &str, remove_ids: Vec<String>) -> BuildProposal {
        let plan = BuildDatasourcePlan {
            kind: BuildDatasourcePlanKind::McpTool,
            tool_name: Some("t".into()),
            server_id: Some("s".into()),
            arguments: None,
            prompt: None,
            output_path: None,
            refresh_cron: None,
            pipeline: Vec::new(),
            source_key: None,
            inputs: None,
        };
        let mut proposal = proposal_with_widget(plan, Vec::new());
        proposal.widgets[0].replace_widget_id = Some(replace_id.to_string());
        proposal.remove_widget_ids = remove_ids;
        proposal
    }

    #[test]
    fn off_target_replace_flagged_when_widget_not_mentioned() {
        let proposal = targeted_proposal("w_unrelated", Vec::new());
        let targets = vec!["w_chosen".to_string()];
        let issues = validate_build_proposal(&proposal, None, &[], Some(&targets));
        assert!(issues.iter().any(|i| matches!(
            i,
            ValidationIssue::OffTargetWidgetReplace { replace_widget_id, .. }
                if replace_widget_id == "w_unrelated"
        )));
    }

    #[test]
    fn replace_passes_when_widget_is_mentioned() {
        let proposal = targeted_proposal("w_chosen", Vec::new());
        let targets = vec!["w_chosen".to_string()];
        let issues = validate_build_proposal(&proposal, None, &[], Some(&targets));
        assert!(!issues
            .iter()
            .any(|i| matches!(i, ValidationIssue::OffTargetWidgetReplace { .. })));
    }

    #[test]
    fn off_target_remove_flagged_when_id_not_mentioned() {
        let proposal = targeted_proposal("w_chosen", vec!["w_other".to_string()]);
        let targets = vec!["w_chosen".to_string()];
        let issues = validate_build_proposal(&proposal, None, &[], Some(&targets));
        assert!(issues.iter().any(|i| matches!(
            i,
            ValidationIssue::OffTargetWidgetRemove { remove_widget_id }
                if remove_widget_id == "w_other"
        )));
    }

    #[test]
    fn gallery_with_hardcoded_data_array_is_flagged() {
        // No pipeline + literal `data: [{src:...}]` is the hardcode pattern.
        let plan = BuildDatasourcePlan {
            kind: BuildDatasourcePlanKind::BuiltinTool,
            tool_name: Some("http_request".into()),
            server_id: None,
            arguments: Some(json!({"method": "GET", "url": "https://example.test/x"})),
            prompt: None,
            output_path: None,
            refresh_cron: None,
            pipeline: Vec::new(),
            source_key: None,
            inputs: None,
        };
        let proposal = BuildProposal {
            id: "p2".into(),
            title: "T".into(),
            summary: None,
            dashboard_name: None,
            dashboard_description: None,
            widgets: vec![BuildWidgetProposal {
                widget_type: BuildWidgetType::Gallery,
                title: "Cats".into(),
                data: json!([
                    {"src": "https://a/1.jpg"},
                    {"src": "https://a/2.jpg"},
                ]),
                datasource_plan: Some(plan),
                config: None,
                x: None,
                y: None,
                w: None,
                h: None,
                replace_widget_id: None,
                size_preset: None,
                layout_pattern: None,
            }],
            remove_widget_ids: Vec::new(),
            shared_datasources: Vec::new(),
            parameters: Vec::new(),
        };
        let issues = validate_build_proposal(&proposal, None, &[], None);
        assert!(issues.iter().any(|i| matches!(
            i,
            ValidationIssue::HardcodedGalleryItems { item_count, .. } if *item_count == 2
        )));
    }

    #[test]
    fn gallery_with_pipeline_is_not_flagged_for_hardcoded_items() {
        let plan = BuildDatasourcePlan {
            kind: BuildDatasourcePlanKind::BuiltinTool,
            tool_name: Some("http_request".into()),
            server_id: None,
            arguments: Some(json!({"method": "GET", "url": "https://example.test/x"})),
            prompt: None,
            output_path: None,
            refresh_cron: None,
            pipeline: vec![crate::models::pipeline::PipelineStep::Pick {
                path: "items".into(),
            }],
            source_key: None,
            inputs: None,
        };
        let proposal = BuildProposal {
            id: "p3".into(),
            title: "T".into(),
            summary: None,
            dashboard_name: None,
            dashboard_description: None,
            widgets: vec![BuildWidgetProposal {
                widget_type: BuildWidgetType::Gallery,
                title: "Cats".into(),
                data: json!([{"src": "https://a/1.jpg"}]),
                datasource_plan: Some(plan),
                config: None,
                x: None,
                y: None,
                w: None,
                h: None,
                replace_widget_id: None,
                size_preset: None,
                layout_pattern: None,
            }],
            remove_widget_ids: Vec::new(),
            shared_datasources: Vec::new(),
            parameters: Vec::new(),
        };
        let issues = validate_build_proposal(&proposal, None, &[], None);
        assert!(!issues
            .iter()
            .any(|i| matches!(i, ValidationIssue::HardcodedGalleryItems { .. })));
    }

    #[test]
    fn explicit_coordinates_are_flagged() {
        let plan = BuildDatasourcePlan {
            kind: BuildDatasourcePlanKind::McpTool,
            tool_name: Some("test_tool".into()),
            server_id: Some("test_server".into()),
            arguments: None,
            prompt: None,
            output_path: None,
            refresh_cron: None,
            pipeline: Vec::new(),
            source_key: None,
            inputs: None,
        };
        let mut proposal = proposal_with_widget(plan, Vec::new());
        proposal.widgets[0].x = Some(4);
        proposal.widgets[0].y = Some(2);
        let issues = validate_build_proposal(&proposal, None, &[], None);
        assert!(issues
            .iter()
            .any(|i| matches!(i, ValidationIssue::ProposedExplicitCoordinates { .. })));
    }

    #[test]
    fn replace_widget_skips_layout_gate() {
        let plan = BuildDatasourcePlan {
            kind: BuildDatasourcePlanKind::McpTool,
            tool_name: Some("test_tool".into()),
            server_id: Some("test_server".into()),
            arguments: None,
            prompt: None,
            output_path: None,
            refresh_cron: None,
            pipeline: Vec::new(),
            source_key: None,
            inputs: None,
        };
        let mut proposal = proposal_with_widget(plan, Vec::new());
        proposal.widgets[0].replace_widget_id = Some("w_old".into());
        proposal.widgets[0].x = Some(4);
        proposal.widgets[0].y = Some(2);
        proposal.widgets[0].size_preset = Some(crate::models::dashboard::SizePreset::Kpi);
        proposal.widgets[0].w = Some(6);
        let issues = validate_build_proposal(&proposal, None, &[], None);
        assert!(!issues
            .iter()
            .any(|i| matches!(i, ValidationIssue::ProposedExplicitCoordinates { .. })));
        assert!(!issues
            .iter()
            .any(|i| matches!(i, ValidationIssue::ConflictingLayoutFields { .. })));
    }

    #[test]
    fn size_preset_with_explicit_wh_is_flagged() {
        let plan = BuildDatasourcePlan {
            kind: BuildDatasourcePlanKind::McpTool,
            tool_name: Some("test_tool".into()),
            server_id: Some("test_server".into()),
            arguments: None,
            prompt: None,
            output_path: None,
            refresh_cron: None,
            pipeline: Vec::new(),
            source_key: None,
            inputs: None,
        };
        let mut proposal = proposal_with_widget(plan, Vec::new());
        proposal.widgets[0].size_preset = Some(crate::models::dashboard::SizePreset::Kpi);
        proposal.widgets[0].w = Some(6);
        let issues = validate_build_proposal(&proposal, None, &[], None);
        assert!(issues
            .iter()
            .any(|i| matches!(i, ValidationIssue::ConflictingLayoutFields { .. })));
    }

    fn compose_proposal_referencing(def_ids: &[&str], workflow_ids: &[&str]) -> BuildProposal {
        let mut inputs = std::collections::BTreeMap::new();
        for (i, id) in def_ids.iter().enumerate() {
            let alias = format!("def_{}", i);
            inputs.insert(
                alias,
                BuildDatasourcePlan {
                    kind: BuildDatasourcePlanKind::McpTool,
                    tool_name: Some("noop".into()),
                    server_id: Some("srv".into()),
                    arguments: Some(json!({ "datasource_definition_id": id })),
                    prompt: None,
                    output_path: None,
                    refresh_cron: None,
                    pipeline: Vec::new(),
                    source_key: None,
                    inputs: None,
                },
            );
        }
        for (i, id) in workflow_ids.iter().enumerate() {
            let alias = format!("wf_{}", i);
            inputs.insert(
                alias,
                BuildDatasourcePlan {
                    kind: BuildDatasourcePlanKind::McpTool,
                    tool_name: Some("noop".into()),
                    server_id: Some("srv".into()),
                    arguments: Some(json!({ "workflow_id": id })),
                    prompt: None,
                    output_path: None,
                    refresh_cron: None,
                    pipeline: Vec::new(),
                    source_key: None,
                    inputs: None,
                },
            );
        }
        let plan = BuildDatasourcePlan {
            kind: BuildDatasourcePlanKind::Compose,
            tool_name: None,
            server_id: None,
            arguments: None,
            prompt: None,
            output_path: None,
            refresh_cron: None,
            pipeline: Vec::new(),
            source_key: None,
            inputs: Some(inputs),
        };
        BuildProposal {
            id: "p".into(),
            title: "t".into(),
            summary: None,
            dashboard_name: None,
            dashboard_description: None,
            widgets: vec![BuildWidgetProposal {
                widget_type: BuildWidgetType::Text,
                title: "narrative".into(),
                data: json!(""),
                datasource_plan: Some(plan),
                config: None,
                x: None,
                y: None,
                w: None,
                h: None,
                replace_widget_id: None,
                size_preset: None,
                layout_pattern: None,
            }],
            remove_widget_ids: Vec::new(),
            shared_datasources: Vec::new(),
            parameters: Vec::new(),
        }
    }

    #[test]
    fn unused_source_mention_is_flagged() {
        let proposal = compose_proposal_referencing(&["def-A"], &[]);
        let mentions = vec![
            MentionedSource {
                label: "Forecast".into(),
                datasource_definition_id: Some("def-A".into()),
                workflow_id: None,
            },
            MentionedSource {
                label: "Air quality".into(),
                datasource_definition_id: Some("def-B".into()),
                workflow_id: None,
            },
        ];
        let issues =
            validate_build_proposal_full(&proposal, None, &[], None, Some(mentions.as_slice()));
        let unused = issues.iter().find_map(|i| match i {
            ValidationIssue::UnusedSourceMention { missing } => Some(missing),
            _ => None,
        });
        let missing = unused.expect("UnusedSourceMention issue expected");
        assert_eq!(missing.len(), 1);
        assert_eq!(
            missing[0].datasource_definition_id.as_deref(),
            Some("def-B")
        );
    }

    #[test]
    fn all_source_mentions_covered_passes() {
        let proposal = compose_proposal_referencing(&["def-A", "def-B"], &[]);
        let mentions = vec![
            MentionedSource {
                label: "Forecast".into(),
                datasource_definition_id: Some("def-A".into()),
                workflow_id: None,
            },
            MentionedSource {
                label: "Air quality".into(),
                datasource_definition_id: Some("def-B".into()),
                workflow_id: None,
            },
        ];
        let issues =
            validate_build_proposal_full(&proposal, None, &[], None, Some(mentions.as_slice()));
        assert!(!issues
            .iter()
            .any(|i| matches!(i, ValidationIssue::UnusedSourceMention { .. })));
    }

    #[test]
    fn legacy_workflow_mention_matches_workflow_id_reference() {
        let proposal = compose_proposal_referencing(&[], &["wf-legacy"]);
        let mentions = vec![MentionedSource {
            label: "Legacy".into(),
            datasource_definition_id: None,
            workflow_id: Some("wf-legacy".into()),
        }];
        let issues =
            validate_build_proposal_full(&proposal, None, &[], None, Some(mentions.as_slice()));
        assert!(!issues
            .iter()
            .any(|i| matches!(i, ValidationIssue::UnusedSourceMention { .. })));
    }

    #[test]
    fn empty_target_set_disables_off_target_gate() {
        // When the user did not mention any widgets, the off-target gate
        // must NOT fire. The agent may touch any widget.
        let proposal = targeted_proposal("w_anything", vec!["w_else".to_string()]);
        let issues = validate_build_proposal(&proposal, None, &[], None);
        assert!(!issues
            .iter()
            .any(|i| matches!(i, ValidationIssue::OffTargetWidgetReplace { .. })));
        assert!(!issues
            .iter()
            .any(|i| matches!(i, ValidationIssue::OffTargetWidgetRemove { .. })));
    }
}

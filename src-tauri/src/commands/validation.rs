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
    BuildDatasourcePlanKind, BuildProposal, BuildWidgetProposal, BuildWidgetType, Dashboard,
};
use crate::models::pipeline::PipelineStep;
use crate::models::validation::ValidationIssue;

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
    }
}

/// Run the full proposal validation gate. The dashboard reference is the
/// dashboard the proposal will be applied against; `None` means the
/// proposal targets a new dashboard, so `replace_widget_id` checks
/// degrade to "any string is unknown".
pub fn validate_build_proposal(
    proposal: &BuildProposal,
    dashboard: Option<&Dashboard>,
    transcript: &[ChatMessage],
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

        if let Some(replace_id) = widget.replace_widget_id.as_ref() {
            if !existing_widget_ids.contains(replace_id) {
                issues.push(ValidationIssue::UnknownReplaceWidgetId {
                    widget_index,
                    widget_title: widget_title.clone(),
                    replace_widget_id: replace_id.clone(),
                });
            }
        }

        validate_widget_pipeline(widget_index, &widget_title, widget, &mut issues);
        validate_text_widget_markdown(widget_index, &widget_title, widget, &mut issues);
        validate_no_hardcoded_value(widget_index, &widget_title, widget, &mut issues);

        if requires_dry_run(&widget.widget_type)
            && !dry_run_titles.contains(widget_title.trim())
            && !dry_run_titles.contains(widget_title.trim().to_lowercase().as_str())
        {
            issues.push(ValidationIssue::MissingDryRunEvidence {
                widget_index,
                widget_title: widget_title.clone(),
                widget_kind: widget_kind_label(&widget.widget_type).to_string(),
            });
        }
    }

    issues
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
        let has_pipeline = widget
            .datasource_plan
            .as_ref()
            .is_some_and(|plan| !plan.pipeline.is_empty() || plan.source_key.is_some());
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
    let mut call_id_to_title: BTreeMap<String, String> = BTreeMap::new();

    for message in transcript {
        if let Some(calls) = &message.tool_calls {
            for call in calls {
                if call.name != "dry_run_widget" {
                    continue;
                }
                if let Some(title) = extract_dry_run_widget_title(&call.arguments) {
                    call_id_to_title.insert(call.id.clone(), title);
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
                if let Some(title) = call_id_to_title.get(&result.tool_call_id) {
                    titles.insert(title.clone());
                    titles.insert(title.to_lowercase());
                }
            }
        }
    }

    titles
}

fn extract_dry_run_widget_title(arguments: &Value) -> Option<String> {
    // The agent passes the widget proposal either as `arguments.proposal`
    // or as the top-level object. Honour both shapes.
    let proposal = arguments.get("proposal").unwrap_or(arguments);
    let title = proposal
        .get("title")
        .and_then(|v| v.as_str())?
        .trim()
        .to_string();
    if title.is_empty() {
        None
    } else {
        Some(title)
    }
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

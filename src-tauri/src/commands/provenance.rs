//! W41: widget execution observability — `get_widget_provenance`.
//!
//! Resolves the widget → datasource → workflow → source call chain into a
//! typed summary suitable for the inspector UI and reflection prompts.
//! Secrets are redacted at the Rust boundary via
//! [`crate::models::provenance::redact_value`]; sample payloads are
//! intentionally *not* persisted here — the W23 traces and W35 runs are
//! still the source of truth for raw data.

use anyhow::{anyhow, Result as AnyResult};
use serde_json::Value;
use tauri::State;

use crate::commands::datasource::{widget_datasource, widget_kind};
use crate::models::pipeline::PipelineStep;
use crate::models::provenance::{
    redact_value, ComposeInputSummary, DatasourceProvenance, LastRunSummary, LlmParticipation,
    ProvenanceLinks, ProviderProvenance, SourceProvenance, TailSummary, WidgetProvenance,
};
use crate::models::provider::{EffectiveWidgetModel, LLMProvider, WidgetModelSource};
use crate::models::workflow::{NodeKind, Workflow, WorkflowNode};
use crate::models::ApiResult;
use crate::modules::storage::Storage;
use crate::AppState;

#[tauri::command]
pub async fn get_widget_provenance(
    state: State<'_, AppState>,
    dashboard_id: String,
    widget_id: String,
) -> Result<ApiResult<WidgetProvenance>, String> {
    let app_provider = crate::commands::dashboard::active_provider_public(&state)
        .await
        .ok()
        .flatten();
    // W43: resolve the widget-effective model so the inspector can
    // render the inheritance badge ("widget override" / "dashboard
    // default" / "app active provider"). Falls back to the legacy
    // app-active-provider shape silently on resolution error so the
    // inspector stays open even when the policy points at a missing
    // provider — the typed error already surfaces on the next refresh.
    let effective = match resolve_effective_for_inspector(
        state.storage.as_ref(),
        &dashboard_id,
        &widget_id,
        app_provider.as_ref(),
    )
    .await
    {
        Ok(value) => value,
        Err(_) => None,
    };
    Ok(
        match build_widget_provenance(
            state.storage.as_ref(),
            app_provider.as_ref(),
            effective.as_ref(),
            &dashboard_id,
            &widget_id,
        )
        .await
        {
            Ok(prov) => ApiResult::ok(prov),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

async fn resolve_effective_for_inspector(
    storage: &Storage,
    dashboard_id: &str,
    widget_id: &str,
    app_provider: Option<&LLMProvider>,
) -> AnyResult<Option<EffectiveWidgetModel>> {
    let Some(dashboard) = storage.get_dashboard(dashboard_id).await? else {
        return Ok(None);
    };
    let Some(widget) = dashboard
        .layout
        .iter()
        .find(|w| w.id() == widget_id)
        .cloned()
    else {
        return Ok(None);
    };
    let providers = storage.list_providers().await?;
    crate::modules::ai::resolve_effective_widget_model(
        &widget,
        &dashboard,
        &providers,
        app_provider,
    )
    .map_err(|error| anyhow!(error.to_string()))
}

/// Resolve a widget's full provenance summary against the live storage.
///
/// `active_provider` is supplied by the caller so this function does not
/// reach back into `AppState` — that keeps it callable both from the
/// Tauri command path and from background tasks (e.g. the W18 reflection
/// turn) that already hold an `AppState` directly.
pub async fn build_widget_provenance(
    storage: &Storage,
    active_provider: Option<&LLMProvider>,
    effective_model: Option<&EffectiveWidgetModel>,
    dashboard_id: &str,
    widget_id: &str,
) -> AnyResult<WidgetProvenance> {
    let dashboard = storage
        .get_dashboard(dashboard_id)
        .await?
        .ok_or_else(|| anyhow!("Dashboard not found"))?;
    let widget = dashboard
        .layout
        .iter()
        .find(|w| w.id() == widget_id)
        .ok_or_else(|| anyhow!("Widget not found"))?;
    let widget_title = widget.title().to_string();
    let widget_kind_str = widget_kind(widget).to_string();

    let mut links = ProvenanceLinks::default();

    // No bound datasource: deterministic-only widget config. There's still
    // a meaningful answer ("no LLM, no calls") so we surface it instead of
    // erroring.
    let Some(ds_config) = widget_datasource(widget) else {
        return Ok(WidgetProvenance {
            dashboard_id: dashboard_id.to_string(),
            widget_id: widget_id.to_string(),
            widget_title,
            widget_kind: widget_kind_str,
            llm_participation: LlmParticipation::None,
            datasource: None,
            provider: None,
            tail: TailSummary {
                step_count: 0,
                has_llm_postprocess: false,
                has_mcp_call: false,
                kinds: Vec::new(),
            },
            last_run: None,
            links,
            redacted_summary: "No datasource bound to this widget — value is static.".to_string(),
        });
    };

    links.workflow_id = Some(ds_config.workflow_id.clone());
    links.datasource_definition_id = ds_config.datasource_definition_id.clone();

    let workflow = match storage.get_workflow(&ds_config.workflow_id).await? {
        Some(w) => Some(w),
        None => dashboard
            .workflows
            .iter()
            .find(|w| w.id == ds_config.workflow_id)
            .cloned(),
    };

    let datasource_definition = match ds_config.datasource_definition_id.as_deref() {
        Some(id) => storage.get_datasource_definition(id).await?,
        None => {
            storage
                .get_datasource_by_workflow_id(&ds_config.workflow_id)
                .await?
        }
    };
    let datasource_name = datasource_definition.as_ref().map(|d| d.name.clone());

    let (source, source_uses_provider) = match workflow.as_ref() {
        Some(wf) => derive_source_provenance(wf),
        None => (
            SourceProvenance::Missing {
                workflow_id: ds_config.workflow_id.clone(),
            },
            false,
        ),
    };

    let pipeline_steps: Vec<PipelineStep> = workflow
        .as_ref()
        .and_then(|wf| pipeline_steps_from_workflow(wf).ok())
        .unwrap_or_default();
    let tail_steps: &[PipelineStep] = &ds_config.tail_pipeline;

    let tail = build_tail_summary(&pipeline_steps, tail_steps);
    let participation = classify_llm_participation(&source, source_uses_provider, &tail);
    let provider = if matches!(
        participation,
        LlmParticipation::ProviderSource | LlmParticipation::LlmPostprocess
    ) {
        // W43: when the widget resolves to an effective model (widget
        // override / dashboard default / app active), surface that.
        // Otherwise fall back to the active provider so legacy widgets
        // still report something useful.
        match effective_model {
            Some(model) => Some(provider_provenance_from_effective(model)),
            None => active_provider.map(provider_provenance_from),
        }
    } else {
        None
    };

    let trigger = workflow.as_ref().map(|wf| wf.trigger.kind.clone());
    let refresh_cron = workflow
        .as_ref()
        .and_then(|wf| wf.trigger.config.as_ref())
        .and_then(|c| c.cron.clone());
    let pause_state = workflow.as_ref().map(|wf| wf.pause_state);

    let last_run = storage
        .list_workflow_run_summaries(Some(&ds_config.workflow_id), 1)
        .await
        .ok()
        .and_then(|rows| rows.into_iter().next())
        .map(|summary| LastRunSummary {
            run_id: summary.id,
            status: summary.status,
            started_at: summary.started_at,
            finished_at: summary.finished_at,
            duration_ms: summary.duration_ms,
            error: summary.error,
        });

    let has_traces = storage
        .list_widget_traces(widget_id)
        .await
        .map(|rows| !rows.is_empty())
        .unwrap_or(false);
    links.has_pipeline_traces = has_traces;

    let datasource = DatasourceProvenance {
        workflow_id: ds_config.workflow_id.clone(),
        output_key: ds_config.output_key.clone(),
        datasource_definition_id: ds_config.datasource_definition_id.clone(),
        datasource_name: datasource_name.clone(),
        binding_source: ds_config.binding_source,
        bound_at: ds_config.bound_at,
        source,
        trigger,
        refresh_cron,
        pause_state,
    };

    let redacted_summary = render_summary(
        &widget_title,
        &widget_kind_str,
        &datasource,
        provider.as_ref(),
        &tail,
        participation,
        last_run.as_ref(),
    );

    Ok(WidgetProvenance {
        dashboard_id: dashboard_id.to_string(),
        widget_id: widget_id.to_string(),
        widget_title,
        widget_kind: widget_kind_str,
        llm_participation: participation,
        datasource: Some(datasource),
        provider,
        tail,
        last_run,
        links,
        redacted_summary,
    })
}

/// Returns `(source, source_uses_provider)`. The second flag is `true`
/// when the workflow's data-producing step is a provider prompt (Llm
/// node) — that's our ProviderSource participation signal.
fn derive_source_provenance(workflow: &Workflow) -> (SourceProvenance, bool) {
    // Compose plans don't have a single "source" node; the build path
    // produces N data-producing nodes (one per named input) and a merge
    // node that joins them. Recognise that by counting non-pipeline,
    // non-output nodes.
    let data_nodes: Vec<&WorkflowNode> = workflow
        .nodes
        .iter()
        .filter(|n| {
            !matches!(
                n.kind,
                NodeKind::Output | NodeKind::Transform | NodeKind::Merge
            )
        })
        .collect();

    if data_nodes.len() > 1 {
        let mut inputs = Vec::with_capacity(data_nodes.len());
        let mut uses_provider = false;
        for node in &data_nodes {
            let (source, prov) = source_from_node(node);
            uses_provider = uses_provider || prov;
            inputs.push(ComposeInputSummary {
                name: node.id.clone(),
                source: Box::new(source),
            });
        }
        return (SourceProvenance::Compose { inputs }, uses_provider);
    }

    // Prefer the canonical "source" id, fall back to the only data node.
    let node = workflow
        .nodes
        .iter()
        .find(|n| n.id == "source")
        .or_else(|| data_nodes.first().copied());
    match node {
        Some(node) => source_from_node(node),
        None => (SourceProvenance::Unknown, false),
    }
}

fn source_from_node(node: &WorkflowNode) -> (SourceProvenance, bool) {
    let config = node.config.clone().unwrap_or(Value::Null);
    match node.kind {
        NodeKind::McpTool => {
            let server_id = config
                .get("server_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let tool_name = config
                .get("tool_name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let arguments_preview = config.get("arguments").map(redact_value);
            let source = if server_id == "builtin" {
                SourceProvenance::BuiltinTool {
                    tool_name,
                    arguments_preview,
                }
            } else {
                SourceProvenance::McpTool {
                    server_id,
                    tool_name,
                    arguments_preview,
                }
            };
            (source, false)
        }
        NodeKind::Llm => {
            let prompt_preview = config
                .get("prompt")
                .and_then(Value::as_str)
                .map(truncate_prompt)
                .unwrap_or_default();
            (SourceProvenance::ProviderPrompt { prompt_preview }, true)
        }
        _ => (SourceProvenance::Unknown, false),
    }
}

fn truncate_prompt(prompt: &str) -> String {
    const MAX: usize = 240;
    if prompt.chars().count() <= MAX {
        return prompt.to_string();
    }
    let truncated: String = prompt.chars().take(MAX).collect();
    format!("{}…", truncated)
}

fn pipeline_steps_from_workflow(workflow: &Workflow) -> AnyResult<Vec<PipelineStep>> {
    let Some(pipeline_node) = workflow
        .nodes
        .iter()
        .find(|n| n.id == "pipeline" && matches!(n.kind, NodeKind::Transform))
    else {
        return Ok(Vec::new());
    };
    let Some(config) = pipeline_node.config.as_ref() else {
        return Ok(Vec::new());
    };
    let Some(steps_value) = config.get("steps").cloned() else {
        return Ok(Vec::new());
    };
    let steps: Vec<PipelineStep> = serde_json::from_value(steps_value)
        .map_err(|e| anyhow!("pipeline steps malformed: {}", e))?;
    Ok(steps)
}

fn build_tail_summary(pipeline: &[PipelineStep], tail: &[PipelineStep]) -> TailSummary {
    let total = pipeline.len() + tail.len();
    let combined = pipeline.iter().chain(tail.iter());
    let mut kinds = Vec::with_capacity(total);
    let mut has_llm_postprocess = false;
    let mut has_mcp_call = false;
    for step in combined {
        let kind = step_kind(step);
        if matches!(step, PipelineStep::LlmPostprocess { .. }) {
            has_llm_postprocess = true;
        }
        if matches!(step, PipelineStep::McpCall { .. }) {
            has_mcp_call = true;
        }
        kinds.push(kind);
    }
    TailSummary {
        step_count: total as u32,
        has_llm_postprocess,
        has_mcp_call,
        kinds,
    }
}

fn step_kind(step: &PipelineStep) -> String {
    match step {
        PipelineStep::Pick { .. } => "pick",
        PipelineStep::Filter { .. } => "filter",
        PipelineStep::Sort { .. } => "sort",
        PipelineStep::Limit { .. } => "limit",
        PipelineStep::Map { .. } => "map",
        PipelineStep::Aggregate { .. } => "aggregate",
        PipelineStep::Set { .. } => "set",
        PipelineStep::Head => "head",
        PipelineStep::Tail => "tail",
        PipelineStep::Length => "length",
        PipelineStep::Flatten => "flatten",
        PipelineStep::Unique { .. } => "unique",
        PipelineStep::Format { .. } => "format",
        PipelineStep::Coerce { .. } => "coerce",
        PipelineStep::LlmPostprocess { .. } => "llm_postprocess",
        PipelineStep::McpCall { .. } => "mcp_call",
    }
    .to_string()
}

fn classify_llm_participation(
    source: &SourceProvenance,
    source_uses_provider: bool,
    tail: &TailSummary,
) -> LlmParticipation {
    if let SourceProvenance::Missing { .. } | SourceProvenance::Unknown = source {
        // Pipeline can still pin the answer when source resolution fails.
        if tail.has_llm_postprocess {
            return LlmParticipation::LlmPostprocess;
        }
        return LlmParticipation::Unknown;
    }
    if source_uses_provider {
        return LlmParticipation::ProviderSource;
    }
    if tail.has_llm_postprocess {
        return LlmParticipation::LlmPostprocess;
    }
    LlmParticipation::None
}

fn provider_provenance_from(provider: &LLMProvider) -> ProviderProvenance {
    let kind = serde_json::to_value(provider.kind)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "unknown".to_string());
    ProviderProvenance {
        provider_id: provider.id.clone(),
        provider_name: provider.name.clone(),
        provider_kind: kind,
        model: provider.default_model.clone(),
        is_active_provider: true,
        model_source: None,
        required_caps: Vec::new(),
    }
}

fn provider_provenance_from_effective(model: &EffectiveWidgetModel) -> ProviderProvenance {
    let kind = serde_json::to_value(model.provider.kind)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "unknown".to_string());
    ProviderProvenance {
        provider_id: model.provider.id.clone(),
        provider_name: model.provider.name.clone(),
        provider_kind: kind,
        model: model.model.clone(),
        is_active_provider: matches!(model.source, WidgetModelSource::AppActiveProvider),
        model_source: Some(model.source),
        required_caps: model.required_caps.clone(),
    }
}

fn render_summary(
    title: &str,
    widget_kind_str: &str,
    datasource: &DatasourceProvenance,
    provider: Option<&ProviderProvenance>,
    tail: &TailSummary,
    participation: LlmParticipation,
    last_run: Option<&LastRunSummary>,
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Widget {:?} ({}) — participation={}",
        title,
        widget_kind_str,
        participation_label(participation)
    ));
    out.push_str(&format!("\n- workflow_id: {}", datasource.workflow_id));
    if let Some(name) = datasource.datasource_name.as_deref() {
        out.push_str(&format!("\n- datasource: {}", name));
    }
    match &datasource.source {
        SourceProvenance::McpTool {
            server_id,
            tool_name,
            ..
        } => out.push_str(&format!(
            "\n- source: mcp_tool {}::{}",
            server_id, tool_name
        )),
        SourceProvenance::BuiltinTool { tool_name, .. } => {
            out.push_str(&format!("\n- source: builtin_tool {}", tool_name))
        }
        SourceProvenance::ProviderPrompt { prompt_preview } => {
            out.push_str(&format!(
                "\n- source: provider_prompt (\"{}\")",
                prompt_preview
            ));
        }
        SourceProvenance::Compose { inputs } => {
            out.push_str(&format!("\n- source: compose ({} input(s))", inputs.len()))
        }
        SourceProvenance::Unknown => out.push_str("\n- source: unknown"),
        SourceProvenance::Missing { workflow_id } => out.push_str(&format!(
            "\n- source: missing (workflow {} not found)",
            workflow_id
        )),
    }
    if let Some(p) = provider {
        out.push_str(&format!(
            "\n- provider: {} ({}) model={}",
            p.provider_name, p.provider_kind, p.model
        ));
    }
    out.push_str(&format!(
        "\n- pipeline: {} step(s) [llm_postprocess={}, mcp_call={}]",
        tail.step_count, tail.has_llm_postprocess, tail.has_mcp_call
    ));
    if let Some(run) = last_run {
        let status_label = serde_json::to_value(&run.status)
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "unknown".to_string());
        out.push_str(&format!(
            "\n- last_run: status={} duration_ms={:?}",
            status_label, run.duration_ms
        ));
        if let Some(error) = run.error.as_deref() {
            out.push_str(&format!(" error={:?}", error));
        }
    } else {
        out.push_str("\n- last_run: not captured");
    }
    out
}

fn participation_label(p: LlmParticipation) -> &'static str {
    match p {
        LlmParticipation::None => "none",
        LlmParticipation::ProviderSource => "provider_source",
        LlmParticipation::LlmPostprocess => "llm_postprocess",
        LlmParticipation::WidgetTextGeneration => "widget_text_generation",
        LlmParticipation::Unknown => "unknown",
    }
}

/// Compact provenance text feed for the reflection / Build Chat path.
/// Mirrors `render_summary` but accepts the full [`WidgetProvenance`]
/// envelope so callers can paste it verbatim into the LLM prompt.
pub fn provenance_summary_for_reflection(prov: &WidgetProvenance) -> String {
    if let Some(ds) = prov.datasource.as_ref() {
        render_summary(
            &prov.widget_title,
            &prov.widget_kind,
            ds,
            prov.provider.as_ref(),
            &prov.tail,
            prov.llm_participation,
            prov.last_run.as_ref(),
        )
    } else {
        prov.redacted_summary.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::pipeline::{LlmExpect, PipelineStep};
    use crate::models::workflow::{NodeKind, WorkflowNode};
    use serde_json::json;

    fn mcp_node() -> WorkflowNode {
        WorkflowNode {
            id: "source".into(),
            kind: NodeKind::McpTool,
            label: "src".into(),
            position: None,
            config: Some(json!({
                "server_id": "hn",
                "tool_name": "get_top_stories",
                "arguments": {
                    "limit": 5,
                    "headers": { "Authorization": "Bearer leak", "Accept": "application/json" }
                }
            })),
        }
    }

    fn llm_node() -> WorkflowNode {
        WorkflowNode {
            id: "source".into(),
            kind: NodeKind::Llm,
            label: "src".into(),
            position: None,
            config: Some(json!({ "prompt": "Summarise yesterday's top stories." })),
        }
    }

    #[test]
    fn mcp_source_strips_sensitive_headers() {
        let (source, uses_provider) = source_from_node(&mcp_node());
        assert!(!uses_provider);
        match source {
            SourceProvenance::McpTool {
                server_id,
                tool_name,
                arguments_preview,
            } => {
                assert_eq!(server_id, "hn");
                assert_eq!(tool_name, "get_top_stories");
                let args = arguments_preview.expect("arguments preview is included");
                assert_eq!(args["headers"]["Authorization"], json!("<redacted>"));
                assert_eq!(args["headers"]["Accept"], json!("application/json"));
                assert_eq!(args["limit"], json!(5));
            }
            other => panic!("expected mcp tool source, got {:?}", other),
        }
    }

    #[test]
    fn llm_source_flags_provider_use_and_truncates_prompt() {
        let (source, uses_provider) = source_from_node(&llm_node());
        assert!(uses_provider);
        match source {
            SourceProvenance::ProviderPrompt { prompt_preview } => {
                assert!(prompt_preview.starts_with("Summarise"));
            }
            other => panic!("expected provider prompt, got {:?}", other),
        }
    }

    #[test]
    fn pipeline_only_widget_is_classified_none() {
        let tail = build_tail_summary(
            &[
                PipelineStep::Pick {
                    path: "items".into(),
                },
                PipelineStep::Limit { count: 5 },
            ],
            &[],
        );
        let (mcp_src, uses_provider) = source_from_node(&mcp_node());
        let participation = classify_llm_participation(&mcp_src, uses_provider, &tail);
        assert_eq!(participation, LlmParticipation::None);
        assert!(!tail.has_llm_postprocess);
        assert!(!tail.has_mcp_call);
        assert_eq!(tail.step_count, 2);
    }

    #[test]
    fn llm_postprocess_marks_widget_as_llm_backed() {
        let tail = build_tail_summary(
            &[PipelineStep::Pick {
                path: "items".into(),
            }],
            &[PipelineStep::LlmPostprocess {
                prompt: "Pick three trends".into(),
                expect: LlmExpect::Text,
            }],
        );
        let (mcp_src, uses_provider) = source_from_node(&mcp_node());
        let participation = classify_llm_participation(&mcp_src, uses_provider, &tail);
        assert_eq!(participation, LlmParticipation::LlmPostprocess);
        assert!(tail.has_llm_postprocess);
    }

    #[test]
    fn provider_source_widget_is_classified_provider_source() {
        let (llm_src, uses_provider) = source_from_node(&llm_node());
        let tail = build_tail_summary(&[], &[]);
        let participation = classify_llm_participation(&llm_src, uses_provider, &tail);
        assert_eq!(participation, LlmParticipation::ProviderSource);
    }

    #[test]
    fn missing_workflow_is_explicit_unknown_unless_tail_uses_provider() {
        let source = SourceProvenance::Missing {
            workflow_id: "wf-missing".into(),
        };
        let tail = build_tail_summary(&[], &[]);
        let participation = classify_llm_participation(&source, false, &tail);
        assert_eq!(participation, LlmParticipation::Unknown);

        let tail_with_llm = build_tail_summary(
            &[],
            &[PipelineStep::LlmPostprocess {
                prompt: "x".into(),
                expect: LlmExpect::Text,
            }],
        );
        let participation_with_llm = classify_llm_participation(&source, false, &tail_with_llm);
        assert_eq!(participation_with_llm, LlmParticipation::LlmPostprocess);
    }

    #[test]
    fn reflection_summary_omits_secret_headers() {
        let (src, uses_provider) = source_from_node(&mcp_node());
        let tail = build_tail_summary(&[], &[]);
        let participation = classify_llm_participation(&src, uses_provider, &tail);
        let ds = DatasourceProvenance {
            workflow_id: "wf".into(),
            output_key: "output".into(),
            datasource_definition_id: Some("ds".into()),
            datasource_name: Some("HN top".into()),
            binding_source: None,
            bound_at: None,
            source: src,
            trigger: None,
            refresh_cron: None,
            pause_state: None,
        };
        let summary = render_summary("My widget", "table", &ds, None, &tail, participation, None);
        assert!(summary.contains("mcp_tool hn::get_top_stories"));
        assert!(!summary.to_ascii_lowercase().contains("bearer leak"));
        assert!(summary.contains("participation=none"));
    }
}

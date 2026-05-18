//! W30: saved datasource definitions used by the Workbench.
//!
//! A [`DatasourceDefinition`] is a thin product object that maps onto the
//! existing workflow engine primitives. It does **not** introduce a new
//! query engine — execution still flows through `WorkflowEngine`,
//! `ToolEngine`, `MCPManager`, provider prompts, and the typed pipeline
//! DSL. The Workbench just gives those primitives a name, an inspectable
//! pipeline editor, a sample-result view, and a list of consuming widgets.
//!
//! Persistence: one row per definition in the `datasource_definitions`
//! table. The latest run summary lives in a separate `datasource_health`
//! row keyed by definition id so frequent refreshes never rewrite the
//! definition JSON.
//!
//! Backing workflow: every persisted definition owns one workflow row,
//! created/updated through `build_shared_fanout_workflow`. Consumer
//! widgets reference the definition through the standard
//! `DatasourceConfig.workflow_id` field; the Workbench computes the
//! consumer list by scanning dashboards for that workflow id.

use super::{Id, Timestamp};
use crate::models::dashboard::BuildDatasourcePlanKind;
use crate::models::pipeline::PipelineStep;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasourceDefinition {
    pub id: Id,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub kind: BuildDatasourcePlanKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pipeline: Vec<PipelineStep>,
    /// Optional cron expression attached to the backing workflow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_cron: Option<String>,
    /// Backing workflow id rebuilt on every save. Consumer widgets bind
    /// to this through the standard `DatasourceConfig.workflow_id`.
    pub workflow_id: Id,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// Latest run summary. `None` until the definition has been
    /// executed at least once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<DatasourceHealth>,
    /// W37: when the definition was created from an external source
    /// catalog entry (`save_external_source_as_datasource`), this points
    /// back to the catalog id. Lets the Workbench badge "from <Source>"
    /// and lets the Source Catalog warn before disabling a source with
    /// active originating datasources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub originated_external_source_id: Option<String>,
}

/// Inspectable health snapshot. Updated by every test-run and by the
/// scheduler when the backing workflow fires.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DatasourceHealth {
    pub last_run_at: Timestamp,
    pub last_status: DatasourceHealthStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub last_duration_ms: u32,
    /// Truncated preview of the final pipeline value, useful for catalog
    /// rendering and for the Build chat reuse prompt. The raw value lives
    /// in pipeline traces / workflow runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_preview: Option<Value>,
    /// Number of consumer widgets observed during the last `list_consumers`
    /// scan. Stored on the health snapshot so the catalog can render a
    /// stale-but-cheap value without re-scanning every dashboard.
    #[serde(default)]
    pub consumer_count: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DatasourceHealthStatus {
    Ok,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateDatasourceRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub kind: BuildDatasourcePlanKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pipeline: Vec<PipelineStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_cron: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateDatasourceRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipeline: Option<Vec<PipelineStep>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_cron: Option<String>,
}

/// One widget consuming a datasource definition. Resolved by matching
/// widgets through their explicit `datasource_definition_id` first and
/// falling back to a `workflow_id` scan for legacy rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasourceConsumer {
    pub dashboard_id: Id,
    pub dashboard_name: String,
    pub widget_id: Id,
    pub widget_title: String,
    pub widget_kind: String,
    pub output_key: String,
    /// W31: `true` when the widget already carries an explicit
    /// `datasource_definition_id`; `false` when discovered via the
    /// legacy `workflow_id` scan and not yet upgraded.
    #[serde(default)]
    pub explicit_binding: bool,
    /// W31: surface that last wrote the binding, if known. `None` for
    /// legacy widgets that have never been upgraded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_source: Option<crate::models::widget::DatasourceBindingSource>,
    /// W31: timestamp of the last binding change, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_at: Option<Timestamp>,
    /// W31.1: number of typed pipeline steps applied per-widget after
    /// the saved datasource workflow output. Surfaces in the catalog
    /// so operators can spot consumers that re-shape the data.
    #[serde(default)]
    pub tail_step_count: u32,
}

/// W31: result of `preview_datasource_impact` — what would change for
/// existing consumers if a definition were edited or deleted. The
/// caller decides whether to proceed; this command is read-only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasourceImpactPreview {
    pub datasource_id: Id,
    pub datasource_name: String,
    pub workflow_id: Id,
    pub consumers: Vec<DatasourceConsumer>,
    /// Consumers still bound only by `workflow_id` (no explicit
    /// `datasource_definition_id`). Editing the datasource will affect
    /// them, but a delete-and-recreate would orphan them.
    pub legacy_consumer_count: u32,
    /// `true` when at least one consumer carries an explicit binding.
    pub has_explicit_consumers: bool,
}

/// W31: result envelope for the bind/unbind commands so the UI can show
/// what actually changed without re-fetching the dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasourceBindingChange {
    pub dashboard_id: Id,
    pub widget_id: Id,
    pub datasource_definition_id: Option<Id>,
    pub workflow_id: Option<Id>,
    pub binding_source: Option<crate::models::widget::DatasourceBindingSource>,
    pub previous_workflow_id: Option<Id>,
    pub previous_datasource_definition_id: Option<Id>,
}

/// Result envelope for the test-run command. Mirrors the most useful
/// fields from a [`PipelineTrace`] without coupling the Workbench UI to
/// the trace model directly. The full trace is still available via the
/// W23 debug commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasourceRunResult {
    pub status: DatasourceHealthStatus,
    pub duration_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_source: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_value: Option<Value>,
    pub pipeline_steps: u32,
    #[serde(default)]
    pub workflow_node_ids: Vec<String>,
}

/// Portable bundle for local backup / handoff. Versioned so we can
/// reject unknown shapes cleanly instead of silently round-tripping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasourceExportBundle {
    /// `1` for the W30 baseline shape. Incremented on breaking changes.
    pub version: u32,
    pub exported_at: Timestamp,
    pub definitions: Vec<DatasourceDefinition>,
}

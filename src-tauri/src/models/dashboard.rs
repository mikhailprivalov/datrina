use super::{Id, Timestamp};
use crate::models::widget::Widget;
use crate::models::workflow::Workflow;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dashboard {
    pub id: Id,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub layout: Vec<Widget>,
    pub workflows: Vec<Workflow>,
    pub is_default: bool,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateDashboardRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<CreateDashboardTemplate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreateDashboardTemplate {
    Blank,
    LocalMvp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateDashboardRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<Vec<Widget>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflows: Option<Vec<Workflow>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddWidgetRequest {
    pub widget_type: DashboardWidgetType,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardWidgetType {
    Text,
    Gauge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyBuildChangeRequest {
    pub action: BuildChangeAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dashboard_id: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildChangeAction {
    CreateLocalDashboard,
    AddTextWidget,
    AddGaugeWidget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyBuildProposalRequest {
    pub proposal: BuildProposal,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dashboard_id: Option<Id>,
    pub confirmed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildProposal {
    pub id: Id,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dashboard_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dashboard_description: Option<String>,
    #[serde(default)]
    pub widgets: Vec<BuildWidgetProposal>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove_widget_ids: Vec<Id>,
    /// Named upstream datasources shared by several widgets. Each entry runs
    /// its source + base pipeline once per refresh; widgets that reference
    /// the entry by `source_key` get fanned out from the same data with
    /// their own per-widget pipeline tail. Saves MCP/HTTP calls and keeps
    /// related widgets consistent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_datasources: Vec<SharedDatasource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedDatasource {
    /// Stable name used by consumer widgets via
    /// `datasource_plan.source_key`. Must be unique within this proposal.
    pub key: String,
    pub kind: BuildDatasourcePlanKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_id: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Optional base pipeline applied once to the raw source output before
    /// it is fanned out to consumer widgets. Each consumer can apply its
    /// own pipeline on top.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pipeline: Vec<crate::models::pipeline::PipelineStep>,
    /// Optional cron expression for periodic refresh. The cron is attached
    /// to the shared workflow, so a single tick refreshes every consumer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_cron: Option<String>,
    /// Optional human label for tracing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildWidgetProposal {
    pub widget_type: BuildWidgetType,
    pub title: String,
    #[serde(default)]
    pub data: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datasource_plan: Option<BuildDatasourcePlan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub w: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub h: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replace_widget_id: Option<Id>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildWidgetType {
    Chart,
    Text,
    Table,
    Image,
    Gauge,
    Stat,
    Logs,
    BarGauge,
    StatusGrid,
    Heatmap,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildDatasourcePlan {
    pub kind: BuildDatasourcePlanKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_id: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_cron: Option<String>,
    /// Optional deterministic transform pipeline applied to the datasource
    /// output before reaching the widget. Each step is a typed JSON object
    /// (see `models::pipeline::PipelineStep`); the final step may be an
    /// optional `llm_postprocess` for shapes the deterministic steps cannot
    /// produce on their own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pipeline: Vec<crate::models::pipeline::PipelineStep>,
    /// For `kind: "shared"` plans, the `key` of the matching
    /// `proposal.shared_datasources` entry whose output feeds this widget.
    /// Other plan fields (tool_name, server_id, arguments, prompt,
    /// refresh_cron) are ignored when source_key is set; only `pipeline`
    /// (applied AFTER the shared base pipeline) is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildDatasourcePlanKind {
    BuiltinTool,
    McpTool,
    /// Reference a `proposal.shared_datasources[<source_key>]` entry. The
    /// shared workflow handles the actual fetch + base pipeline; the
    /// widget's own `pipeline` field is applied on top as a per-widget
    /// tail.
    Shared,
    ProviderPrompt,
}

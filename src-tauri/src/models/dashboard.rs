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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildWidgetType {
    Chart,
    Text,
    Table,
    Image,
    Gauge,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildDatasourcePlanKind {
    BuiltinTool,
    McpTool,
    ProviderPrompt,
}

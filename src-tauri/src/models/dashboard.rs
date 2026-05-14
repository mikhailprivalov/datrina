use super::{Id, Timestamp};
use crate::models::widget::Widget;
use crate::models::workflow::Workflow;
use serde::{Deserialize, Serialize};

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

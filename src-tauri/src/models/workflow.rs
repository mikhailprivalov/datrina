use super::{Id, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const WORKFLOW_EVENT_CHANNEL: &str = "workflow:event";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    pub id: Id,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub nodes: Vec<WorkflowNode>,
    pub edges: Vec<WorkflowEdge>,
    pub trigger: WorkflowTrigger,
    #[serde(default = "default_true")]
    pub is_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run: Option<WorkflowRun>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowNode {
    pub id: Id,
    pub kind: NodeKind,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<NodePosition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodePosition {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    McpTool,
    Llm,
    Transform,
    Datasource,
    Condition,
    Merge,
    Output,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowEdge {
    pub id: Id,
    pub source: Id,
    pub target: Id,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowTrigger {
    pub kind: TriggerKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<TriggerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerKind {
    Cron,
    Event,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRun {
    pub id: Id,
    pub started_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<Timestamp>,
    pub status: RunStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_results: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Idle,
    Running,
    Success,
    Error,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowEventKind {
    RunStarted,
    NodeStarted,
    NodeFinished,
    RunFinished,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowEventEnvelope {
    pub kind: WorkflowEventKind,
    pub workflow_id: Id,
    pub run_id: Id,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<Id>,
    pub status: RunStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub emitted_at: Timestamp,
}

fn default_true() -> bool {
    true
}

use super::{Id, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const WORKFLOW_EVENT_CHANNEL: &str = "workflow:event";

// ─── W35: Operations Cockpit models ────────────────────────────────────────

/// Cheap row summary for the Operations list. Excludes node_results so
/// the run list can render without pulling potentially large JSON blobs
/// from the workflow_runs table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRunSummary {
    pub id: Id,
    pub workflow_id: Id,
    pub started_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<Timestamp>,
    pub status: RunStatus,
    /// Wall-clock duration in milliseconds when `finished_at` is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// True when the persisted row has a non-null `node_results` payload.
    /// Lets the UI hide the "open detail" affordance for empty rows.
    #[serde(default)]
    pub has_node_results: bool,
}

/// Filter parameters for `list_workflow_runs`. All fields are optional;
/// owner filters resolve to a `workflow_id` server-side so the UI can
/// drive Operations from a dashboard/widget/datasource without knowing
/// the backing workflow id.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowRunFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashboard_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub widget_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datasource_definition_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<RunStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Owner references resolved for a workflow: the saved datasource
/// definition that owns it (if any) and the dashboards/widgets that
/// consume it through `DatasourceConfig.workflow_id`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowOwnerRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datasource_definition_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datasource_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dashboards: Vec<WorkflowOwnerDashboard>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowOwnerDashboard {
    pub dashboard_id: Id,
    pub dashboard_name: String,
    pub widgets: Vec<WorkflowOwnerWidget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowOwnerWidget {
    pub widget_id: Id,
    pub widget_title: String,
    pub widget_kind: String,
    pub output_key: String,
    #[serde(default)]
    pub explicit_binding: bool,
}

/// Schedule health for a single workflow: whether it is currently
/// scheduled, the configured cron string (raw), the normalized 6-field
/// form actually accepted by the scheduler, and whether the raw cron
/// parsed cleanly. `cron_is_valid = false` means the workflow has a
/// cron trigger that the scheduler refused — surfaced as an actionable
/// warning, not a hidden no-op.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowScheduleSummary {
    pub is_scheduled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron_normalized: Option<String>,
    #[serde(default)]
    pub cron_is_valid: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_kind: Option<TriggerKind>,
    /// W50: user-controlled pause state. Independent of `is_enabled` so
    /// the operator can pause automatic refresh without disabling the
    /// workflow entirely (manual refresh still works while paused).
    #[serde(default)]
    pub pause_state: SchedulePauseState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_paused_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_pause_reason: Option<String>,
    /// W50: high-level state the UI renders directly. Derived from
    /// `pause_state`, `cron_is_valid`, `is_scheduled`, and the trigger
    /// kind so React doesn't need to replicate the rules.
    pub display_state: ScheduleDisplayState,
}

/// W50: persisted user intent for a workflow's automatic refresh. Default
/// is `Active`. A `Paused` workflow is intentionally not registered with
/// `tokio_cron_scheduler`; manual `execute_workflow` still runs.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SchedulePauseState {
    #[default]
    Active,
    Paused,
}

impl SchedulePauseState {
    pub fn as_str(&self) -> &'static str {
        match self {
            SchedulePauseState::Active => "active",
            SchedulePauseState::Paused => "paused",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw {
            "paused" => SchedulePauseState::Paused,
            _ => SchedulePauseState::Active,
        }
    }
}

/// W50: high-level schedule state surfaced to the UI. The Rust side is
/// the single source of truth so dashboard, Workbench, and Operations
/// render identical labels.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleDisplayState {
    /// Cron trigger registered and ticking.
    Active,
    /// User has explicitly paused automatic refresh.
    PausedByUser,
    /// Workflow has a cron trigger but no cron expression — manual-only.
    ManualOnly,
    /// Cron is configured but the scheduler refused it.
    Invalid,
    /// Workflow is disabled (hard off, distinct from paused).
    Disabled,
    /// Trigger kind is `Manual` or `Event` — automatic refresh not
    /// applicable; manual `Refresh` still works.
    NotScheduled,
}

/// One row in the Operations cockpit list: workflow metadata, latest
/// run summary, owner references, and schedule health.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowSummary {
    pub id: Id,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub is_enabled: bool,
    pub trigger: WorkflowTrigger,
    pub schedule: WorkflowScheduleSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<WorkflowRunSummary>,
    pub owner: WorkflowOwnerRef,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// Detail envelope returned by `get_workflow_run_detail`. The full
/// `WorkflowRun` carries node_results; the owner block lets the UI link
/// straight to the dashboard/widget/datasource that produced the run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRunDetail {
    pub run: WorkflowRun,
    pub workflow_id: Id,
    pub workflow_name: String,
    pub owner: WorkflowOwnerRef,
}

/// Result envelope for `cancel_workflow_run`. The current runtime
/// executes workflows synchronously inside the scheduler tick / command
/// future and exposes no abort handle, so cancellation honestly reports
/// `unsupported` instead of pretending. When a future runtime supports
/// real cancellation it can fill `cancelled=true` and a reason.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRunCancelOutcome {
    pub cancelled: bool,
    pub reason: String,
    pub run_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_status: Option<RunStatus>,
}

/// Aggregate scheduler health surfaced to the Operations cockpit. Lists
/// every workflow the scheduler refused to load or that carries a
/// broken trigger configuration so operators don't need to scrape logs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerHealth {
    pub scheduler_started: bool,
    pub scheduled_workflow_ids: Vec<Id>,
    pub warnings: Vec<SchedulerWarning>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerWarning {
    pub workflow_id: Id,
    pub workflow_name: String,
    pub kind: SchedulerWarningKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerWarningKind {
    InvalidCron,
    CronTriggerDisabled,
    ScheduledButDisabled,
    EnabledButNotScheduled,
}

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
    /// W50: user-set pause state. Defaults to `Active`. When `Paused`,
    /// the startup scheduler skips registration and `schedule_if_cron`
    /// honors the pause instead of re-registering. Independent of
    /// `is_enabled` so an operator can stop automatic ticks without
    /// losing the workflow definition.
    #[serde(default)]
    pub pause_state: SchedulePauseState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_paused_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_pause_reason: Option<String>,
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

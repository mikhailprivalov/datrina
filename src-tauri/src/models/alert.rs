use super::{Id, Timestamp};
use crate::models::chat::ChatMode;
use serde::{Deserialize, Serialize};

pub const ALERT_EVENT_CHANNEL: &str = "alert:event";

/// W21: a single alert definition attached to one widget. Stored
/// separately from the widget JSON (see `widget_alerts` table) so the
/// 10 widget variants don't each need to learn about alerts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WidgetAlert {
    pub id: Id,
    pub name: String,
    pub condition: AlertCondition,
    pub severity: AlertSeverity,
    /// Supports `{value}`, `{path}`, `{threshold}` placeholders rendered
    /// when the alert fires.
    pub message_template: String,
    #[serde(default = "default_cooldown")]
    pub cooldown_seconds: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Optional autonomous trigger. When present and the alert fires,
    /// a background chat session is spawned with the rendered prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_action: Option<AgentAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AlertCondition {
    Threshold {
        path: String,
        op: ThresholdOp,
        value: serde_json::Value,
    },
    PathPresent {
        path: String,
        expected: PresenceExpectation,
    },
    StatusEquals {
        path: String,
        status: String,
    },
    /// JMESPath-style boolean expression. v1 evaluates this as
    /// truthiness of `resolve_path(data, expr)`. Full JMESPath support
    /// is deferred.
    Custom {
        jmespath_expr: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThresholdOp {
    Gt,
    Lt,
    Gte,
    Lte,
    Eq,
    Neq,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PresenceExpectation {
    Present,
    Absent,
    Empty,
    NonEmpty,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

impl AlertSeverity {
    pub fn as_str(&self) -> &'static str {
        match self {
            AlertSeverity::Info => "info",
            AlertSeverity::Warning => "warning",
            AlertSeverity::Critical => "critical",
        }
    }

    pub fn from_str(s: &str) -> Option<AlertSeverity> {
        match s {
            "info" => Some(AlertSeverity::Info),
            "warning" => Some(AlertSeverity::Warning),
            "critical" => Some(AlertSeverity::Critical),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAction {
    pub mode: ChatMode,
    pub prompt_template: String,
    #[serde(default = "default_runs_per_day")]
    pub max_runs_per_day: u32,
    /// If false (default), spawned agent runs cannot Apply proposals —
    /// they can only suggest, leaving the user in the loop.
    #[serde(default)]
    pub allow_apply: bool,
    /// W22: per-spawn USD budget cap. Defaults to a conservative $0.10
    /// when omitted so an autonomous loop can't burn real money silently.
    #[serde(default = "default_autonomous_max_cost_usd")]
    pub max_cost_usd: f64,
}

fn default_autonomous_max_cost_usd() -> f64 {
    0.10
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertEvent {
    pub id: Id,
    pub widget_id: Id,
    pub dashboard_id: Id,
    pub alert_id: Id,
    pub fired_at: Timestamp,
    pub severity: AlertSeverity,
    pub message: String,
    /// `{ value, path, threshold }` — the resolved values at evaluation
    /// time so an inspector can show *why* it fired.
    pub context: serde_json::Value,
    pub acknowledged_at: Option<Timestamp>,
    /// Session id spawned by an autonomous trigger; populated only if
    /// the alert had an `agent_action` and the daily budget permitted a
    /// run.
    pub triggered_session_id: Option<Id>,
    /// W35: workflow run that produced the data this alert fired on.
    /// Set when the widget refresh emitted a fresh `WorkflowRun`;
    /// `None` for legacy rows and for ad-hoc evaluations that ran with
    /// no backing workflow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_id: Option<Id>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetWidgetAlertsRequest {
    pub dashboard_id: Id,
    pub widget_id: Id,
    pub alerts: Vec<WidgetAlert>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestAlertConditionRequest {
    pub condition: AlertCondition,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestAlertConditionResult {
    pub fired: bool,
    pub resolved_value: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

fn default_cooldown() -> u32 {
    600
}

fn default_runs_per_day() -> u32 {
    5
}

fn default_true() -> bool {
    true
}

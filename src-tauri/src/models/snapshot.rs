use super::{Id, Timestamp};
use serde::{Deserialize, Serialize};

/// W36: last-known-good rendered widget runtime data. Persisted by
/// `refresh_widget` after every successful refresh so the next
/// app/dashboard load can paint immediately while the live refresh path
/// runs in the background.
///
/// Snapshots are display-only. Alerts, autonomous triggers, and Build
/// chat continue to read from live `refresh_widget` output — a cached
/// snapshot is never used as evidence of datasource health.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WidgetRuntimeSnapshot {
    pub dashboard_id: Id,
    pub widget_id: Id,
    pub widget_kind: String,
    pub runtime_data: serde_json::Value,
    pub captured_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datasource_definition_id: Option<Id>,
    /// Fingerprint of the widget pieces that influence the *shape* of
    /// `runtime_data` (kind + datasource binding + tail pipeline). On
    /// load we recompute it from the current widget; a mismatch means
    /// the cached value is no longer safe to display and the snapshot
    /// is dropped.
    pub config_fingerprint: String,
    /// Fingerprint of the resolved dashboard parameter values that were
    /// in effect when the snapshot was captured. Parameter changes
    /// invalidate every snapshot in the dashboard so a stale dropdown
    /// value never paints over a fresh selection.
    pub parameter_fingerprint: String,
}

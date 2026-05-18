//! W42: typed Tauri events for widget-level refresh streaming.
//!
//! A widget refresh produces a sequence of envelopes on the
//! [`WIDGET_STREAM_EVENT_CHANNEL`] channel. Each envelope carries a
//! `refresh_run_id` so the UI can drop deltas from a superseded run and
//! a monotonic `sequence` for in-run ordering. Final widget data is
//! still committed via the existing `refresh_widget` return value — the
//! envelope is observational state only and never overwrites the
//! persisted [`WidgetRuntimeSnapshot`].
use super::{Id, Timestamp};
use serde::{Deserialize, Serialize};

pub const WIDGET_STREAM_EVENT_CHANNEL: &str = "widget:stream";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WidgetStreamEnvelope {
    pub dashboard_id: Id,
    pub widget_id: Id,
    pub refresh_run_id: Id,
    pub sequence: u32,
    pub kind: WidgetStreamKind,
    #[serde(flatten)]
    pub payload: WidgetStreamPayload,
    pub emitted_at: Timestamp,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WidgetStreamKind {
    /// Refresh has started; UI can show an in-progress chrome.
    RefreshStarted,
    /// Provider reasoning content arrived. `text` carries the delta;
    /// the UI accumulates it.
    ReasoningDelta,
    /// Streaming text delta from the LLM. Mutually-exclusive with
    /// `final` — `text` accumulates until `final` arrives.
    TextDelta,
    /// Non-streaming progress hint (e.g. provider does not support
    /// SSE so we are waiting for a single blocking response).
    Status,
    /// Final committed runtime data for the widget. UI should replace
    /// any partial state with this value.
    Final,
    /// Refresh failed. `partial_text` may carry any text that did
    /// stream before the failure — it is NOT committed as final
    /// runtime data, but the UI may show it as failed/partial.
    Failed,
    /// This refresh was superseded by a newer one. UI should drop any
    /// partial state for this run id without overwriting newer
    /// committed data.
    Superseded,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WidgetStreamPayload {
    /// Streaming text delta or reasoning delta.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Final runtime data shape (matches `WidgetRuntimeData` on the
    /// frontend) — only set on `Final`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_data: Option<serde_json::Value>,
    /// Partial text accumulated when the stream failed mid-flight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_text: Option<String>,
    /// Error message on `Failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Free-text status hint surfaced for non-streaming providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Optional workflow run id once the workflow body has settled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_id: Option<Id>,
}

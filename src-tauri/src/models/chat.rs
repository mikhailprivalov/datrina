use super::{Id, Timestamp};
use crate::models::dashboard::BuildProposal;
use crate::models::validation::ValidationIssue;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const CHAT_EVENT_CHANNEL: &str = "chat:event";

/// Lightweight session header for the sidebar list. Skips the full
/// messages array so we don't ship megabytes of conversation history
/// through the Tauri IPC every time the panel opens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSessionSummary {
    pub id: Id,
    pub mode: ChatMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dashboard_id: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub widget_id: Option<Id>,
    pub title: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub message_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSession {
    pub id: Id,
    pub mode: ChatMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dashboard_id: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub widget_id: Option<Id>,
    pub title: String,
    pub messages: Vec<ChatMessage>,
    /// W18: structured plan emitted by `submit_plan` once per Build session.
    /// Subsequent assistant turns continue advancing the same plan via
    /// `_plan_step` arguments on later tool calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_plan: Option<PlanArtifact>,
    /// W18: step_id -> current status. Persisted alongside the plan so
    /// continuations resume with accurate state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_status: Option<BTreeMap<String, PlanStepStatus>>,
    /// W22: running per-session token + cost totals. Updated transactionally
    /// after every persisted assistant message that came back with a parsed
    /// `usage` block. Footer + Costs view read directly from these.
    #[serde(default)]
    pub total_input_tokens: u64,
    #[serde(default)]
    pub total_output_tokens: u64,
    #[serde(default)]
    pub total_reasoning_tokens: u64,
    #[serde(default)]
    pub total_cost_usd: f64,
    /// W22: optional per-session budget cap in USD. When `total_cost_usd`
    /// would exceed this, the next provider request is denied with a
    /// `budget_exceeded` error. `None` == no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_usd: Option<f64>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanArtifact {
    pub summary: String,
    pub steps: Vec<PlanStep>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub title: String,
    pub kind: PlanStepKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rationale: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanStepKind {
    Explore,
    Fetch,
    Design,
    Test,
    Propose,
    Other,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanStepStatus {
    Pending,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: Id,
    pub role: MessageRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<ChatMessagePart>,
    pub mode: ChatMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_results: Option<Vec<ToolResult>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MessageMetadata>,
    pub timestamp: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatMessagePart {
    Text {
        text: String,
    },
    VisibleReasoning {
        text: String,
    },
    ProviderOpaqueReasoningState {
        state_id: Id,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    ToolCall {
        id: Id,
        name: String,
        arguments_preview: serde_json::Value,
        policy_decision: ToolPolicyDecision,
        status: ToolTraceStatus,
    },
    ToolResult {
        tool_call_id: Id,
        name: String,
        status: ToolTraceStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        result_preview: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    BuildProposal {
        proposal: BuildProposal,
    },
    Error {
        message: String,
        recoverable: bool,
    },
    Cancellation {
        reason: String,
    },
    /// W18: structured plan + live status. Persisted on the assistant
    /// message that owns the plan; UI renders it as a checklist above the
    /// message body.
    Plan {
        plan: PlanArtifact,
        status: BTreeMap<String, PlanStepStatus>,
    },
    /// W18: surfaced on the assistant message produced from a
    /// post-apply reflection turn so the UI can badge it as a suggestion
    /// rather than a fresh user-driven proposal.
    ReflectionMeta {
        widget_ids: Vec<Id>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatMode {
    Build,
    Context,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: Id,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: Id,
    pub name: String,
    pub result: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<TokenUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_proposal: Option<BuildProposal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// W22: resolved cost in USD for this single assistant turn. Computed
    /// at persist time from `tokens` and the pricing table so a later
    /// override edit doesn't silently rewrite history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// W22: token usage as parsed from the provider's `usage` chunk. Reasoning
/// tokens are tracked separately because o-series and a few OpenRouter
/// aliases bill them at a different rate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt: u32,
    pub completion: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<u32>,
}

/// W22: per-session entry returned by the top-sessions cost view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostSessionEntry {
    pub session_id: Id,
    pub title: String,
    pub mode: ChatMode,
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub updated_at: Timestamp,
}

// ─── Requests ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    pub mode: ChatMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dashboard_id: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub widget_id: Option<Id>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
}

// ─── Streaming Events ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatEventEnvelope {
    pub kind: ChatEventKind,
    pub session_id: Id,
    pub message_id: Id,
    pub sequence: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_event: Option<AgentEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_delta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_delta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<ToolCallTrace>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<ToolResultTrace>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_proposal: Option<BuildProposal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_message: Option<Box<ChatMessage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub synthetic: bool,
    pub emitted_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatEventKind {
    MessageStarted,
    ContentDelta,
    ReasoningDelta,
    ReasoningSnapshot,
    ToolCallRequested,
    ToolExecutionStarted,
    ToolResult,
    BuildProposalParsed,
    MessageCompleted,
    MessageFailed,
    MessageCancelled,
    AgentPhase,
    /// W16: proposal validation outcome. Always paired with
    /// `AgentEvent::ProposalValidationResult` so the UI can render the
    /// typed issue list.
    ProposalValidation,
    /// W18: emitted whenever the session-scoped plan or its step status
    /// map changes (initial `submit_plan`, each `_plan_step` transition,
    /// terminal cleanup).
    PlanUpdated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    RunStarted,
    RunFinished,
    RunError {
        message: String,
        recoverable: bool,
    },
    TextStart,
    TextDelta {
        text: String,
    },
    TextEnd,
    ReasoningStart,
    ReasoningDelta {
        text: String,
    },
    ReasoningEnd {
        text: String,
    },
    ToolCallStart {
        id: Id,
        name: String,
        arguments_preview: serde_json::Value,
        policy_decision: ToolPolicyDecision,
    },
    ToolCallEnd {
        id: Id,
        name: String,
        status: ToolTraceStatus,
    },
    ToolResult {
        tool_call_id: Id,
        name: String,
        status: ToolTraceStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        result_preview: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    BuildProposal {
        proposal: BuildProposal,
    },
    AbortCancel {
        reason: String,
    },
    RecoverableFailure {
        message: String,
    },
    AgentPhase {
        phase: AgentPhase,
        status: AgentPhaseStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// W16: structured validator result. `Started` carries an empty
    /// issues list; `Completed` carries the (now empty) issues list and
    /// signals the proposal is good; `Failed` carries the remaining
    /// issues after the retry budget was spent.
    ProposalValidationResult {
        status: AgentPhaseStatus,
        issues: Vec<ValidationIssue>,
        #[serde(default)]
        retried: bool,
    },
    /// W18: full plan snapshot + current step status map. Emitted on
    /// `submit_plan` and again whenever `_plan_step` flips a step's status.
    PlanUpdated {
        plan: PlanArtifact,
        status: BTreeMap<String, PlanStepStatus>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentPhase {
    McpReconnect,
    McpListTools {
        server_id: String,
    },
    ProviderRequest,
    ProviderFirstByte,
    ToolResume {
        iteration: u8,
    },
    /// W16: the agent called the same `(tool_name, arguments)` repeatedly
    /// inside one assistant run. Always emitted with `Failed`. The
    /// repeated tool call is short-circuited with a synthetic
    /// `loop_detected` tool result.
    LoopDetected {
        tool_name: String,
    },
    /// W16: proposal validator gate. `Started` fires before the validator
    /// runs. `Completed` fires when the proposal passes. `Failed` fires
    /// when issues remain after the retry budget. The structured issues
    /// themselves travel on the matching `AgentEvent::ProposalValidationResult`
    /// envelope, not here.
    ProposalValidation,
    /// W18: plan enforcement gate. `Started` fires when the agent's first
    /// tool call wasn't `submit_plan`. `Completed` when the agent
    /// submits a plan. `Failed` when the budget is spent without a plan.
    PlanEnforcement,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPhaseStatus {
    Started,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallTrace {
    pub id: Id,
    pub name: String,
    pub arguments_preview: serde_json::Value,
    pub policy_decision: ToolPolicyDecision,
    pub status: ToolTraceStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultTrace {
    pub tool_call_id: Id,
    pub name: String,
    pub status: ToolTraceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_preview: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPolicyDecision {
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolTraceStatus {
    Requested,
    Running,
    Success,
    Error,
}

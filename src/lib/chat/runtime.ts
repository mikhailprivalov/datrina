import type {
  AgentEvent,
  AgentPhase,
  AgentPhaseEntry,
  AgentPhaseStatus,
  BuildProposal,
  ChatEventEnvelope,
  ChatMessage,
  ChatMessagePart,
  PlanArtifact,
  PlanStepStatus,
  ToolCall,
  ToolCallTrace,
  ToolResult,
  ToolResultTrace,
  ValidationIssue,
} from '../api';

export type ChatRuntimeStatus = 'idle' | 'streaming' | 'complete' | 'failed' | 'cancelled';

export interface ChatRuntimeMessage {
  id: string;
  role: ChatMessage['role'];
  mode: ChatMessage['mode'];
  timestamp: number;
  status: ChatRuntimeStatus;
  synthetic: boolean;
  provider?: string;
  model?: string;
  tokens?: { prompt: number; completion: number };
  latency_ms?: number;
  parts: ChatMessagePart[];
}

export interface ChatRuntimeState {
  messages: ChatRuntimeMessage[];
  isLoading: boolean;
}

export function createChatRuntimeState(messages: ChatMessage[]): ChatRuntimeState {
  return {
    messages: messages.map(messageToRuntimeMessage),
    isLoading: false,
  };
}

export function appendUserRuntimeMessage(
  state: ChatRuntimeState,
  message: ChatMessage
): ChatRuntimeState {
  return {
    ...state,
    messages: [...state.messages, messageToRuntimeMessage(message)],
    isLoading: true,
  };
}

export function appendErrorRuntimeMessage(
  state: ChatRuntimeState,
  message: string,
  mode: ChatMessage['mode']
): ChatRuntimeState {
  return {
    ...state,
    isLoading: false,
    messages: [
      ...state.messages,
      {
        id: crypto.randomUUID(),
        role: 'assistant',
        mode,
        timestamp: Date.now(),
        status: 'failed',
        synthetic: false,
        parts: [{ type: 'error', message, recoverable: true }],
      },
    ],
  };
}

export function applyChatEvent(
  state: ChatRuntimeState,
  event: ChatEventEnvelope,
  mode: ChatMessage['mode']
): ChatRuntimeState {
  const base = ensureAssistantRuntimeMessage(state, event, mode);
  const agentEvent = event.agent_event ?? legacyAgentEvent(event);
  if (!agentEvent) return base;

  switch (agentEvent.type) {
    case 'run_started':
      return updateRuntimeMessage(base, event.message_id, message => ({
        ...message,
        status: 'streaming',
        synthetic: event.synthetic,
        provider: event.provider_id ?? message.provider,
        model: event.model ?? message.model,
      }), true);
    case 'text_delta':
      return updateRuntimeMessage(base, event.message_id, message => ({
        ...message,
        status: 'streaming',
        parts: appendTextPart(message.parts, agentEvent.text),
      }), true);
    case 'reasoning_delta':
      return updateRuntimeMessage(base, event.message_id, message => ({
        ...message,
        status: 'streaming',
        parts: appendReasoningPart(message.parts, agentEvent.text),
      }), true);
    case 'reasoning_end':
      return updateRuntimeMessage(base, event.message_id, message => ({
        ...message,
        parts: replaceReasoningPart(message.parts, agentEvent.text),
      }), true);
    case 'tool_call_start':
      return updateRuntimeMessage(base, event.message_id, message => ({
        ...message,
        status: 'streaming',
        parts: upsertToolCallPart(message.parts, {
          type: 'tool_call',
          id: agentEvent.id,
          name: agentEvent.name,
          arguments_preview: agentEvent.arguments_preview,
          policy_decision: agentEvent.policy_decision,
          status: 'requested',
        }),
      }), true);
    case 'tool_call_end':
      return updateRuntimeMessage(base, event.message_id, message => ({
        ...message,
        status: 'streaming',
        parts: upsertToolCallStatus(message.parts, agentEvent.id, agentEvent.status),
      }), true);
    case 'tool_result':
      return updateRuntimeMessage(base, event.message_id, message => ({
        ...message,
        status: 'streaming',
        parts: upsertToolResultPart(message.parts, {
          type: 'tool_result',
          tool_call_id: agentEvent.tool_call_id,
          name: agentEvent.name,
          status: agentEvent.status,
          result_preview: agentEvent.result_preview,
          error: agentEvent.error,
        }),
      }), true);
    case 'build_proposal':
      return updateRuntimeMessage(base, event.message_id, message => ({
        ...message,
        parts: upsertBuildProposalPart(message.parts, agentEvent.proposal),
      }), true);
    case 'run_finished':
      return event.final_message
        ? completeFromFinalMessage(base, event.final_message, event.synthetic)
        : updateRuntimeMessage(base, event.message_id, message => ({
            ...message,
            status: 'complete',
            synthetic: event.synthetic,
          }), false);
    case 'run_error':
      return markRuntimeFailure(base, event.message_id, agentEvent.message, agentEvent.recoverable, false);
    case 'recoverable_failure':
      return markRuntimeFailure(base, event.message_id, agentEvent.message, true, false);
    case 'abort_cancel':
      return updateRuntimeMessage(base, event.message_id, message => ({
        ...message,
        status: 'cancelled',
        parts: upsertTerminalPart(message.parts, { type: 'cancellation', reason: agentEvent.reason }),
      }), false);
    case 'agent_phase':
      return updateRuntimeMessage(base, event.message_id, message => ({
        ...message,
        parts: upsertAgentPhasePart(
          message.parts,
          agentEvent.phase,
          agentEvent.status,
          agentEvent.detail,
          event.emitted_at,
        ),
      }), true);
    case 'proposal_validation_result':
      return updateRuntimeMessage(base, event.message_id, message => ({
        ...message,
        parts: upsertProposalValidationPart(
          message.parts,
          agentEvent.status,
          agentEvent.issues,
          agentEvent.retried,
          event.emitted_at,
        ),
      }), agentEvent.status === 'started');
    case 'plan_updated':
      return updateRuntimeMessage(base, event.message_id, message => ({
        ...message,
        parts: upsertPlanPart(message.parts, agentEvent.plan, agentEvent.status),
      }), true);
    case 'text_start':
    case 'text_end':
    case 'reasoning_start':
      return base;
  }
}

export function messageText(message: ChatRuntimeMessage): string {
  return message.parts
    .filter((part): part is Extract<ChatMessagePart, { type: 'text' }> => part.type === 'text')
    .map(part => part.text)
    .join('');
}

function messageToRuntimeMessage(message: ChatMessage): ChatRuntimeMessage {
  return {
    id: message.id,
    role: message.role,
    mode: message.mode,
    timestamp: message.timestamp,
    status: 'complete',
    synthetic: false,
    provider: message.metadata?.provider,
    model: message.metadata?.model,
    tokens: message.metadata?.tokens,
    latency_ms: message.metadata?.latency_ms,
    parts: message.parts?.length ? message.parts : legacyMessageParts(message),
  };
}

function ensureAssistantRuntimeMessage(
  state: ChatRuntimeState,
  event: ChatEventEnvelope,
  mode: ChatMessage['mode']
): ChatRuntimeState {
  if (state.messages.some(message => message.id === event.message_id)) return state;
  return {
    ...state,
    isLoading: true,
    messages: [
      ...state.messages,
      {
        id: event.message_id,
        role: 'assistant',
        mode,
        timestamp: event.emitted_at,
        status: 'streaming',
        synthetic: event.synthetic,
        provider: event.provider_id,
        model: event.model,
        parts: [],
      },
    ],
  };
}

function completeFromFinalMessage(
  state: ChatRuntimeState,
  message: ChatMessage,
  synthetic: boolean
): ChatRuntimeState {
  return updateRuntimeMessage(state, message.id, item => ({
    ...messageToRuntimeMessage(message),
    synthetic,
    status: 'complete',
    parts: message.parts?.length ? message.parts : item.parts,
  }), false);
}

function markRuntimeFailure(
  state: ChatRuntimeState,
  messageId: string,
  message: string,
  recoverable: boolean,
  isLoading: boolean
): ChatRuntimeState {
  return updateRuntimeMessage(state, messageId, item => ({
    ...item,
    status: 'failed',
    parts: upsertTerminalPart(item.parts, { type: 'error', message, recoverable }),
  }), isLoading);
}

function updateRuntimeMessage(
  state: ChatRuntimeState,
  id: string,
  update: (message: ChatRuntimeMessage) => ChatRuntimeMessage,
  isLoading: boolean
): ChatRuntimeState {
  return {
    ...state,
    isLoading,
    messages: state.messages.map(message => message.id === id ? update(message) : message),
  };
}

function legacyAgentEvent(event: ChatEventEnvelope): AgentEvent | null {
  switch (event.kind) {
    case 'message_started':
      return { type: 'run_started' };
    case 'content_delta':
      return event.content_delta ? { type: 'text_delta', text: event.content_delta } : null;
    case 'reasoning_delta':
      return event.reasoning_delta ? { type: 'reasoning_delta', text: event.reasoning_delta } : null;
    case 'reasoning_snapshot':
      return { type: 'reasoning_end', text: event.reasoning ?? '' };
    case 'tool_call_requested':
      return event.tool_call ? toolCallTraceToAgentEvent(event.tool_call) : null;
    case 'tool_execution_started':
      return event.tool_call ? { type: 'tool_call_end', id: event.tool_call.id, name: event.tool_call.name, status: event.tool_call.status } : null;
    case 'tool_result':
      return event.tool_result ? toolResultTraceToAgentEvent(event.tool_result) : null;
    case 'build_proposal_parsed':
      return event.build_proposal ? { type: 'build_proposal', proposal: event.build_proposal } : null;
    case 'message_completed':
      return { type: 'run_finished' };
    case 'message_failed':
      return { type: 'run_error', message: event.error ?? 'Chat stream failed', recoverable: true };
    case 'message_cancelled':
      return { type: 'abort_cancel', reason: event.error ?? 'cancelled by user' };
    case 'agent_phase':
    case 'proposal_validation':
    case 'plan_updated':
      return null;
  }
}

function toolCallTraceToAgentEvent(trace: ToolCallTrace): AgentEvent {
  return {
    type: 'tool_call_start',
    id: trace.id,
    name: trace.name,
    arguments_preview: trace.arguments_preview,
    policy_decision: trace.policy_decision,
  };
}

function toolResultTraceToAgentEvent(trace: ToolResultTrace): AgentEvent {
  return {
    type: 'tool_result',
    tool_call_id: trace.tool_call_id,
    name: trace.name,
    status: trace.status,
    result_preview: trace.result_preview,
    error: trace.error,
  };
}

function legacyMessageParts(message: ChatMessage): ChatMessagePart[] {
  const parts: ChatMessagePart[] = [];
  if (message.content.trim()) parts.push({ type: 'text', text: message.content });
  if (message.metadata?.reasoning?.trim()) {
    parts.push({ type: 'visible_reasoning', text: message.metadata.reasoning });
  }
  for (const call of message.tool_calls ?? []) {
    parts.push(toolCallToPart(call, message.tool_results ?? []));
  }
  for (const result of message.tool_results ?? []) {
    parts.push(toolResultToPart(result));
  }
  if (message.metadata?.build_proposal) {
    parts.push({ type: 'build_proposal', proposal: message.metadata.build_proposal });
  }
  return parts;
}

function toolCallToPart(call: ToolCall, results: ToolResult[]): ChatMessagePart {
  const result = results.find(item => item.tool_call_id === call.id);
  return {
    type: 'tool_call',
    id: call.id,
    name: call.name,
    arguments_preview: maskForDisplay(call.arguments),
    policy_decision: 'accepted',
    status: result?.error ? 'error' : result ? 'success' : 'requested',
  };
}

function toolResultToPart(result: ToolResult): ChatMessagePart {
  return {
    type: 'tool_result',
    tool_call_id: result.tool_call_id,
    name: result.name,
    status: result.error ? 'error' : 'success',
    result_preview: maskForDisplay(result.result),
    error: result.error,
  };
}

function appendTextPart(parts: ChatMessagePart[], text: string): ChatMessagePart[] {
  if (!text) return parts;
  const next = [...parts];
  const last = next[next.length - 1];
  if (last?.type === 'text') {
    next[next.length - 1] = { ...last, text: `${last.text}${text}` };
    return next;
  }
  next.push({ type: 'text', text });
  return next;
}

function appendReasoningPart(parts: ChatMessagePart[], text: string): ChatMessagePart[] {
  if (!text) return parts;
  const next = [...parts];
  const last = next[next.length - 1];
  if (last?.type === 'visible_reasoning') {
    next[next.length - 1] = { ...last, text: `${last.text}${text}` };
    return next;
  }
  next.push({ type: 'visible_reasoning', text });
  return next;
}

function replaceReasoningPart(parts: ChatMessagePart[], text: string): ChatMessagePart[] {
  if (!text.trim()) return parts;
  const hasReasoning = parts.some(part => part.type === 'visible_reasoning');
  if (hasReasoning) return parts;
  return [...parts, { type: 'visible_reasoning', text }];
}

function upsertToolCallPart(parts: ChatMessagePart[], nextPart: Extract<ChatMessagePart, { type: 'tool_call' }>): ChatMessagePart[] {
  const index = parts.findIndex(part => part.type === 'tool_call' && part.id === nextPart.id);
  if (index === -1) return [...parts, nextPart];
  return parts.map((part, itemIndex) => itemIndex === index ? nextPart : part);
}

function upsertToolCallStatus(
  parts: ChatMessagePart[],
  id: string,
  status: Extract<ChatMessagePart, { type: 'tool_call' }>['status']
): ChatMessagePart[] {
  return parts.map(part => part.type === 'tool_call' && part.id === id ? { ...part, status } : part);
}

function upsertToolResultPart(parts: ChatMessagePart[], nextPart: Extract<ChatMessagePart, { type: 'tool_result' }>): ChatMessagePart[] {
  const index = parts.findIndex(part => part.type === 'tool_result' && part.tool_call_id === nextPart.tool_call_id);
  if (index === -1) return [...parts, nextPart];
  return parts.map((part, itemIndex) => itemIndex === index ? nextPart : part);
}

function upsertBuildProposalPart(parts: ChatMessagePart[], proposal: BuildProposal): ChatMessagePart[] {
  const nextPart: ChatMessagePart = { type: 'build_proposal', proposal };
  const index = parts.findIndex(part => part.type === 'build_proposal');
  if (index === -1) return [...parts, nextPart];
  return parts.map((part, itemIndex) => itemIndex === index ? nextPart : part);
}

function upsertTerminalPart(parts: ChatMessagePart[], nextPart: ChatMessagePart): ChatMessagePart[] {
  const index = parts.findIndex(part => part.type === nextPart.type);
  if (index === -1) return [...parts, nextPart];
  return parts.map((part, itemIndex) => itemIndex === index ? nextPart : part);
}

function agentPhaseKey(phase: AgentPhase): string {
  switch (phase.kind) {
    case 'mcp_reconnect':
      return 'mcp_reconnect';
    case 'mcp_list_tools':
      return `mcp_list_tools:${phase.server_id}`;
    case 'provider_request':
      return 'provider_request';
    case 'provider_first_byte':
      return 'provider_first_byte';
    case 'tool_resume':
      return `tool_resume:${phase.iteration}`;
    case 'loop_detected':
      return `loop_detected:${phase.tool_name}`;
    case 'proposal_validation':
      return 'proposal_validation';
    case 'plan_enforcement':
      return 'plan_enforcement';
  }
}

function upsertPlanPart(
  parts: ChatMessagePart[],
  plan: PlanArtifact,
  status: Record<string, PlanStepStatus>,
): ChatMessagePart[] {
  const nextPart: ChatMessagePart = { type: 'plan', plan, status };
  const index = parts.findIndex(part => part.type === 'plan');
  if (index === -1) {
    return [nextPart, ...parts];
  }
  return parts.map((part, itemIndex) => (itemIndex === index ? nextPart : part));
}

function upsertProposalValidationPart(
  parts: ChatMessagePart[],
  status: AgentPhaseStatus,
  issues: ValidationIssue[],
  retried: boolean,
  emittedAt: number,
): ChatMessagePart[] {
  const nextPart: ChatMessagePart = {
    type: 'proposal_validation',
    status,
    issues,
    retried,
    updated_at: emittedAt,
  };
  const index = parts.findIndex(part => part.type === 'proposal_validation');
  if (index === -1) return [...parts, nextPart];
  return parts.map((part, itemIndex) => itemIndex === index ? nextPart : part);
}

function upsertAgentPhasePart(
  parts: ChatMessagePart[],
  phase: AgentPhase,
  status: AgentPhaseStatus,
  detail: string | undefined,
  emittedAt: number,
): ChatMessagePart[] {
  const key = agentPhaseKey(phase);
  const existingIndex = parts.findIndex(part => part.type === 'agent_phase');
  const existingPart = existingIndex === -1 ? null : parts[existingIndex];
  const existingPhases = existingPart && existingPart.type === 'agent_phase' ? existingPart.phases : [];
  const entryIndex = existingPhases.findIndex(entry => entry.key === key);
  let nextPhases: AgentPhaseEntry[];
  if (entryIndex === -1) {
    nextPhases = [
      ...existingPhases,
      {
        key,
        phase,
        status,
        detail,
        started_at: emittedAt,
        finished_at: status === 'started' ? undefined : emittedAt,
      },
    ];
  } else {
    nextPhases = existingPhases.map((entry, index) => index === entryIndex
      ? {
          ...entry,
          phase,
          status,
          detail: detail ?? entry.detail,
          finished_at: status === 'started' ? entry.finished_at : emittedAt,
        }
      : entry);
  }
  const nextPart: ChatMessagePart = { type: 'agent_phase', phases: nextPhases };
  if (existingIndex === -1) {
    return [nextPart, ...parts];
  }
  return parts.map((part, index) => index === existingIndex ? nextPart : part);
}

function maskForDisplay(value: unknown, depth = 0): unknown {
  if (depth >= 5) return '...';
  if (Array.isArray(value)) return value.slice(0, 12).map(item => maskForDisplay(item, depth + 1));
  if (value && typeof value === 'object') {
    const entries = Object.entries(value as Record<string, unknown>).slice(0, 24);
    return Object.fromEntries(entries.map(([key, item]) => [
      key,
      isSecretKey(key) ? '***' : maskForDisplay(item, depth + 1),
    ]));
  }
  if (typeof value === 'string') {
    if (looksLikeSecret(value)) return '***';
    return value.length > 240 ? `${value.slice(0, 240)}...` : value;
  }
  return value;
}

function isSecretKey(key: string) {
  const normalized = key.toLowerCase();
  return ['authorization', 'api_key', 'apikey', 'token', 'secret', 'password', 'key']
    .some(part => normalized.includes(part));
}

function looksLikeSecret(value: string) {
  const trimmed = value.trim();
  return trimmed.startsWith('Bearer ')
    || trimmed.startsWith('sk-')
    || (trimmed.length >= 32 && /^[A-Za-z0-9_.=:-]+$/.test(trimmed));
}

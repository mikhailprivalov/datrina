/**
 * Tauri API wrapper — all communication with Rust backend
 */

import { invoke } from '@tauri-apps/api/core';

// ─── Types ───────────────────────────────────────────────────────────────────

export interface ApiResponse<T> {
  success: boolean;
  data: T | null;
  error: string | null;
}

export interface Dashboard {
  id: string;
  name: string;
  description?: string;
  layout: Widget[];
  workflows: Workflow[];
  is_default: boolean;
  created_at: number;
  updated_at: number;
}

export type WidgetType = 'chart' | 'text' | 'table' | 'image' | 'gauge';

export interface WidgetBase {
  id: string;
  title: string;
  x: number;
  y: number;
  w: number;
  h: number;
  datasource?: DatasourceConfig;
  refresh_interval?: number;
}

export interface ChartWidget extends WidgetBase {
  type: 'chart';
  config: ChartConfig;
}

export interface TextWidget extends WidgetBase {
  type: 'text';
  config: TextConfig;
}

export interface TableWidget extends WidgetBase {
  type: 'table';
  config: TableConfig;
}

export interface ImageWidget extends WidgetBase {
  type: 'image';
  config: ImageConfig;
}

export interface GaugeWidget extends WidgetBase {
  type: 'gauge';
  config: GaugeConfig;
}

export type Widget = ChartWidget | TextWidget | TableWidget | ImageWidget | GaugeWidget;

export type WidgetRuntimeData =
  | ChartWidgetRuntimeData
  | TextWidgetRuntimeData
  | TableWidgetRuntimeData
  | ImageWidgetRuntimeData
  | GaugeWidgetRuntimeData;

export interface ChartWidgetRuntimeData {
  kind: 'chart';
  rows: Record<string, string | number | null>[];
}

export interface TextWidgetRuntimeData {
  kind: 'text';
  content: string;
}

export interface TableWidgetRuntimeData {
  kind: 'table';
  rows: Record<string, string | number | boolean | null>[];
}

export interface ImageWidgetRuntimeData {
  kind: 'image';
  src: string;
  alt?: string;
}

export interface GaugeWidgetRuntimeData {
  kind: 'gauge';
  value: number;
}

export interface WidgetRefreshResult {
  status: 'ok' | 'unsupported' | 'not_implemented' | 'error' | string;
  workflow_run_id?: string;
  data?: WidgetRuntimeData;
  error?: string;
}

export interface WorkflowEventEnvelope {
  kind: 'run_started' | 'node_started' | 'node_finished' | 'run_finished';
  workflow_id: string;
  run_id: string;
  node_id?: string;
  status: 'idle' | 'running' | 'success' | 'error' | 'skipped';
  payload?: unknown;
  error?: string;
  emitted_at: number;
}

// W7 owns the final Tauri event names and envelope. W4 keeps this narrow
// frontend-side contract so widget rendering can accept pushed runtime data
// later without coupling components to raw Tauri events.
export interface WidgetDataEventEnvelope {
  kind: 'widget_data';
  dashboard_id: string;
  widget_id: string;
  data: WidgetRuntimeData;
  emitted_at: number;
}

export interface ChartConfig {
  kind: 'line' | 'bar' | 'area' | 'pie' | 'scatter';
  x_axis?: string;
  y_axis?: string[];
  colors?: string[];
  stacked?: boolean;
  show_legend?: boolean;
}

export interface TextConfig {
  format: 'markdown' | 'plain' | 'html';
  font_size?: number;
  color?: string;
  align?: 'left' | 'center' | 'right';
}

export interface TableColumn {
  key: string;
  header: string;
  width?: number;
  format?: 'text' | 'number' | 'date' | 'currency' | 'percent';
}

export interface TableConfig {
  columns: TableColumn[];
  page_size?: number;
  sortable?: boolean;
  filterable?: boolean;
}

export interface ImageConfig {
  fit: 'cover' | 'contain' | 'fill';
  border_radius?: number;
}

export interface GaugeThreshold {
  value: number;
  color: string;
  label?: string;
}

export interface GaugeConfig {
  min: number;
  max: number;
  unit?: string;
  thresholds?: GaugeThreshold[];
  show_value?: boolean;
}

export interface DatasourceConfig {
  workflow_id: string;
  output_key: string;
  post_process?: PostProcessStep[];
}

export interface PostProcessStep {
  kind: 'llm_analyze' | 'filter' | 'aggregate' | 'transform' | 'sort' | 'limit';
  config?: Record<string, unknown>;
}

export interface Workflow {
  id: string;
  name: string;
  description?: string;
  nodes: WorkflowNode[];
  edges: WorkflowEdge[];
  trigger: WorkflowTrigger;
  is_enabled: boolean;
  last_run?: WorkflowRun;
  created_at: number;
  updated_at: number;
}

export interface WorkflowNode {
  id: string;
  kind: 'mcp_tool' | 'llm' | 'transform' | 'datasource' | 'condition' | 'merge' | 'output';
  label: string;
  position?: { x: number; y: number };
  config?: Record<string, unknown>;
}

export interface WorkflowEdge {
  id: string;
  source: string;
  target: string;
  condition?: string;
}

export interface WorkflowTrigger {
  kind: 'cron' | 'event' | 'manual';
  config?: { cron?: string; event?: string };
}

export interface WorkflowRun {
  id: string;
  started_at: number;
  finished_at?: number;
  status: 'idle' | 'running' | 'success' | 'error' | 'skipped';
  node_results?: Record<string, unknown>;
  error?: string;
}

export interface ChatSession {
  id: string;
  mode: 'build' | 'context';
  dashboard_id?: string;
  widget_id?: string;
  title: string;
  messages: ChatMessage[];
  created_at: number;
  updated_at: number;
}

export interface ChatMessage {
  id: string;
  role: 'user' | 'assistant' | 'system' | 'tool';
  content: string;
  parts?: ChatMessagePart[];
  mode: 'build' | 'context';
  tool_calls?: ToolCall[];
  tool_results?: ToolResult[];
  metadata?: MessageMetadata;
  timestamp: number;
}

export type ChatMessagePart =
  | { type: 'text'; text: string }
  | { type: 'visible_reasoning'; text: string }
  | {
      type: 'provider_opaque_reasoning_state';
      state_id: string;
      provider_id?: string;
      model?: string;
    }
  | {
      type: 'tool_call';
      id: string;
      name: string;
      arguments_preview: unknown;
      policy_decision: 'accepted' | 'rejected';
      status: 'requested' | 'running' | 'success' | 'error';
    }
  | {
      type: 'tool_result';
      tool_call_id: string;
      name: string;
      status: 'requested' | 'running' | 'success' | 'error';
      result_preview?: unknown;
      error?: string;
    }
  | { type: 'build_proposal'; proposal: BuildProposal }
  | { type: 'error'; message: string; recoverable: boolean }
  | { type: 'cancellation'; reason: string };

export interface ToolCall {
  id: string;
  name: string;
  arguments: Record<string, unknown>;
}

export interface ToolResult {
  tool_call_id: string;
  name: string;
  result: unknown;
  error?: string;
}

export interface MessageMetadata {
  model?: string;
  provider?: string;
  tokens?: { prompt: number; completion: number };
  latency_ms?: number;
  build_proposal?: BuildProposal;
  reasoning?: string;
}

export type ChatEventKind =
  | 'message_started'
  | 'content_delta'
  | 'reasoning_delta'
  | 'reasoning_snapshot'
  | 'tool_call_requested'
  | 'tool_execution_started'
  | 'tool_result'
  | 'build_proposal_parsed'
  | 'message_completed'
  | 'message_failed'
  | 'message_cancelled';

export type AgentEvent =
  | { type: 'run_started' }
  | { type: 'run_finished' }
  | { type: 'run_error'; message: string; recoverable: boolean }
  | { type: 'text_start' }
  | { type: 'text_delta'; text: string }
  | { type: 'text_end' }
  | { type: 'reasoning_start' }
  | { type: 'reasoning_delta'; text: string }
  | { type: 'reasoning_end'; text: string }
  | {
      type: 'tool_call_start';
      id: string;
      name: string;
      arguments_preview: unknown;
      policy_decision: 'accepted' | 'rejected';
    }
  | {
      type: 'tool_call_end';
      id: string;
      name: string;
      status: 'requested' | 'running' | 'success' | 'error';
    }
  | {
      type: 'tool_result';
      tool_call_id: string;
      name: string;
      status: 'requested' | 'running' | 'success' | 'error';
      result_preview?: unknown;
      error?: string;
    }
  | { type: 'build_proposal'; proposal: BuildProposal }
  | { type: 'abort_cancel'; reason: string }
  | { type: 'recoverable_failure'; message: string };

export interface ChatEventEnvelope {
  kind: ChatEventKind;
  session_id: string;
  message_id: string;
  sequence: number;
  agent_event?: AgentEvent;
  provider_id?: string;
  model?: string;
  content_delta?: string;
  reasoning_delta?: string;
  reasoning?: string;
  tool_call?: ToolCallTrace;
  tool_result?: ToolResultTrace;
  build_proposal?: BuildProposal;
  final_message?: ChatMessage;
  error?: string;
  synthetic: boolean;
  emitted_at: number;
}

export interface ToolCallTrace {
  id: string;
  name: string;
  arguments_preview: unknown;
  policy_decision: 'accepted' | 'rejected';
  status: 'requested' | 'running' | 'success' | 'error';
}

export interface ToolResultTrace {
  tool_call_id: string;
  name: string;
  status: 'requested' | 'running' | 'success' | 'error';
  result_preview?: unknown;
  error?: string;
}

export interface BuildProposal {
  id: string;
  title: string;
  summary?: string;
  dashboard_name?: string;
  dashboard_description?: string;
  widgets: BuildWidgetProposal[];
}

export interface BuildWidgetProposal {
  widget_type: WidgetType;
  title: string;
  data?: unknown;
  datasource_plan?: BuildDatasourcePlan;
  config?: Record<string, unknown>;
  x?: number;
  y?: number;
  w?: number;
  h?: number;
}

export interface BuildDatasourcePlan {
  kind: 'builtin_tool' | 'mcp_tool' | 'provider_prompt';
  tool_name?: string;
  server_id?: string;
  arguments?: Record<string, unknown>;
  prompt?: string;
  output_path?: string;
  refresh_cron?: string;
}

export interface LLMProvider {
  id: string;
  name: string;
  kind: 'openrouter' | 'ollama' | 'custom' | 'local_mock';
  base_url: string;
  api_key?: string;
  default_model: string;
  models: string[];
  is_enabled: boolean;
}

export interface CreateProviderRequest {
  name: string;
  kind: LLMProvider['kind'];
  base_url: string;
  api_key?: string;
  default_model: string;
  models?: string[];
}

export interface UpdateProviderRequest {
  name?: string;
  kind?: LLMProvider['kind'];
  base_url?: string;
  api_key?: string;
  default_model?: string;
  models?: string[];
  is_enabled?: boolean;
}

export interface ProviderTestResult {
  status: 'ok' | 'invalid_config' | 'unavailable' | 'unsupported';
  provider_id: string;
  provider: string;
  model: string;
  error?: string;
  checked_at: number;
}

export interface MCPServer {
  id: string;
  name: string;
  transport: 'stdio' | 'http';
  is_enabled: boolean;
  command?: string;
  args?: string[];
  env?: Record<string, string>;
  url?: string;
}

export interface MCPTool {
  server_id: string;
  name: string;
  description: string;
  input_schema: Record<string, unknown>;
}

// ─── Helper ──────────────────────────────────────────────────────────────────

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const res = await invoke<ApiResponse<T>>(command, args);
  if (!res.success) {
    throw new Error(res.error || 'Unknown error');
  }
  if (res.data === null) {
    throw new Error(`Command "${command}" returned null data`);
  }
  return res.data;
}

async function callNullable<T>(command: string, args?: Record<string, unknown>): Promise<T | null> {
  const res = await invoke<ApiResponse<T>>(command, args);
  if (!res.success) {
    throw new Error(res.error || 'Unknown error');
  }
  return res.data;
}

async function callVoid(command: string, args?: Record<string, unknown>): Promise<void> {
  const res = await invoke<ApiResponse<null>>(command, args);
  if (!res.success) {
    throw new Error(res.error || 'Unknown error');
  }
}

// ─── Dashboard API ───────────────────────────────────────────────────────────

export const dashboardApi = {
  list: () => call<Dashboard[]>('list_dashboards'),
  get: (id: string) => call<Dashboard>('get_dashboard', { id }),
  create: (name: string, description?: string, template: 'blank' | 'local_mvp' = 'blank') =>
    call<Dashboard>('create_dashboard', { req: { name, description, template } }),
  update: (id: string, data: Partial<Dashboard>) =>
    call<Dashboard>('update_dashboard', { id, req: data }),
  addWidget: (
    dashboardId: string,
    widget: { widget_type: 'text' | 'gauge'; title: string; content?: string; value?: number }
  ) => call<Dashboard>('add_dashboard_widget', { dashboardId, req: widget }),
  applyBuildChange: (req: {
    action: 'create_local_dashboard' | 'add_text_widget' | 'add_gauge_widget';
    dashboard_id?: string;
    title?: string;
    content?: string;
    value?: number;
  }) => call<Dashboard>('apply_build_change', { req }),
  applyBuildProposal: (req: {
    proposal: BuildProposal;
    dashboard_id?: string;
    confirmed: boolean;
  }) => call<Dashboard>('apply_build_proposal', { req }),
  delete: (id: string) => call<boolean>('delete_dashboard', { id }),
  refreshWidget: (dashboardId: string, widgetId: string) =>
    call<WidgetRefreshResult>('refresh_widget', { dashboardId, widgetId }),
};

// ─── Chat API ────────────────────────────────────────────────────────────────

export const chatApi = {
  listSessions: () => call<ChatSession[]>('list_sessions'),
  getSession: (id: string) => call<ChatSession>('get_session', { id }),
  createSession: (mode: 'build' | 'context', dashboardId?: string) =>
    call<ChatSession>('create_session', { req: { mode, dashboard_id: dashboardId } }),
  sendMessage: (sessionId: string, content: string) =>
    call<ChatMessage>('send_message', { sessionId, req: { content } }),
  sendMessageStream: (sessionId: string, content: string) =>
    call<ChatMessage>('send_message_stream', { sessionId, req: { content } }),
  cancelResponse: (sessionId: string) =>
    call<boolean>('cancel_chat_response', { sessionId }),
  deleteSession: (id: string) => call<boolean>('delete_session', { id }),
};

// ─── MCP API ─────────────────────────────────────────────────────────────────

export const mcpApi = {
  listServers: () => call<MCPServer[]>('list_servers'),
  addServer: (server: MCPServer) => call<boolean>('add_server', { server }),
  removeServer: (id: string) => call<boolean>('remove_server', { id }),
  enableServer: (id: string) => call<MCPTool[]>('enable_server', { id }),
  reconnectEnabledServers: () => call<MCPTool[]>('reconnect_enabled_servers'),
  disableServer: (id: string) => call<boolean>('disable_server', { id }),
  listTools: () => call<MCPTool[]>('list_tools'),
  callTool: (serverId: string, toolName: string, args?: Record<string, unknown>) =>
    call<unknown>('call_tool', { req: { server_id: serverId, tool_name: toolName, arguments: args } }),
};

// ─── Provider API ────────────────────────────────────────────────────────────

export const providerApi = {
  list: () => call<LLMProvider[]>('list_providers'),
  add: (provider: CreateProviderRequest) => call<LLMProvider>('add_provider', { req: provider }),
  update: (id: string, provider: UpdateProviderRequest) =>
    call<LLMProvider>('update_provider', { id, req: provider }),
  setEnabled: (id: string, isEnabled: boolean) =>
    call<LLMProvider>('set_provider_enabled', { id, isEnabled }),
  remove: (id: string) => call<boolean>('remove_provider', { id }),
  test: (id: string) => call<ProviderTestResult>('test_provider', { id }),
};

// ─── Workflow API ────────────────────────────────────────────────────────────

export const workflowApi = {
  list: () => call<Workflow[]>('list_workflows'),
  get: (id: string) => call<Workflow>('get_workflow', { id }),
  execute: (id: string, input?: Record<string, unknown>) =>
    call<WorkflowRun>('execute_workflow', { id, input }),
  create: (workflow: Workflow) => call<boolean>('create_workflow', { workflow }),
  delete: (id: string) => call<boolean>('delete_workflow', { id }),
};

// ─── Tool API ────────────────────────────────────────────────────────────────

export const toolApi = {
  getWhitelist: () => call<string[]>('get_whitelist'),
  executeCurl: (args: string[]) => call<unknown>('execute_curl', { args }),
};

// ─── Config API ──────────────────────────────────────────────────────────────

export const configApi = {
  get: (key: string) => callNullable<string>('get_config', { key }),
  set: (key: string, value: string) => call<boolean>('set_config', { key, value }),
};

// ─── System API ──────────────────────────────────────────────────────────────

export const systemApi = {
  getAppInfo: () => call<Record<string, string>>('get_app_info'),
  openUrl: (url: string) => callVoid('open_url', { url }),
};

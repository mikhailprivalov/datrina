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
  /** W25: Grafana-style template variables. */
  parameters?: DashboardParameter[];
}

// ─── W25: Dashboard parameters ──────────────────────────────────────────────

export type ParameterValue =
  | string
  | number
  | boolean
  | { from: number; to: number }
  | ParameterValue[];

export interface ParameterOption {
  label: string;
  value: ParameterValue;
}

export type DashboardParameterKind =
  | { kind: 'static_list'; options: ParameterOption[] }
  | { kind: 'text_input'; placeholder?: string }
  | { kind: 'time_range'; default_preset?: string }
  | { kind: 'interval'; presets: string[] }
  | {
      kind: 'mcp_query';
      server_id: string;
      tool_name: string;
      arguments?: unknown;
      pipeline?: unknown[];
    }
  | {
      kind: 'http_query';
      method: string;
      url: string;
      headers?: unknown;
      body?: unknown;
      pipeline?: unknown[];
    }
  | { kind: 'constant'; value: ParameterValue };

export type DashboardParameter = {
  id: string;
  name: string;
  label: string;
  multi?: boolean;
  include_all?: boolean;
  default?: ParameterValue;
  depends_on?: string[];
  description?: string;
} & DashboardParameterKind;

export interface DashboardParameterState {
  parameter: DashboardParameter;
  value?: ParameterValue;
  options: ParameterOption[];
  options_error?: string;
}

export interface SetDashboardParameterResult {
  affected_widget_ids: string[];
}

export interface ResolveDashboardParametersResult {
  values: Record<string, ParameterValue>;
  cycle?: string[];
}

export type WidgetType = 'chart' | 'text' | 'table' | 'image' | 'gauge' | 'stat' | 'logs' | 'bar_gauge' | 'status_grid' | 'heatmap';

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

export interface StatWidget extends WidgetBase {
  type: 'stat';
  config: StatConfig;
}

export interface LogsWidget extends WidgetBase {
  type: 'logs';
  config: LogsConfig;
}

export interface BarGaugeWidget extends WidgetBase {
  type: 'bar_gauge';
  config: BarGaugeConfig;
}

export interface StatusGridWidget extends WidgetBase {
  type: 'status_grid';
  config: StatusGridConfig;
}

export interface HeatmapWidget extends WidgetBase {
  type: 'heatmap';
  config: HeatmapConfig;
}

export type Widget =
  | ChartWidget
  | TextWidget
  | TableWidget
  | ImageWidget
  | GaugeWidget
  | StatWidget
  | LogsWidget
  | BarGaugeWidget
  | StatusGridWidget
  | HeatmapWidget;

export type WidgetRuntimeData =
  | ChartWidgetRuntimeData
  | TextWidgetRuntimeData
  | TableWidgetRuntimeData
  | ImageWidgetRuntimeData
  | GaugeWidgetRuntimeData
  | StatWidgetRuntimeData
  | LogsWidgetRuntimeData
  | BarGaugeWidgetRuntimeData
  | StatusGridWidgetRuntimeData
  | HeatmapWidgetRuntimeData;

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
  rows: Record<string, unknown>[];
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

export interface StatWidgetRuntimeData {
  kind: 'stat';
  value: number | string;
  delta?: number | string | null;
  label?: string | null;
  sparkline?: Array<{ t?: string | number; v: number } | number> | null;
}

export interface LogEntry {
  ts?: string | number;
  level?: string;
  message?: string;
  source?: string;
  [extra: string]: unknown;
}

export interface LogsWidgetRuntimeData {
  kind: 'logs';
  entries: LogEntry[];
}

export interface BarGaugeRow {
  name: string;
  value: number;
  max?: number;
}

export interface BarGaugeWidgetRuntimeData {
  kind: 'bar_gauge';
  rows: BarGaugeRow[];
}

export interface StatusGridItem {
  name: string;
  status: string;
  detail?: string | number | null;
}

export interface StatusGridWidgetRuntimeData {
  kind: 'status_grid';
  items: StatusGridItem[];
}

export interface HeatmapCell {
  x: number | string;
  y: number | string;
  value: number;
}

export interface HeatmapWidgetRuntimeData {
  kind: 'heatmap';
  cells: HeatmapCell[];
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
  format?: 'markdown' | 'plain' | 'html';
  font_size?: number;
  color?: string;
  align?: 'left' | 'center' | 'right';
}

export type TableColumnFormat =
  | 'text'
  | 'number'
  | 'date'
  | 'currency'
  | 'percent'
  | 'status'
  | 'progress'
  | 'badge'
  | 'link'
  | 'sparkline';

export interface TableColumn {
  key: string;
  header: string;
  width?: number;
  format?: TableColumnFormat;
  thresholds?: GaugeThreshold[];
  status_colors?: Record<string, string>;
  link_template?: string;
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

export interface StatConfig {
  unit?: string;
  prefix?: string;
  suffix?: string;
  decimals?: number;
  color_mode?: 'none' | 'value' | 'background';
  thresholds?: GaugeThreshold[];
  show_sparkline?: boolean;
  graph_mode?: 'none' | 'sparkline';
  align?: 'left' | 'center' | 'right';
}

export interface LogsConfig {
  max_entries?: number;
  show_timestamp?: boolean;
  show_level?: boolean;
  wrap?: boolean;
  reverse?: boolean;
}

export interface BarGaugeConfig {
  orientation?: 'horizontal' | 'vertical';
  display_mode?: 'gradient' | 'basic' | 'retro';
  show_value?: boolean;
  min?: number;
  max?: number;
  unit?: string;
  thresholds?: GaugeThreshold[];
}

export interface StatusGridConfig {
  columns?: number;
  layout?: 'grid' | 'row' | 'compact';
  show_label?: boolean;
  status_colors?: Record<string, string>;
}

export interface HeatmapConfig {
  color_scheme?: 'viridis' | 'magma' | 'cool' | 'warm' | 'green_red';
  x_label?: string;
  y_label?: string;
  unit?: string;
  show_legend?: boolean;
  log_scale?: boolean;
}

export interface DatasourceConfig {
  workflow_id: string;
  output_key: string;
  post_process?: PostProcessStep[];
  /** W23: opt-in pipeline trace capture on every refresh. */
  capture_traces?: boolean;
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
  current_plan?: PlanArtifact | null;
  plan_status?: Record<string, PlanStepStatus> | null;
  /** W22: running per-session token + cost totals. */
  total_input_tokens?: number;
  total_output_tokens?: number;
  total_reasoning_tokens?: number;
  total_cost_usd?: number;
  /** W22: optional per-session USD budget cap. Null/undefined == no cap. */
  max_cost_usd?: number | null;
  created_at: number;
  updated_at: number;
}

export type PlanStepKind = 'explore' | 'fetch' | 'design' | 'test' | 'propose' | 'other';
export type PlanStepStatus = 'pending' | 'running' | 'done' | 'failed';

export interface PlanStep {
  id: string;
  title: string;
  kind: PlanStepKind;
  depends_on?: string[];
  rationale?: string;
}

export interface PlanArtifact {
  summary: string;
  steps: PlanStep[];
  created_at: number;
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
  | { type: 'cancellation'; reason: string }
  | { type: 'agent_phase'; phases: AgentPhaseEntry[] }
  | {
      type: 'proposal_validation';
      status: AgentPhaseStatus;
      issues: ValidationIssue[];
      retried: boolean;
      updated_at: number;
    }
  | {
      type: 'plan';
      plan: PlanArtifact;
      status: Record<string, PlanStepStatus>;
    }
  | { type: 'reflection_meta'; widget_ids: string[] };

export interface AgentPhaseEntry {
  key: string;
  phase: AgentPhase;
  status: AgentPhaseStatus;
  detail?: string;
  started_at: number;
  finished_at?: number;
}

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
  tokens?: { prompt: number; completion: number; reasoning?: number };
  latency_ms?: number;
  build_proposal?: BuildProposal;
  reasoning?: string;
  /** W22: resolved USD cost for this single assistant turn. */
  cost_usd?: number;
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
  | 'message_cancelled'
  | 'agent_phase'
  | 'proposal_validation'
  | 'plan_updated';

export type AgentPhaseStatus = 'started' | 'completed' | 'failed';

export type AgentPhase =
  | { kind: 'mcp_reconnect' }
  | { kind: 'mcp_list_tools'; server_id: string }
  | { kind: 'provider_request' }
  | { kind: 'provider_first_byte' }
  | { kind: 'tool_resume'; iteration: number }
  | { kind: 'loop_detected'; tool_name: string }
  | { kind: 'proposal_validation' }
  | { kind: 'plan_enforcement' };

export type ValidationIssue =
  | {
      kind: 'missing_datasource_plan';
      widget_index: number;
      widget_title: string;
    }
  | {
      kind: 'unknown_replace_widget_id';
      widget_index: number;
      widget_title: string;
      replace_widget_id: string;
    }
  | {
      kind: 'unknown_source_key';
      widget_index: number;
      widget_title: string;
      source_key: string;
    }
  | {
      kind: 'hardcoded_literal_value';
      widget_index: number;
      widget_title: string;
      path: string;
    }
  | {
      kind: 'text_widget_contains_raw_json';
      widget_index: number;
      widget_title: string;
    }
  | {
      kind: 'missing_dry_run_evidence';
      widget_index: number;
      widget_title: string;
      widget_kind: string;
    }
  | {
      kind: 'pipeline_schema_invalid';
      widget_index: number;
      widget_title: string;
      error: string;
    }
  | { kind: 'duplicate_shared_key'; key: string };

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
  | { type: 'recoverable_failure'; message: string }
  | {
      type: 'agent_phase';
      phase: AgentPhase;
      status: AgentPhaseStatus;
      detail?: string;
    }
  | {
      type: 'proposal_validation_result';
      status: AgentPhaseStatus;
      issues: ValidationIssue[];
      retried: boolean;
    }
  | {
      type: 'plan_updated';
      plan: PlanArtifact;
      status: Record<string, PlanStepStatus>;
    };

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

export interface WidgetDryRunResult {
  status: 'ok' | 'error';
  widget_runtime?: WidgetRuntimeData | null;
  raw_output?: unknown;
  error?: string;
  duration_ms: number;
  pipeline_steps: number;
  has_llm_step: boolean;
  workflow_node_ids: string[];
}

export interface BuildProposal {
  id: string;
  title: string;
  summary?: string;
  dashboard_name?: string;
  dashboard_description?: string;
  widgets: BuildWidgetProposal[];
  remove_widget_ids?: string[];
  shared_datasources?: SharedDatasource[];
  /** W25: dashboard-level template variables proposed alongside widgets. */
  parameters?: DashboardParameter[];
}

export interface SharedDatasource {
  key: string;
  kind: 'builtin_tool' | 'mcp_tool' | 'provider_prompt';
  tool_name?: string;
  server_id?: string;
  arguments?: Record<string, unknown>;
  prompt?: string;
  pipeline?: PipelineStep[];
  refresh_cron?: string;
  label?: string;
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
  replace_widget_id?: string;
}

export interface BuildDatasourcePlan {
  kind: 'builtin_tool' | 'mcp_tool' | 'provider_prompt' | 'shared';
  tool_name?: string;
  server_id?: string;
  arguments?: Record<string, unknown>;
  prompt?: string;
  output_path?: string;
  refresh_cron?: string;
  pipeline?: PipelineStep[];
  source_key?: string;
}

export type FilterOp =
  | 'eq' | 'ne'
  | 'gt' | 'gte' | 'lt' | 'lte'
  | 'contains' | 'starts_with' | 'ends_with'
  | 'in' | 'not_in'
  | 'exists' | 'not_exists'
  | 'truthy' | 'falsy';

export type SortOrder = 'asc' | 'desc';

export type AggregateMetric =
  | { kind: 'count' }
  | { kind: 'sum'; field: string }
  | { kind: 'avg'; field: string }
  | { kind: 'min'; field: string }
  | { kind: 'max'; field: string }
  | { kind: 'first'; field: string }
  | { kind: 'last'; field: string };

export type CoerceTarget = 'number' | 'integer' | 'string' | 'array';

export type PipelineStep =
  | { kind: 'pick'; path: string }
  | { kind: 'filter'; field: string; op?: FilterOp; value?: unknown }
  | { kind: 'sort'; by: string; order?: SortOrder }
  | { kind: 'limit'; count: number }
  | { kind: 'map'; fields?: string[]; rename?: Record<string, string> }
  | { kind: 'aggregate'; group_by?: string; metric: AggregateMetric; output_key?: string }
  | { kind: 'set'; field: string; value: unknown }
  | { kind: 'head' }
  | { kind: 'tail' }
  | { kind: 'length' }
  | { kind: 'flatten' }
  | { kind: 'unique'; by?: string }
  | { kind: 'format'; template: string; output_key?: string }
  | { kind: 'coerce'; to: CoerceTarget }
  | { kind: 'llm_postprocess'; prompt: string; expect?: 'text' | 'json' };

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

export type DashboardVersionSource = 'agent_apply' | 'manual_edit' | 'restore' | 'pre_delete';

export interface DashboardVersionSummary {
  id: string;
  dashboard_id: string;
  applied_at: number;
  source: DashboardVersionSource;
  summary: string;
  widget_count: number;
  source_session_id?: string;
  parent_version_id?: string;
}

export interface DashboardVersion extends DashboardVersionSummary {
  snapshot: Dashboard;
}

export interface JsonPathChange {
  path: string;
  before: unknown;
  after: unknown;
}

export interface WidgetSummary {
  id: string;
  title: string;
  kind: string;
}

export interface WidgetDiff {
  widget_id: string;
  widget_title: string;
  kind_changed?: [string, string];
  title_changed?: [string, string];
  config_changes: JsonPathChange[];
  datasource_plan_changed: boolean;
}

export interface DashboardDiff {
  from_version_id: string;
  to_version_id: string;
  added_widgets: WidgetSummary[];
  removed_widgets: WidgetSummary[];
  modified_widgets: WidgetDiff[];
  name_changed?: [string, string];
  description_changed?: [string | null, string | null];
  layout_changed: boolean;
}

export const dashboardApi = {
  list: () => call<Dashboard[]>('list_dashboards'),
  get: (id: string) => call<Dashboard>('get_dashboard', { id }),
  create: (name: string, description?: string, template: 'blank' | 'local_mvp' = 'blank') => {
    const safeTemplate: 'blank' | 'local_mvp' = template === 'local_mvp' ? 'local_mvp' : 'blank';
    return call<Dashboard>('create_dashboard', { req: { name, description, template: safeTemplate } });
  },
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
    session_id?: string;
  }) => call<Dashboard>('apply_build_proposal', { req }),
  dryRunWidget: (proposal: BuildWidgetProposal, sharedDatasources?: SharedDatasource[]) =>
    call<WidgetDryRunResult>('dry_run_widget', { proposal, sharedDatasources }),
  delete: (id: string) => call<boolean>('delete_dashboard', { id }),
  refreshWidget: (dashboardId: string, widgetId: string) =>
    call<WidgetRefreshResult>('refresh_widget', { dashboardId, widgetId }),
  listVersions: (dashboardId: string) =>
    call<DashboardVersionSummary[]>('list_dashboard_versions', { dashboardId }),
  getVersion: (versionId: string) =>
    call<DashboardVersion>('get_dashboard_version', { versionId }),
  diffVersions: (fromId: string, toId: string) =>
    call<DashboardDiff>('diff_dashboard_versions', { fromId, toId }),
  restoreVersion: (versionId: string) =>
    call<Dashboard>('restore_dashboard_version', { versionId }),
  // W25: parameters
  listParameters: (dashboardId: string) =>
    call<DashboardParameterState[]>('list_dashboard_parameters', { dashboardId }),
  getParameterValues: (dashboardId: string) =>
    call<Record<string, ParameterValue>>('get_dashboard_parameter_values', { dashboardId }),
  setParameterValue: (dashboardId: string, paramName: string, value: ParameterValue) =>
    call<SetDashboardParameterResult>('set_dashboard_parameter_value', {
      dashboardId,
      paramName,
      value,
    }),
  resolveParameters: (dashboardId: string) =>
    call<ResolveDashboardParametersResult>('resolve_dashboard_parameters', { dashboardId }),
};

// ─── W23: Pipeline Debug API ────────────────────────────────────────────────

export interface SizeHint {
  items?: number;
  bytes?: number;
}

export type SampleKind = 'value' | 'array_head' | 'object' | 'null' | 'truncated_string';

export interface SampleValue {
  kind: SampleKind;
  size_hint: SizeHint;
  preview: unknown;
}

export interface PipelineStepTrace {
  index: number;
  kind: string;
  config_json: unknown;
  input_sample: SampleValue;
  output_sample: SampleValue;
  duration_ms: number;
  error?: string;
}

export type SourceSummary =
  | { kind: 'mcp_tool'; server_id: string; tool_name: string; arguments?: unknown }
  | { kind: 'builtin_tool'; tool_name: string; arguments?: unknown }
  | { kind: 'provider_prompt'; prompt: string }
  | { kind: 'unknown' };

export interface PipelineTrace {
  workflow_id: string;
  widget_id: string;
  started_at: number;
  finished_at: number;
  source_summary: SourceSummary;
  steps: PipelineStepTrace[];
  final_value?: unknown;
  error?: string;
}

export interface TraceEntry {
  captured_at: number;
  trace: PipelineTrace;
}

export const debugApi = {
  traceWidget: (dashboardId: string, widgetId: string) =>
    call<PipelineTrace>('trace_widget_pipeline', { dashboardId, widgetId }),
  listTraces: (widgetId: string) =>
    call<TraceEntry[]>('list_widget_traces', { widgetId }),
  getTrace: (widgetId: string, capturedAt: number) =>
    call<PipelineTrace | null>('get_widget_trace', { widgetId, capturedAt }),
  setCaptureTraces: (dashboardId: string, widgetId: string, capture: boolean) =>
    call<boolean>('set_widget_capture_traces', { dashboardId, widgetId, capture }),
};

// ─── Chat API ────────────────────────────────────────────────────────────────

export interface ChatSessionSummary {
  id: string;
  mode: 'build' | 'context';
  dashboard_id?: string;
  widget_id?: string;
  title: string;
  created_at: number;
  updated_at: number;
  message_count: number;
  preview?: string;
}

export const chatApi = {
  listSessions: () => call<ChatSession[]>('list_sessions'),
  listSessionSummaries: () => call<ChatSessionSummary[]>('list_session_summaries'),
  getSession: (id: string) => call<ChatSession>('get_session', { id }),
  createSession: (mode: 'build' | 'context', dashboardId?: string) =>
    call<ChatSession>('create_session', { req: { mode, dashboard_id: dashboardId } }),
  sendMessage: (sessionId: string, content: string) =>
    call<ChatMessage>('send_message', { sessionId, req: { content } }),
  sendMessageStream: (sessionId: string, content: string) =>
    call<ChatMessage>('send_message_stream', { sessionId, req: { content } }),
  cancelResponse: (sessionId: string) =>
    call<boolean>('cancel_chat_response', { sessionId }),
  truncateMessages: (sessionId: string, beforeMessageId: string) =>
    call<ChatSession>('truncate_chat_messages', { sessionId, beforeMessageId }),
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

export interface HttpRequestArgs {
  method: string;
  url: string;
  body?: unknown;
  headers?: Record<string, string>;
}

export interface HttpRequestResponse {
  status: number;
  body: unknown;
}

export const toolApi = {
  getWhitelist: () => call<string[]>('get_whitelist'),
  executeCurl: (args: string[]) => call<unknown>('execute_curl', { args }),
  executeHttpRequest: (req: HttpRequestArgs) =>
    call<HttpRequestResponse>('execute_http_request', { req }),
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

// ─── Memory API (W17) ────────────────────────────────────────────────────────

export type MemoryKind = 'fact' | 'preference' | 'tool_shape' | 'lesson';

export type MemoryScope =
  | { kind: 'global' }
  | { kind: 'dashboard'; id: string }
  | { kind: 'mcp_server'; id: string }
  | { kind: 'session'; id: string };

export interface MemoryRecord {
  id: string;
  scope: MemoryScope;
  kind: MemoryKind;
  content: string;
  metadata?: unknown;
  created_at: number;
  accessed_count: number;
  last_accessed_at?: number | null;
  expires_at?: number | null;
  compressed_into?: string | null;
}

export interface MemoryHit {
  record: MemoryRecord;
  score: number;
}

export interface ToolShape {
  id: string;
  server_id: string;
  tool_name: string;
  args_fingerprint: string;
  shape_summary: string;
  shape_full: string;
  sample_path?: string | null;
  observed_at: number;
  observation_count: number;
}

export interface RememberMemoryRequest {
  scope: MemoryScope;
  kind: MemoryKind;
  content: string;
  metadata?: unknown;
}

export interface RecallMemoryRequest {
  query: string;
  dashboard_id?: string;
  mcp_server_ids?: string[];
  session_id?: string;
  top_n?: number;
}

export const memoryApi = {
  list: () => call<MemoryRecord[]>('list_memories'),
  remove: (id: string) => call<boolean>('delete_memory', { id }),
  remember: (req: RememberMemoryRequest) => call<MemoryRecord>('remember_memory', { req }),
  recall: (req: RecallMemoryRequest) => call<MemoryHit[]>('recall_memories', { req }),
  listToolShapes: (serverId: string) =>
    call<ToolShape[]>('list_tool_shapes', { serverId }),
  listKinds: () => call<MemoryKind[]>('list_memory_kinds'),
};

// ─── Playground API (W20) ────────────────────────────────────────────────────

export type PlaygroundToolKind = 'mcp' | 'http';

export interface PlaygroundPreset {
  id: string;
  tool_kind: PlaygroundToolKind;
  server_id?: string;
  tool_name: string;
  display_name: string;
  arguments: unknown;
  created_at: number;
  updated_at: number;
}

export interface SavePlaygroundPresetRequest {
  tool_kind: PlaygroundToolKind;
  server_id?: string;
  tool_name: string;
  display_name: string;
  arguments: unknown;
}

export const playgroundApi = {
  listPresets: () => call<PlaygroundPreset[]>('list_playground_presets'),
  savePreset: (req: SavePlaygroundPresetRequest) =>
    call<PlaygroundPreset>('save_playground_preset', { req }),
  deletePreset: (id: string) => call<boolean>('delete_playground_preset', { id }),
};

// ─── Alert API (W21) ─────────────────────────────────────────────────────────

export const ALERT_EVENT_CHANNEL = 'alert:event';

export type AlertSeverity = 'info' | 'warning' | 'critical';

export type ThresholdOp = 'gt' | 'lt' | 'gte' | 'lte' | 'eq' | 'neq';

export type PresenceExpectation = 'present' | 'absent' | 'empty' | 'non_empty';

export type AlertCondition =
  | { kind: 'threshold'; path: string; op: ThresholdOp; value: unknown }
  | { kind: 'path_present'; path: string; expected: PresenceExpectation }
  | { kind: 'status_equals'; path: string; status: string }
  | { kind: 'custom'; jmespath_expr: string };

export interface AgentAction {
  mode: 'build' | 'context';
  prompt_template: string;
  max_runs_per_day: number;
  allow_apply: boolean;
  /** W22: per-autonomous-spawn USD budget cap. Defaults to 0.10 server-side. */
  max_cost_usd?: number;
}

export interface WidgetAlert {
  id: string;
  name: string;
  condition: AlertCondition;
  severity: AlertSeverity;
  message_template: string;
  cooldown_seconds: number;
  enabled: boolean;
  agent_action?: AgentAction;
}

export interface AlertEvent {
  id: string;
  widget_id: string;
  dashboard_id: string;
  alert_id: string;
  fired_at: number;
  severity: AlertSeverity;
  message: string;
  context: { value?: unknown; path?: string; threshold?: unknown };
  acknowledged_at?: number | null;
  triggered_session_id?: string | null;
}

export interface SetWidgetAlertsRequest {
  dashboard_id: string;
  widget_id: string;
  alerts: WidgetAlert[];
}

export interface TestAlertConditionResult {
  fired: boolean;
  resolved_value: unknown;
  reason?: string | null;
}

export const alertApi = {
  listEvents: (onlyUnacknowledged = false, limit = 200) =>
    call<AlertEvent[]>('list_alert_events', { onlyUnacknowledged, limit }),
  acknowledge: (eventId: string) =>
    call<boolean>('acknowledge_alert', { eventId }),
  getForWidget: (widgetId: string) =>
    call<WidgetAlert[]>('get_widget_alerts', { widgetId }),
  setForWidget: (req: SetWidgetAlertsRequest) =>
    call<WidgetAlert[]>('set_widget_alerts', { req }),
  countUnacknowledged: () => call<number>('count_unacknowledged_alerts'),
  testCondition: (condition: AlertCondition, data: unknown) =>
    call<TestAlertConditionResult>('test_alert_condition', { req: { condition, data } }),
};

// ─── Cost API (W22) ──────────────────────────────────────────────────────────

export interface SessionCostSnapshot {
  session_id: string;
  model?: string;
  input_tokens: number;
  output_tokens: number;
  reasoning_tokens: number;
  cost_usd: number;
  max_cost_usd?: number | null;
  today_cost_usd: number;
}

export interface DailyCostBucket {
  day_start_ms: number;
  cost_usd: number;
}

export interface CostSessionEntry {
  session_id: string;
  title: string;
  mode: 'build' | 'context';
  cost_usd: number;
  input_tokens: number;
  output_tokens: number;
  reasoning_tokens: number;
  updated_at: number;
}

export interface CostSummary {
  today_cost_usd: number;
  last_30_days: DailyCostBucket[];
  top_sessions: CostSessionEntry[];
}

export interface ModelPricingOverride {
  model_pattern: string;
  provider_kind?: 'openrouter' | 'ollama' | 'custom' | 'local_mock';
  input_usd_per_1m: number;
  output_usd_per_1m: number;
  reasoning_usd_per_1m?: number;
}

export const costApi = {
  getSessionSnapshot: (sessionId: string) =>
    call<SessionCostSnapshot>('get_session_cost_snapshot', { sessionId }),
  getSummary: (days?: number) => call<CostSummary>('get_cost_summary', { days }),
  setSessionBudget: (sessionId: string, maxCostUsd: number | null) =>
    call<ChatSession>('set_session_budget', {
      req: { session_id: sessionId, max_cost_usd: maxCostUsd },
    }),
  getPricingOverrides: () =>
    call<ModelPricingOverride[]>('get_pricing_overrides'),
  setPricingOverrides: (overrides: ModelPricingOverride[]) =>
    call<ModelPricingOverride[]>('set_pricing_overrides', { req: { overrides } }),
};

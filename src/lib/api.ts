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
  /** W43: dashboard-level default LLM policy for LLM-backed widgets. */
  model_policy?: DashboardModelPolicy | null;
  /** W47: dashboard-level assistant language policy override. */
  language_policy?: AssistantLanguagePolicy | null;
}

// ─── W47: Assistant language policy ─────────────────────────────────────────

/** Per-scope language policy. `auto` follows the user's natural language;
 *  `explicit` pins a curated BCP-47 tag from `listAssistantLanguages`. */
export type AssistantLanguagePolicy =
  | { mode: 'auto' }
  | { mode: 'explicit'; tag: string };

export type AssistantLanguageDirection = 'ltr' | 'rtl';

export type AssistantLanguageSource =
  | 'auto'
  | 'app_default'
  | 'dashboard_override'
  | 'session_override';

export interface AssistantLanguageProviderSupport {
  provider: string;
  prompt_supported: boolean;
  notes?: string | null;
}

export interface AssistantLanguageOption {
  tag: string;
  label: string;
  native_label: string;
  direction: AssistantLanguageDirection;
  prompt_name: string;
  provider_support: AssistantLanguageProviderSupport[];
}

export interface AssistantLanguageCatalog {
  options: AssistantLanguageOption[];
}

export interface EffectiveAssistantLanguage {
  source: AssistantLanguageSource;
  option?: AssistantLanguageOption | null;
}

/** W43: capability tag pinned by dashboard/widget model policy. */
export type WidgetCapability =
  | 'structured_json_object'
  | 'streaming'
  | 'tool_calling';

/** W43: which surface chose the resolved provider/model. */
export type WidgetModelSource =
  | 'widget_override'
  | 'dashboard_default'
  | 'app_active_provider';

export interface DashboardModelPolicy {
  provider_id: string;
  model: string;
  required_caps?: WidgetCapability[];
}

export interface WidgetModelOverride {
  provider_id: string;
  model: string;
  required_caps?: WidgetCapability[];
}

/** W43: typed error returned by `set_dashboard_model_policy` /
 * `set_widget_model_override` / refresh when the policy can't be honoured.
 * Surfaced through `ApiResponse.error` as a JSON-encoded message; callers
 * may match on the `code` prefix (`widget_model_*`). */
export type WidgetModelErrorCode =
  | 'widget_model_provider_missing'
  | 'widget_model_provider_disabled'
  | 'widget_model_provider_invalid_config'
  | 'widget_model_capability_unsupported';

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
  | {
      kind: 'datasource_query';
      datasource_id: string;
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
  /** W34: dependent parameters re-resolved server-side using the new value. */
  downstream?: DashboardParameterState[];
}

export interface ResolveDashboardParametersResult {
  values: Record<string, ParameterValue>;
  cycle?: string[];
}

export type WidgetType = 'chart' | 'text' | 'table' | 'image' | 'gauge' | 'stat' | 'logs' | 'bar_gauge' | 'status_grid' | 'heatmap' | 'gallery';

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

export interface GalleryWidget extends WidgetBase {
  type: 'gallery';
  config: GalleryConfig;
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
  | HeatmapWidget
  | GalleryWidget;

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
  | HeatmapWidgetRuntimeData
  | GalleryWidgetRuntimeData;

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

export interface GalleryItem {
  src: string;
  title?: string;
  caption?: string;
  alt?: string;
  source?: string;
  link?: string;
  id?: string;
}

export interface GalleryWidgetRuntimeData {
  kind: 'gallery';
  items: GalleryItem[];
}

export interface WidgetRefreshResult {
  status: 'ok' | 'unsupported' | 'not_implemented' | 'error' | string;
  workflow_run_id?: string;
  data?: WidgetRuntimeData;
  error?: string;
}

/**
 * W40: one entry per refreshable widget returned by the batched
 * `refresh_dashboard_widgets` command. Mirrors
 * `src-tauri/src/commands/dashboard.rs::DashboardWidgetRefreshResult`.
 * Widgets without a datasource are silently omitted by the backend, so
 * the result vector only carries widgets the loop actually touched.
 */
export interface DashboardWidgetRefreshResult {
  widget_id: string;
  status: 'ok' | 'error' | string;
  workflow_run_id?: string;
  data?: WidgetRuntimeData;
  error?: string;
}

/**
 * W36: cached last-known-good runtime value for one widget. Mirrors
 * `src-tauri/src/models/snapshot.rs::WidgetRuntimeSnapshot`. Returned
 * by `list_widget_snapshots` after the backend has already pruned
 * fingerprint-mismatched entries — incompatible snapshots never reach
 * the UI.
 */
export interface WidgetRuntimeSnapshot {
  dashboard_id: string;
  widget_id: string;
  widget_kind: string;
  runtime_data: WidgetRuntimeData;
  captured_at: number;
  workflow_id?: string;
  workflow_run_id?: string;
  datasource_definition_id?: string;
  config_fingerprint: string;
  parameter_fingerprint: string;
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

export interface GalleryConfig {
  layout?: 'grid' | 'row' | 'masonry';
  thumbnail_aspect?: 'square' | 'landscape' | 'portrait' | 'original';
  max_visible_items?: number;
  show_caption?: boolean;
  show_source?: boolean;
  fullscreen_enabled?: boolean;
  fit?: 'cover' | 'contain' | 'fill';
  border_radius?: number;
}

export interface DatasourceConfig {
  workflow_id: string;
  output_key: string;
  post_process?: PostProcessStep[];
  /** W23: opt-in pipeline trace capture on every refresh. */
  capture_traces?: boolean;
  /** W31: optional saved-datasource identity bound to this widget. */
  datasource_definition_id?: string;
  /** W31: which surface last wrote this binding. */
  binding_source?: DatasourceBindingSource;
  /** W31: timestamp of the last binding change. */
  bound_at?: number;
  /** W31.1: per-widget tail pipeline applied after the workflow output. */
  tail_pipeline?: PipelineStep[];
  /** W43: per-widget LLM override. Wins over dashboard default. */
  model_override?: WidgetModelOverride | null;
}

export type DatasourceBindingSource =
  | 'build_chat'
  | 'workbench'
  | 'playground'
  | 'import'
  | 'manual';

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
  /** W47: per-session assistant language override. Wins over dashboard
   *  and app defaults. Null/undefined falls back to the wider scope. */
  language_override?: AssistantLanguagePolicy | null;
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
      /** W51: compression telemetry surfaced on the chat trace part. */
      compression?: ToolResultCompression;
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
  | { type: 'reflection_meta'; widget_ids: string[] }
  | { type: 'widget_mentions'; mentions: WidgetMention[] }
  | { type: 'source_mentions'; mentions: SourceMention[] };

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
  /** W51: when present, `result` already carries the compressor's
   *  compact payload and the raw redacted payload lives in the local
   *  `raw_artifacts` table. UI reads this to render a `87 KB → 2.1 KB
   *  sent (~96% saved)` badge and offer the "view raw locally" button. */
  compression?: ToolResultCompression;
}

/** W51: per-tool-result compression telemetry mirrored from the Rust
 *  `ToolResultCompression` struct. `raw_artifact_id` is the lookup key
 *  for `debugApi.getRawArtifact`; `truncation_paths` are the JSON
 *  pointers the model can quote when calling `inspect_artifact`. */
export interface ToolResultCompression {
  profile: string;
  raw_bytes: number;
  compact_bytes: number;
  estimated_tokens_saved: number;
  raw_artifact_id?: string;
  truncation_paths: string[];
}

/** W51: payload behind a `raw_artifact_id`. Already post-redaction —
 *  the compressor strips secrets before persisting — so it is safe to
 *  render inline in debug surfaces. */
export interface RawArtifactPayload {
  id: string;
  owner_kind: string;
  owner_id: string;
  profile: string;
  raw_size: number;
  compact_size: number;
  checksum: string;
  redaction_version: number;
  retention_class: string;
  payload_json: string;
  created_at: number;
}

export interface MessageMetadata {
  model?: string;
  provider?: string;
  tokens?: {
    prompt: number;
    completion: number;
    reasoning?: number;
    /** W49: provider-reported total cost for the turn, when available
     *  (OpenRouter returns this as `usage.cost`). */
    provider_cost_usd?: number;
  };
  latency_ms?: number;
  build_proposal?: BuildProposal;
  reasoning?: string;
  /** W22: resolved USD cost for this single assistant turn. */
  cost_usd?: number;
  /** W49: provenance of `cost_usd`. */
  cost_source?: CostSource;
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
  | { kind: 'duplicate_shared_key'; key: string }
  | {
      kind: 'unknown_parameter_reference';
      widget_index: number;
      widget_title: string;
      param_name: string;
    }
  | { kind: 'parameter_cycle'; cycle: string[] }
  | {
      /** W38: Build proposal mutated a widget that was not in the mention
       *  target set. Allowed targets travel in `target_widget_ids`. */
      kind: 'off_target_widget_replace';
      widget_index: number;
      widget_title: string;
      replace_widget_id: string;
    }
  | {
      kind: 'off_target_widget_remove';
      remove_widget_id: string;
    }
  | {
      /** W39: an http_request datasource (inline or shared) failed the
       *  safety gate before apply (bad method/scheme/host/headers). */
      kind: 'unsafe_http_datasource';
      widget_index: number;
      widget_title: string;
      source_kind: string;
      reason: string;
    }
  | {
      /** W44: gallery widget bakes a hardcoded image array in `data`
       *  without a pipeline producing items. */
      kind: 'hardcoded_gallery_items';
      widget_index: number;
      widget_title: string;
      item_count: number;
    }
  | {
      /** W45: agent set explicit x/y on a new widget. Auto-pack owns
       *  placement; explicit coordinates are always wrong. */
      kind: 'proposed_explicit_coordinates';
      widget_index: number;
      widget_title: string;
    }
  | {
      /** W45: widget declares both size_preset and raw w/h. Force the
       *  agent to pick one. */
      kind: 'conflicting_layout_fields';
      widget_index: number;
      widget_title: string;
    }
  | {
      /** W48: Build proposal failed to reference one or more sources the
       *  user named with `@source`. The agent must rerun and produce a
       *  widget whose datasource_plan consumes every missing entry. */
      kind: 'unused_source_mention';
      missing: Array<{
        label: string;
        datasource_definition_id?: string;
        workflow_id?: string;
      }>;
    };

/** W38: typed mention of an existing widget captured in the Build chat
 *  composer. Stays on the SendMessageRequest so the agent + validator can
 *  scope edits to the user-named targets. */
export interface WidgetMention {
  widget_id: string;
  dashboard_id?: string;
  label: string;
  widget_kind?: string;
}

/** W48: typed mention of an existing data source captured in the Build
 *  chat composer. Resolved on the backend against DatasourceDefinition
 *  rows + legacy workflows + widget bindings. At least one of the three
 *  id fields must be set; labels are presentation only. */
export type SourceMentionKind = 'datasource' | 'workflow' | 'widget';

export interface SourceMention {
  kind: SourceMentionKind;
  label: string;
  datasource_definition_id?: string;
  workflow_id?: string;
  widget_id?: string;
  dashboard_id?: string;
  input_alias?: string;
}

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
      /** W51: compression telemetry surfaced on the chat trace part. */
      compression?: ToolResultCompression;
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
  /** W51: streaming-event compression telemetry. */
  compression?: ToolResultCompression;
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
  /** W45: named size preset, resolved against the widget kind to (w, h).
   *  Mutually exclusive with raw `w`/`h` (validator rejects both). */
  size_preset?: SizePreset;
  /** W45: layout pattern hint at the widget level. Soft hint only —
   *  packer still places row-first by array order. */
  layout_pattern?: LayoutPattern;
}

/** W45: small set of named size presets resolved at apply time. */
export type SizePreset =
  | 'kpi'
  | 'half_width'
  | 'wide_chart'
  | 'full_width'
  | 'table'
  | 'text_panel'
  | 'gallery';

/** W45: named layout pattern intent for a widget. */
export type LayoutPattern =
  | 'kpi_row'
  | 'trend_chart_row'
  | 'operations_table'
  | 'datasource_overview'
  | 'media_board'
  | 'text_panel';

export interface BuildDatasourcePlan {
  kind: 'builtin_tool' | 'mcp_tool' | 'provider_prompt' | 'shared' | 'compose';
  tool_name?: string;
  server_id?: string;
  arguments?: Record<string, unknown>;
  prompt?: string;
  output_path?: string;
  refresh_cron?: string;
  pipeline?: PipelineStep[];
  source_key?: string;
  inputs?: Record<string, BuildDatasourcePlan>;
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
  | { kind: 'llm_postprocess'; prompt: string; expect?: 'text' | 'json' }
  | {
      kind: 'mcp_call';
      server_id: string;
      tool_name: string;
      arguments?: unknown;
    };

/**
 * W29: production provider kinds. `local_mock` was removed; existing
 * persisted rows are normalised by the backend to `is_unsupported: true`
 * and surfaced to the UI as a force-disabled row carrying a remediation
 * message. New providers can never be created with that kind.
 */
export type ProviderKind = 'openrouter' | 'ollama' | 'custom';

export interface LLMProvider {
  id: string;
  name: string;
  kind: ProviderKind;
  base_url: string;
  api_key?: string;
  default_model: string;
  models: string[];
  is_enabled: boolean;
  /** W29: row's stored kind is not a supported product kind anymore. */
  is_unsupported?: boolean;
}

export interface CreateProviderRequest {
  name: string;
  kind: ProviderKind;
  base_url: string;
  api_key?: string;
  default_model: string;
  models?: string[];
}

export interface UpdateProviderRequest {
  name?: string;
  kind?: ProviderKind;
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

/**
 * W29: typed remediation surfaces for chat / build send paths. Mirrors
 * `models::provider::ProviderSetupError` in Rust. The Tauri command
 * still returns a string error today; UI parses the leading
 * `code: …` prefix to route to the right CTA.
 */
export type ProviderSetupErrorCode =
  | 'no_active_provider'
  | 'active_provider_missing'
  | 'active_provider_disabled'
  | 'provider_invalid_config'
  | 'provider_unsupported'
  | 'provider_unavailable';

export interface ProviderSetupErrorInfo {
  code: ProviderSetupErrorCode;
  message: string;
}

const SETUP_ERROR_CODES: ProviderSetupErrorCode[] = [
  'no_active_provider',
  'active_provider_missing',
  'active_provider_disabled',
  'provider_invalid_config',
  'provider_unsupported',
  'provider_unavailable',
];

/**
 * W29: parse the typed prefix from a backend send error so the UI can
 * branch on the remediation code. Returns `null` for non-setup errors.
 */
export function parseProviderSetupError(message: string): ProviderSetupErrorInfo | null {
  for (const code of SETUP_ERROR_CODES) {
    if (message.startsWith(`${code}:`)) {
      return { code, message };
    }
  }
  return null;
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
  /** W31: datasource identity (workflow_id / definition_id / output_key) changed. */
  binding_changed?: boolean;
  /** W31: per-widget tail (post_process, capture_traces, provenance) changed. */
  tail_changed?: boolean;
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

// ─── W39: proposal materialization preview ──────────────────────────────────

export interface MaterializationEntry {
  widget_title: string;
  source_kind: string;
  label: string;
  origin: string;
  datasource_definition_id?: string;
  workflow_id?: string;
}

export interface MaterializationReject {
  widget_title: string;
  source_kind: string;
  origin: string;
  reason: string;
}

export interface ProposalMaterializationPreview {
  creates: MaterializationEntry[];
  reuses: MaterializationEntry[];
  rejects: MaterializationReject[];
  passthrough: MaterializationEntry[];
  total_widgets: number;
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
  /** W43: write the dashboard-level default LLM policy. `policy: null`
   *  clears the default — eligible widgets then fall back to the app
   *  active provider. Server-side validation runs the resolver against
   *  the live provider list and rejects bad policies up front. */
  setModelPolicy: (dashboardId: string, policy: DashboardModelPolicy | null) =>
    call<Dashboard>('set_dashboard_model_policy', {
      req: { dashboard_id: dashboardId, policy },
    }),
  /** W43: write a per-widget LLM override. `policy: null` clears it. */
  setWidgetModelOverride: (
    dashboardId: string,
    widgetId: string,
    policy: WidgetModelOverride | null,
  ) =>
    call<Dashboard>('set_widget_model_override', {
      req: { dashboard_id: dashboardId, widget_id: widgetId, policy },
    }),
  /** W47: write the dashboard-level assistant language policy.
   *  `policy: null` clears it; the dashboard then falls back to the app
   *  default (and then to `auto`). */
  setLanguagePolicy: (dashboardId: string, policy: AssistantLanguagePolicy | null) =>
    call<Dashboard>('set_dashboard_language_policy', {
      req: { dashboard_id: dashboardId, policy },
    }),
  /** W39: per-source resolution preview (create / reuse / reject /
   *  passthrough). Read-only — mirrors apply's materialization logic. */
  previewProposalMaterialization: (proposal: BuildProposal) =>
    call<ProposalMaterializationPreview>('preview_proposal_materialization', {
      req: { proposal },
    }),
  dryRunWidget: (proposal: BuildWidgetProposal, sharedDatasources?: SharedDatasource[]) =>
    call<WidgetDryRunResult>('dry_run_widget', { proposal, sharedDatasources }),
  delete: (id: string) => call<boolean>('delete_dashboard', { id }),
  refreshWidget: (dashboardId: string, widgetId: string) =>
    call<WidgetRefreshResult>('refresh_widget', { dashboardId, widgetId }),
  /**
   * W40: refresh several widgets on a dashboard in one Tauri call.
   * Widgets sharing a `workflow_id` execute the shared workflow exactly
   * once and their per-widget tail pipelines run against the shared
   * output. Independent workflows run concurrently with a bounded cap
   * so one slow upstream call does not block the rest of the grid.
   * `widgetIds = undefined` refreshes every refreshable widget.
   */
  refreshDashboardWidgets: (dashboardId: string, widgetIds?: string[]) =>
    call<DashboardWidgetRefreshResult[]>('refresh_dashboard_widgets', {
      dashboardId,
      widgetIds,
    }),
  // W36: hydrate cached widget data before live refresh paints
  listWidgetSnapshots: (dashboardId: string) =>
    call<WidgetRuntimeSnapshot[]>('list_widget_snapshots', { dashboardId }),
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
  // W34: refresh query-backed options for one parameter
  refreshParameterOptions: (dashboardId: string, paramName: string) =>
    call<DashboardParameterState>('refresh_dashboard_parameter_options', {
      dashboardId,
      paramName,
    }),
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
  /** W51: fetch the redacted raw payload behind a compressed tool
   *  result. Returns `null` when the artifact id is unknown or has
   *  been pruned. */
  getRawArtifact: (artifactId: string) =>
    callNullable<RawArtifactPayload>('get_raw_artifact', { artifactId }),
  /** W32: replay a candidate pipeline against an inline sample or a
   *  stored W23 trace. Deterministic-only — Studio replay rejects
   *  `llm_postprocess` and `mcp_call` so previews never trigger
   *  network calls. Run a full traced refresh through `traceWidget`
   *  if you need the provider/MCP-aware path. */
  replayPipeline: (req: ReplayPipelineRequest) =>
    call<PipelineReplayResult>('replay_pipeline', { req }),
};

export interface ReplayPipelineRequest {
  steps: PipelineStep[];
  sample?: unknown;
  from_widget_trace?: { widget_id: string; captured_at: number };
}

export interface PipelineReplayResult {
  started_at: number;
  finished_at: number;
  initial_value?: unknown;
  steps: PipelineStepTrace[];
  final_value?: unknown;
  error?: string;
  /** 0-based index of the first failed / empty step, or undefined. */
  first_empty_step_index?: number;
}

// ─── W41: Widget Execution Observability ────────────────────────────────────

export type LlmParticipation =
  | 'none'
  | 'provider_source'
  | 'llm_postprocess'
  | 'widget_text_generation'
  | 'unknown';

export type ProvenanceSource =
  | { kind: 'mcp_tool'; server_id: string; tool_name: string; arguments_preview?: unknown }
  | { kind: 'builtin_tool'; tool_name: string; arguments_preview?: unknown }
  | { kind: 'provider_prompt'; prompt_preview: string }
  | { kind: 'compose'; inputs: ProvenanceComposeInput[] }
  | { kind: 'unknown' }
  | { kind: 'missing'; workflow_id: string };

export interface ProvenanceComposeInput {
  name: string;
  source: ProvenanceSource;
}

export interface DatasourceProvenance {
  workflow_id: string;
  output_key: string;
  datasource_definition_id?: string;
  datasource_name?: string;
  binding_source?: DatasourceBindingSource;
  bound_at?: number;
  source: ProvenanceSource;
  trigger?: 'cron' | 'event' | 'manual';
  refresh_cron?: string;
  /** W50: user pause state on the backing workflow. `paused` means
   *  automatic refresh is intentionally off — manual refresh still works. */
  pause_state?: SchedulePauseState;
}

export interface ProviderProvenance {
  provider_id: string;
  provider_name: string;
  provider_kind: string;
  model: string;
  is_active_provider: boolean;
  /** W43: inheritance source for the model badge. */
  model_source?: WidgetModelSource;
  /** W43: capabilities the resolved policy pinned. */
  required_caps?: WidgetCapability[];
}

export interface ProvenanceTailSummary {
  step_count: number;
  has_llm_postprocess: boolean;
  has_mcp_call: boolean;
  kinds?: string[];
}

export interface ProvenanceLastRun {
  run_id: string;
  status: 'idle' | 'running' | 'success' | 'error' | 'skipped';
  started_at: number;
  finished_at?: number;
  duration_ms?: number;
  error?: string;
}

export interface ProvenanceLinks {
  workflow_id?: string;
  datasource_definition_id?: string;
  has_pipeline_traces: boolean;
}

export interface WidgetProvenance {
  dashboard_id: string;
  widget_id: string;
  widget_title: string;
  widget_kind: string;
  llm_participation: LlmParticipation;
  datasource?: DatasourceProvenance;
  provider?: ProviderProvenance;
  tail: ProvenanceTailSummary;
  last_run?: ProvenanceLastRun;
  links: ProvenanceLinks;
  redacted_summary: string;
}

export const provenanceApi = {
  getWidget: (dashboardId: string, widgetId: string) =>
    call<WidgetProvenance>('get_widget_provenance', { dashboardId, widgetId }),
};

// ─── W30: Datasource Workbench API ──────────────────────────────────────────

export type DatasourceDefinitionKind =
  | 'builtin_tool'
  | 'mcp_tool'
  | 'provider_prompt';

export type DatasourceHealthStatus = 'ok' | 'error';

export interface DatasourceHealth {
  last_run_at: number;
  last_status: DatasourceHealthStatus;
  last_error?: string;
  last_duration_ms: number;
  sample_preview?: unknown;
  consumer_count?: number;
}

export interface DatasourceDefinition {
  id: string;
  name: string;
  description?: string;
  kind: DatasourceDefinitionKind;
  tool_name?: string;
  server_id?: string;
  arguments?: unknown;
  prompt?: string;
  pipeline?: PipelineStep[];
  refresh_cron?: string;
  workflow_id: string;
  created_at: number;
  updated_at: number;
  health?: DatasourceHealth;
  /** W37: catalog id this definition was created from, if any. */
  originated_external_source_id?: string;
}

export interface CreateDatasourceRequest {
  name: string;
  description?: string;
  kind: DatasourceDefinitionKind;
  tool_name?: string;
  server_id?: string;
  arguments?: unknown;
  prompt?: string;
  pipeline?: PipelineStep[];
  refresh_cron?: string;
}

export interface UpdateDatasourceRequest {
  name?: string;
  description?: string;
  tool_name?: string;
  server_id?: string;
  arguments?: unknown;
  prompt?: string;
  pipeline?: PipelineStep[];
  refresh_cron?: string;
}

export interface DatasourceConsumer {
  dashboard_id: string;
  dashboard_name: string;
  widget_id: string;
  widget_title: string;
  widget_kind: string;
  output_key: string;
  /** W31: `true` when the widget carries an explicit datasource_definition_id. */
  explicit_binding?: boolean;
  /** W31: surface that last wrote the binding, if known. */
  binding_source?: DatasourceBindingSource;
  /** W31: timestamp of the last binding change. */
  bound_at?: number;
  /** W31.1: number of per-widget tail pipeline steps. */
  tail_step_count?: number;
}

export interface DatasourceImpactPreview {
  datasource_id: string;
  datasource_name: string;
  workflow_id: string;
  consumers: DatasourceConsumer[];
  legacy_consumer_count: number;
  has_explicit_consumers: boolean;
}

export interface DatasourceBindingChange {
  dashboard_id: string;
  widget_id: string;
  datasource_definition_id?: string;
  workflow_id?: string;
  binding_source?: DatasourceBindingSource;
  previous_workflow_id?: string;
  previous_datasource_definition_id?: string;
}

export interface DatasourceRunResult {
  status: DatasourceHealthStatus;
  duration_ms: number;
  error?: string;
  raw_source?: unknown;
  final_value?: unknown;
  pipeline_steps: number;
  workflow_node_ids: string[];
}

export interface DatasourceExportBundle {
  version: number;
  exported_at: number;
  definitions: DatasourceDefinition[];
}

export interface ImportDatasourcesResult {
  imported: number;
  skipped: number;
  overwritten: number;
  errors: string[];
}

export const datasourceApi = {
  list: () => call<DatasourceDefinition[]>('list_datasource_definitions'),
  get: (id: string) => call<DatasourceDefinition>('get_datasource_definition', { id }),
  create: (req: CreateDatasourceRequest) =>
    call<DatasourceDefinition>('create_datasource_definition', { req }),
  update: (id: string, req: UpdateDatasourceRequest) =>
    call<DatasourceDefinition>('update_datasource_definition', { id, req }),
  remove: (id: string) => call<boolean>('delete_datasource_definition', { id }),
  duplicate: (id: string) =>
    call<DatasourceDefinition>('duplicate_datasource_definition', { id }),
  run: (id: string) => call<DatasourceRunResult>('run_datasource_definition', { id }),
  listConsumers: (id: string) =>
    call<DatasourceConsumer[]>('list_datasource_consumers', { id }),
  previewImpact: (id: string) =>
    call<DatasourceImpactPreview>('preview_datasource_impact', { id }),
  bindWidget: (req: {
    dashboard_id: string;
    widget_id: string;
    datasource_definition_id: string;
    output_key?: string;
    binding_source?: DatasourceBindingSource;
  }) => call<DatasourceBindingChange>('bind_widget_to_datasource', { req }),
  unbindWidget: (req: {
    dashboard_id: string;
    widget_id: string;
    drop_datasource?: boolean;
  }) => call<DatasourceBindingChange>('unbind_widget_from_datasource', { req }),
  export: (ids: string[] = []) =>
    call<DatasourceExportBundle>('export_datasource_definitions', { req: { ids } }),
  import: (bundle: DatasourceExportBundle, overwrite = false) =>
    call<ImportDatasourcesResult>('import_datasource_definitions', {
      req: { bundle, overwrite },
    }),
};

// ─── W37: External Source Catalog API ───────────────────────────────────────

export type ExternalSourceReviewStatus =
  | 'allowed'
  | 'allowed_with_conditions'
  | 'needs_review'
  | 'blocked';

export type ExternalSourceAdapter = 'http_json' | 'web_fetch' | 'mcp_recommended';

export type ExternalSourceDomain =
  | 'web_search'
  | 'web_fetch'
  | 'knowledge_base'
  | 'crypto_market'
  | 'developer_data'
  | 'news'
  | 'mcp_recommended';

export type ExternalSourceCredentialPolicy = 'none' | 'optional' | 'required';

export interface ExternalSourceRateLimit {
  plan_name: string;
  free_quota: string;
  paid_tier?: string;
  queries_per_second?: number;
  attribution_required: boolean;
  storage_rights_required: boolean;
}

export interface McpInstallEnvHint {
  name: string;
  description: string;
  required: boolean;
}

export interface McpInstallRecommendation {
  command: string;
  args: string[];
  env_hints: McpInstallEnvHint[];
  package_kind: string;
  package_name: string;
}

export interface ExternalSourceParam {
  name: string;
  description: string;
  schema?: unknown;
  required: boolean;
  default?: unknown;
}

export interface ExternalSourceHttpRequest {
  method: string;
  url: string;
  query?: Record<string, string>;
  headers?: Record<string, string>;
  credential_header?: string;
  credential_prefix?: string;
  body_params?: string[];
}

export interface ExternalSource {
  id: string;
  display_name: string;
  description: string;
  domain: ExternalSourceDomain;
  adapter: ExternalSourceAdapter;
  review_status: ExternalSourceReviewStatus;
  review_date: string;
  adapter_license: string;
  terms_url: string;
  review_notes: string;
  attribution?: string;
  credential_policy: ExternalSourceCredentialPolicy;
  credential_help?: string;
  http: ExternalSourceHttpRequest;
  params: ExternalSourceParam[];
  default_pipeline?: PipelineStep[];
  /** W37++: published plan/rate metadata for the UI. */
  rate_limit?: ExternalSourceRateLimit;
  /** W37++: install metadata for recommended MCP servers. */
  mcp_install?: McpInstallRecommendation;
}

export interface ExternalSourceState {
  source_id: string;
  is_enabled: boolean;
  has_credential: boolean;
  updated_at: number;
}

export interface ExternalSourceWithState extends ExternalSource {
  state: ExternalSourceState;
  is_runnable: boolean;
  blocked_reason?: string;
}

export interface ExternalSourceTestResult {
  source_id: string;
  duration_ms: number;
  raw_response: unknown;
  final_value: unknown;
  pipeline_steps: number;
  effective_url: string;
}

export interface SaveExternalSourceResult {
  source_id: string;
  datasource_id: string;
  workflow_id: string;
}

export interface OriginatingDatasource {
  datasource_id: string;
  name: string;
  workflow_id: string;
}

export interface ExternalSourceImpactPreview {
  source_id: string;
  originating_datasources: OriginatingDatasource[];
  has_credential: boolean;
}

export const externalSourceApi = {
  list: () => call<ExternalSourceWithState[]>('list_external_sources'),
  setEnabled: (sourceId: string, enabled: boolean) =>
    call<ExternalSourceWithState>('set_external_source_enabled', {
      sourceId,
      enabled,
    }),
  setCredential: (sourceId: string, credential: string | null) =>
    call<ExternalSourceWithState>('set_external_source_credential', {
      sourceId,
      credential,
    }),
  test: (sourceId: string, args: Record<string, unknown> = {}) =>
    call<ExternalSourceTestResult>('test_external_source', {
      req: { source_id: sourceId, arguments: args },
    }),
  saveAsDatasource: (req: {
    source_id: string;
    name: string;
    arguments: Record<string, unknown>;
    refresh_cron?: string;
  }) => call<SaveExternalSourceResult>('save_external_source_as_datasource', { req }),
  previewImpact: (sourceId: string) =>
    call<ExternalSourceImpactPreview>('preview_external_source_impact', { sourceId }),
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
  sendMessage: (
    sessionId: string,
    content: string,
    widgetMentions?: WidgetMention[],
    sourceMentions?: SourceMention[],
  ) =>
    call<ChatMessage>('send_message', {
      sessionId,
      req: {
        content,
        widget_mentions: widgetMentions ?? [],
        source_mentions: sourceMentions ?? [],
      },
    }),
  sendMessageStream: (
    sessionId: string,
    content: string,
    widgetMentions?: WidgetMention[],
    sourceMentions?: SourceMention[],
  ) =>
    call<ChatMessage>('send_message_stream', {
      sessionId,
      req: {
        content,
        widget_mentions: widgetMentions ?? [],
        source_mentions: sourceMentions ?? [],
      },
    }),
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

// ─── Workflow Operations Cockpit (W35) ───────────────────────────────────────

export type RunStatusValue = 'idle' | 'running' | 'success' | 'error' | 'skipped';

export interface WorkflowRunSummary {
  id: string;
  workflow_id: string;
  started_at: number;
  finished_at?: number;
  status: RunStatusValue;
  duration_ms?: number;
  error?: string;
  has_node_results: boolean;
}

export interface WorkflowRunFilter {
  workflow_id?: string;
  dashboard_id?: string;
  widget_id?: string;
  datasource_definition_id?: string;
  status?: RunStatusValue;
  limit?: number;
}

export interface WorkflowOwnerWidget {
  widget_id: string;
  widget_title: string;
  widget_kind: string;
  output_key: string;
  explicit_binding: boolean;
}

export interface WorkflowOwnerDashboard {
  dashboard_id: string;
  dashboard_name: string;
  widgets: WorkflowOwnerWidget[];
}

export interface WorkflowOwnerRef {
  datasource_definition_id?: string;
  datasource_name?: string;
  dashboards: WorkflowOwnerDashboard[];
}

/** W50: user-controlled pause flag. Independent of `is_enabled` so an
 *  operator can stop automatic refresh without disabling the workflow. */
export type SchedulePauseState = 'active' | 'paused';

/** W50: single label the UI surfaces. Rust is the source of truth — do
 *  not recompute the value on the React side. */
export type ScheduleDisplayState =
  | 'active'
  | 'paused_by_user'
  | 'manual_only'
  | 'invalid'
  | 'disabled'
  | 'not_scheduled';

export interface WorkflowScheduleSummary {
  is_scheduled: boolean;
  cron?: string;
  cron_normalized?: string;
  cron_is_valid: boolean;
  trigger_kind?: 'cron' | 'event' | 'manual';
  /** W50: user-paused vs ticking. */
  pause_state: SchedulePauseState;
  last_paused_at?: number;
  last_pause_reason?: string;
  /** W50: rendered label, derived on the Rust side. */
  display_state: ScheduleDisplayState;
}

export interface WorkflowSummary {
  id: string;
  name: string;
  description?: string;
  is_enabled: boolean;
  trigger: WorkflowTrigger;
  schedule: WorkflowScheduleSummary;
  last_run?: WorkflowRunSummary;
  owner: WorkflowOwnerRef;
  created_at: number;
  updated_at: number;
}

export interface WorkflowRunDetail {
  run: WorkflowRun;
  workflow_id: string;
  workflow_name: string;
  owner: WorkflowOwnerRef;
}

export interface WorkflowRunCancelOutcome {
  cancelled: boolean;
  reason: string;
  run_id: string;
  run_status?: RunStatusValue;
}

export type SchedulerWarningKind =
  | 'invalid_cron'
  | 'cron_trigger_disabled'
  | 'scheduled_but_disabled'
  | 'enabled_but_not_scheduled';

export interface SchedulerWarning {
  workflow_id: string;
  workflow_name: string;
  kind: SchedulerWarningKind;
  message: string;
}

export interface SchedulerHealth {
  scheduler_started: boolean;
  scheduled_workflow_ids: string[];
  warnings: SchedulerWarning[];
}

export const operationsApi = {
  listWorkflowSummaries: () =>
    call<WorkflowSummary[]>('list_workflow_summaries'),
  listRuns: (filter?: WorkflowRunFilter) =>
    call<WorkflowRunSummary[]>('list_workflow_runs', { filter }),
  getRunDetail: (runId: string) =>
    call<WorkflowRunDetail>('get_workflow_run_detail', { runId }),
  retryRun: (runId: string) =>
    call<WorkflowRun>('retry_workflow_run', { runId }),
  cancelRun: (runId: string) =>
    call<WorkflowRunCancelOutcome>('cancel_workflow_run', { runId }),
  schedulerHealth: () => call<SchedulerHealth>('get_scheduler_health'),
};

// ─── W50: schedule pause + cadence controls ────────────────────────────────

export const scheduleApi = {
  /** Pause automatic refresh for a single workflow. Manual refresh still
   *  works. Returns the updated summary so the UI does not re-fetch. */
  pauseWorkflow: (workflowId: string, reason?: string) =>
    call<WorkflowSummary>('pause_workflow_schedule', { workflowId, reason }),
  /** Resume automatic refresh. Re-registers the cron job only when the
   *  cron expression is still valid. */
  resumeWorkflow: (workflowId: string) =>
    call<WorkflowSummary>('resume_workflow_schedule', { workflowId }),
  /** Update / clear the cron expression on an existing workflow. Pass
   *  `null` to revert the trigger to manual. */
  setWorkflowCron: (workflowId: string, cron: string | null) =>
    call<WorkflowSummary>('set_workflow_schedule', { workflowId, cron }),
  /** Pause every distinct workflow referenced by the dashboard's
   *  widgets. */
  pauseDashboard: (dashboardId: string, reason?: string) =>
    call<WorkflowSummary[]>('pause_dashboard_schedules', { dashboardId, reason }),
  resumeDashboard: (dashboardId: string) =>
    call<WorkflowSummary[]>('resume_dashboard_schedules', { dashboardId }),
};

/** W50: friendly cadence presets shared by every schedule editor. The
 *  `cron` field is the 6-field form expected by `tokio_cron_scheduler`. */
export interface SchedulePreset {
  id: string;
  label: string;
  cron: string | null;
}

export const SCHEDULE_PRESETS: SchedulePreset[] = [
  { id: 'manual', label: 'Manual only', cron: null },
  { id: 'every_1m', label: 'Every 1 minute', cron: '0 * * * * *' },
  { id: 'every_5m', label: 'Every 5 minutes', cron: '0 */5 * * * *' },
  { id: 'every_15m', label: 'Every 15 minutes', cron: '0 */15 * * * *' },
  { id: 'every_60m', label: 'Every hour', cron: '0 0 * * * *' },
];

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
  getHttpUserAgent: () => call<string>('get_http_user_agent'),
  setHttpUserAgent: (userAgent: string) =>
    call<string>('set_http_user_agent', { userAgent }),
};

// ─── Config API ──────────────────────────────────────────────────────────────

export const configApi = {
  get: (key: string) => callNullable<string>('get_config', { key }),
  set: (key: string, value: string) => call<boolean>('set_config', { key, value }),
};

// ─── W47: Assistant language API ────────────────────────────────────────────

export const languageApi = {
  /** Curated BCP-47 catalog with provider support hints. Static, served
   *  from Rust so the Rust-side resolver and the React selector cannot
   *  drift. */
  list: () => call<AssistantLanguageCatalog>('list_assistant_languages'),
  /** App-level default policy. `auto` is the factory default. */
  getAppPolicy: () => call<AssistantLanguagePolicy>('get_app_assistant_language'),
  setAppPolicy: (policy: AssistantLanguagePolicy) =>
    call<AssistantLanguagePolicy>('set_app_assistant_language', { policy }),
  /** Per-session override; takes precedence over dashboard + app. */
  setSessionPolicy: (sessionId: string, policy: AssistantLanguagePolicy | null) =>
    call<ChatSession>('set_session_language_policy', {
      req: { session_id: sessionId, policy },
    }),
  /** Resolve the effective language for a given scope. Returns the
   *  source surface ("session_override" / "dashboard_override" /
   *  "app_default" / "auto") plus the catalog option (or `null` for
   *  auto). */
  resolve: (scope: { dashboard_id?: string; session_id?: string } = {}) =>
    call<EffectiveAssistantLanguage>('resolve_assistant_language', { req: scope }),
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

// ─── Widget streaming events (W42) ───────────────────────────────────────────

export const WIDGET_STREAM_EVENT_CHANNEL = 'widget:stream';

export type WidgetStreamKind =
  | 'refresh_started'
  | 'reasoning_delta'
  | 'text_delta'
  | 'status'
  | 'final'
  | 'failed'
  | 'superseded';

export interface WidgetStreamEnvelope {
  dashboard_id: string;
  widget_id: string;
  refresh_run_id: string;
  sequence: number;
  kind: WidgetStreamKind;
  text?: string;
  final_data?: WidgetRuntimeData;
  partial_text?: string;
  error?: string;
  status?: string;
  workflow_run_id?: string;
  emitted_at: number;
}

/**
 * W42: live streaming state for a single widget refresh as reconciled
 * by the UI. Lives in `api.ts` so widget components and DashboardGrid
 * can import it without pulling in the whole `App.tsx` module graph.
 */
export interface WidgetStreamState {
  runId: string;
  status: 'starting' | 'reasoning' | 'streaming' | 'waiting' | 'failed';
  partialText: string;
  reasoningText: string;
  hasReasoning: boolean;
  statusHint?: string;
  error?: string;
  partialOnFail?: string;
}

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
  /** W35: id of the workflow run that produced the value this alert
   * fired on. Populated for alerts evaluated post-refresh; `null` for
   * legacy rows. Click → Operations cockpit with the run preselected. */
  workflow_run_id?: string | null;
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

/**
 * W49: provenance of a single turn's cost figure. Surfaced on the
 * footer so operators can tell whether the number came from the
 * upstream provider's own billing field or from the local pricing
 * table, and whether the session has any unpriced turns.
 */
export type CostSource = 'provider_total' | 'pricing_table' | 'unknown_pricing';

export interface SessionCostSnapshot {
  session_id: string;
  model?: string;
  input_tokens: number;
  output_tokens: number;
  reasoning_tokens: number;
  cost_usd: number;
  max_cost_usd?: number | null;
  today_cost_usd: number;
  /** W49: number of assistant turns whose tokens were recorded but
   *  pricing was unknown. When > 0, `cost_usd` is a lower bound. */
  cost_unknown_turns?: number;
  /** W49: provenance of the most recent assistant turn's cost. */
  latest_cost_source?: CostSource;
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
  provider_kind?: ProviderKind;
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

// W32: typed pipeline step registry.
//
// Mirrors the Rust `PipelineStep` enum (`src-tauri/src/models/pipeline.rs`)
// so the UI can render a typed form for each variant instead of asking
// the user to hand-write JSON. The registry stays declarative — every
// variant lists its label, a short description, required-field set,
// validation, and a default seed used when adding the step. The Studio
// component reads this metadata and renders rows; it does not branch
// on `kind` itself.

import type {
  AggregateMetric,
  CoerceTarget,
  FilterOp,
  PipelineStep,
  SortOrder,
} from '../api';

export type PipelineStepKind = PipelineStep['kind'];

export interface PipelineStepSpec {
  kind: PipelineStepKind;
  label: string;
  description: string;
  /** Marks deterministic-only Studio replay steps. Provider / MCP-aware
   * steps are accepted by the editor and persisted, but Studio replay
   * refuses to run them (the Rust command returns an explicit error). */
  advanced?: boolean;
  /** Best-effort validator. Returns `null` when the step is well-formed,
   * otherwise an error message ready for the UI. */
  validate: (step: PipelineStep) => string | null;
  /** Seed value used when the user adds a fresh step of this kind. */
  defaultStep: () => PipelineStep;
}

const FILTER_OPS: FilterOp[] = [
  'eq',
  'ne',
  'gt',
  'gte',
  'lt',
  'lte',
  'contains',
  'starts_with',
  'ends_with',
  'in',
  'not_in',
  'exists',
  'not_exists',
  'truthy',
  'falsy',
];

const SORT_ORDERS: SortOrder[] = ['asc', 'desc'];

const COERCE_TARGETS: CoerceTarget[] = ['number', 'integer', 'string', 'array'];

const AGGREGATE_METRICS: AggregateMetric['kind'][] = [
  'count',
  'sum',
  'avg',
  'min',
  'max',
  'first',
  'last',
];

export const FILTER_OP_OPTIONS = FILTER_OPS;
export const SORT_ORDER_OPTIONS = SORT_ORDERS;
export const COERCE_TARGET_OPTIONS = COERCE_TARGETS;
export const AGGREGATE_METRIC_OPTIONS = AGGREGATE_METRICS;

function requireString(value: unknown, label: string): string | null {
  if (typeof value !== 'string' || !value.trim()) return `${label} is required`;
  return null;
}

export const PIPELINE_STEP_SPECS: Record<PipelineStepKind, PipelineStepSpec> = {
  pick: {
    kind: 'pick',
    label: 'Pick',
    description: 'Navigate to a sub-value using a dotted path (supports [*], [n]).',
    validate: step =>
      step.kind === 'pick' ? requireString(step.path, 'path') : 'wrong kind',
    defaultStep: () => ({ kind: 'pick', path: '' }),
  },
  filter: {
    kind: 'filter',
    label: 'Filter',
    description: 'Keep array items where `field op value` is truthy.',
    validate: step => {
      if (step.kind !== 'filter') return 'wrong kind';
      const fieldErr = requireString(step.field, 'field');
      if (fieldErr) return fieldErr;
      if (step.op && !FILTER_OPS.includes(step.op)) return `unknown op ${step.op}`;
      return null;
    },
    defaultStep: () => ({ kind: 'filter', field: '', op: 'eq', value: '' }),
  },
  sort: {
    kind: 'sort',
    label: 'Sort',
    description: 'Sort an array by a field; non-arrays pass through.',
    validate: step => {
      if (step.kind !== 'sort') return 'wrong kind';
      const byErr = requireString(step.by, 'by');
      if (byErr) return byErr;
      if (step.order && !SORT_ORDERS.includes(step.order))
        return `unknown order ${step.order}`;
      return null;
    },
    defaultStep: () => ({ kind: 'sort', by: '', order: 'asc' }),
  },
  limit: {
    kind: 'limit',
    label: 'Limit',
    description: 'Keep the first N items of an array.',
    validate: step => {
      if (step.kind !== 'limit') return 'wrong kind';
      if (typeof step.count !== 'number' || step.count < 0 || !Number.isFinite(step.count))
        return 'count must be a non-negative number';
      return null;
    },
    defaultStep: () => ({ kind: 'limit', count: 10 }),
  },
  map: {
    kind: 'map',
    label: 'Map',
    description: 'Reshape each item: keep selected fields, optionally rename.',
    validate: step => {
      if (step.kind !== 'map') return 'wrong kind';
      if (step.fields && !Array.isArray(step.fields)) return 'fields must be a list';
      return null;
    },
    defaultStep: () => ({ kind: 'map', fields: [], rename: {} }),
  },
  aggregate: {
    kind: 'aggregate',
    label: 'Aggregate',
    description: 'Reduce an array into a single value or grouped buckets.',
    validate: step => {
      if (step.kind !== 'aggregate') return 'wrong kind';
      if (!step.metric || typeof step.metric !== 'object' || !('kind' in step.metric))
        return 'metric is required';
      if (!AGGREGATE_METRICS.includes(step.metric.kind)) return `unknown metric ${step.metric.kind}`;
      if (step.metric.kind !== 'count' && !('field' in step.metric && step.metric.field))
        return `${step.metric.kind} requires a field`;
      return null;
    },
    defaultStep: () => ({
      kind: 'aggregate',
      metric: { kind: 'count' },
      output_key: 'value',
    }),
  },
  set: {
    kind: 'set',
    label: 'Set field',
    description: 'Set or override a top-level field with a literal value.',
    validate: step =>
      step.kind === 'set' ? requireString(step.field, 'field') : 'wrong kind',
    defaultStep: () => ({ kind: 'set', field: '', value: '' }),
  },
  head: {
    kind: 'head',
    label: 'Head',
    description: 'Take the first element of an array.',
    validate: step => (step.kind === 'head' ? null : 'wrong kind'),
    defaultStep: () => ({ kind: 'head' }),
  },
  tail: {
    kind: 'tail',
    label: 'Tail',
    description: 'Take the last element of an array.',
    validate: step => (step.kind === 'tail' ? null : 'wrong kind'),
    defaultStep: () => ({ kind: 'tail' }),
  },
  length: {
    kind: 'length',
    label: 'Length',
    description: 'Replace the input with the length of the array.',
    validate: step => (step.kind === 'length' ? null : 'wrong kind'),
    defaultStep: () => ({ kind: 'length' }),
  },
  flatten: {
    kind: 'flatten',
    label: 'Flatten',
    description: 'Flatten one level of array-of-arrays.',
    validate: step => (step.kind === 'flatten' ? null : 'wrong kind'),
    defaultStep: () => ({ kind: 'flatten' }),
  },
  unique: {
    kind: 'unique',
    label: 'Unique',
    description: 'Deduplicate items by full equality or by a key.',
    validate: step => (step.kind === 'unique' ? null : 'wrong kind'),
    defaultStep: () => ({ kind: 'unique' }),
  },
  format: {
    kind: 'format',
    label: 'Format',
    description: 'Render a `{field}` template; per-item for arrays.',
    validate: step =>
      step.kind === 'format' ? requireString(step.template, 'template') : 'wrong kind',
    defaultStep: () => ({ kind: 'format', template: '' }),
  },
  coerce: {
    kind: 'coerce',
    label: 'Coerce',
    description: 'Coerce the value to number / integer / string / array.',
    validate: step => {
      if (step.kind !== 'coerce') return 'wrong kind';
      if (!COERCE_TARGETS.includes(step.to)) return `unknown target ${step.to}`;
      return null;
    },
    defaultStep: () => ({ kind: 'coerce', to: 'number' }),
  },
  llm_postprocess: {
    kind: 'llm_postprocess',
    label: 'LLM postprocess',
    description:
      'Last-resort provider call. Studio replay refuses to execute this — run a full Debug trace instead.',
    advanced: true,
    validate: step =>
      step.kind === 'llm_postprocess' ? requireString(step.prompt, 'prompt') : 'wrong kind',
    defaultStep: () => ({ kind: 'llm_postprocess', prompt: '', expect: 'text' }),
  },
  mcp_call: {
    kind: 'mcp_call',
    label: 'MCP call',
    description:
      'Call any MCP tool mid-pipeline. Studio replay refuses to execute this — run a full Debug trace instead.',
    advanced: true,
    validate: step => {
      if (step.kind !== 'mcp_call') return 'wrong kind';
      const serverErr = requireString(step.server_id, 'server_id');
      if (serverErr) return serverErr;
      return requireString(step.tool_name, 'tool_name');
    },
    defaultStep: () => ({ kind: 'mcp_call', server_id: '', tool_name: '' }),
  },
};

export const PIPELINE_STEP_KINDS: PipelineStepKind[] = Object.keys(
  PIPELINE_STEP_SPECS,
) as PipelineStepKind[];

/** Validate a whole pipeline. Returns `null` on success or the first
 * `{ index, message }` error so the UI can highlight the row. */
export function validatePipeline(
  steps: PipelineStep[],
): { index: number; message: string } | null {
  for (let i = 0; i < steps.length; i++) {
    const step = steps[i];
    const spec = PIPELINE_STEP_SPECS[step.kind];
    if (!spec) return { index: i, message: `unknown step kind "${step.kind}"` };
    const err = spec.validate(step);
    if (err) return { index: i, message: err };
  }
  return null;
}

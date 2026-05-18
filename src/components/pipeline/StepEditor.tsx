// W32: typed forms for each `PipelineStep` variant. Kept as a fat
// switch on `step.kind` so each form stays inline and readable; the
// Studio component does not need to know variant shapes.

import { useState } from 'react';
import type { AggregateMetric, PipelineStep } from '../../lib/api';
import {
  AGGREGATE_METRIC_OPTIONS,
  COERCE_TARGET_OPTIONS,
  FILTER_OP_OPTIONS,
  SORT_ORDER_OPTIONS,
} from '../../lib/pipeline/registry';

interface Props {
  step: PipelineStep;
  onChange: (next: PipelineStep) => void;
}

const inputCls =
  'w-full rounded-md border border-border bg-background px-2 py-1 text-[12px] font-mono';
const selectCls =
  'rounded-md border border-border bg-background px-2 py-1 text-[12px]';
const labelCls = 'flex flex-col gap-0.5 text-[11px]';
const captionCls = 'mono uppercase tracking-wider text-muted-foreground';

export function StepEditor({ step, onChange }: Props) {
  switch (step.kind) {
    case 'pick':
      return (
        <label className={labelCls}>
          <span className={captionCls}>path</span>
          <input
            className={inputCls}
            value={step.path}
            placeholder="items[*].name"
            onChange={e => onChange({ ...step, path: e.target.value })}
          />
        </label>
      );

    case 'filter':
      return (
        <div className="grid grid-cols-3 gap-2">
          <label className={labelCls}>
            <span className={captionCls}>field</span>
            <input
              className={inputCls}
              value={step.field}
              placeholder="status"
              onChange={e => onChange({ ...step, field: e.target.value })}
            />
          </label>
          <label className={labelCls}>
            <span className={captionCls}>op</span>
            <select
              className={selectCls}
              value={step.op ?? 'eq'}
              onChange={e => onChange({ ...step, op: e.target.value as typeof step.op })}
            >
              {FILTER_OP_OPTIONS.map(op => (
                <option key={op} value={op}>
                  {op}
                </option>
              ))}
            </select>
          </label>
          <ValueEditor
            label="value"
            value={step.value}
            onChange={value => onChange({ ...step, value })}
          />
        </div>
      );

    case 'sort':
      return (
        <div className="grid grid-cols-2 gap-2">
          <label className={labelCls}>
            <span className={captionCls}>by</span>
            <input
              className={inputCls}
              value={step.by}
              placeholder="updated_at"
              onChange={e => onChange({ ...step, by: e.target.value })}
            />
          </label>
          <label className={labelCls}>
            <span className={captionCls}>order</span>
            <select
              className={selectCls}
              value={step.order ?? 'asc'}
              onChange={e =>
                onChange({ ...step, order: e.target.value as typeof step.order })
              }
            >
              {SORT_ORDER_OPTIONS.map(order => (
                <option key={order} value={order}>
                  {order}
                </option>
              ))}
            </select>
          </label>
        </div>
      );

    case 'limit':
      return (
        <label className={labelCls}>
          <span className={captionCls}>count</span>
          <input
            type="number"
            min={0}
            className={inputCls}
            value={Number.isFinite(step.count) ? step.count : 0}
            onChange={e => {
              const next = Number.parseInt(e.target.value, 10);
              onChange({ ...step, count: Number.isFinite(next) ? next : 0 });
            }}
          />
        </label>
      );

    case 'map':
      return <MapEditor step={step} onChange={onChange} />;

    case 'aggregate':
      return <AggregateEditor step={step} onChange={onChange} />;

    case 'set':
      return (
        <div className="grid grid-cols-2 gap-2">
          <label className={labelCls}>
            <span className={captionCls}>field</span>
            <input
              className={inputCls}
              value={step.field}
              placeholder="status"
              onChange={e => onChange({ ...step, field: e.target.value })}
            />
          </label>
          <ValueEditor
            label="value"
            value={step.value}
            onChange={value => onChange({ ...step, value })}
          />
        </div>
      );

    case 'head':
    case 'tail':
    case 'length':
    case 'flatten':
      return (
        <p className="text-[11px] text-muted-foreground">
          No parameters. The step runs as-is on the current value.
        </p>
      );

    case 'unique':
      return (
        <label className={labelCls}>
          <span className={captionCls}>by (optional)</span>
          <input
            className={inputCls}
            value={step.by ?? ''}
            placeholder="id"
            onChange={e => {
              const next = e.target.value.trim();
              if (next) onChange({ ...step, by: next });
              else {
                const { by: _drop, ...rest } = step;
                onChange(rest as typeof step);
              }
            }}
          />
        </label>
      );

    case 'format':
      return (
        <div className="space-y-1">
          <label className={labelCls}>
            <span className={captionCls}>template</span>
            <input
              className={inputCls}
              value={step.template}
              placeholder="{name}: {count}"
              onChange={e => onChange({ ...step, template: e.target.value })}
            />
          </label>
          <label className={labelCls}>
            <span className={captionCls}>output_key (optional)</span>
            <input
              className={inputCls}
              value={step.output_key ?? ''}
              placeholder="label"
              onChange={e => {
                const next = e.target.value.trim();
                if (next) onChange({ ...step, output_key: next });
                else {
                  const { output_key: _drop, ...rest } = step;
                  onChange(rest as typeof step);
                }
              }}
            />
          </label>
        </div>
      );

    case 'coerce':
      return (
        <label className={labelCls}>
          <span className={captionCls}>to</span>
          <select
            className={selectCls}
            value={step.to}
            onChange={e => onChange({ ...step, to: e.target.value as typeof step.to })}
          >
            {COERCE_TARGET_OPTIONS.map(t => (
              <option key={t} value={t}>
                {t}
              </option>
            ))}
          </select>
        </label>
      );

    case 'llm_postprocess':
      return (
        <div className="space-y-1">
          <p className="rounded border border-neon-amber/40 bg-neon-amber/5 px-2 py-1 text-[10px] text-neon-amber">
            Studio replay cannot execute this — run a full traced refresh
            in W23 Debug to inspect the live result.
          </p>
          <label className={labelCls}>
            <span className={captionCls}>prompt</span>
            <textarea
              rows={3}
              className={inputCls}
              value={step.prompt}
              onChange={e => onChange({ ...step, prompt: e.target.value })}
            />
          </label>
          <label className={labelCls}>
            <span className={captionCls}>expect</span>
            <select
              className={selectCls}
              value={step.expect ?? 'text'}
              onChange={e =>
                onChange({ ...step, expect: e.target.value as 'text' | 'json' })
              }
            >
              <option value="text">text</option>
              <option value="json">json</option>
            </select>
          </label>
        </div>
      );

    case 'mcp_call':
      return (
        <div className="space-y-1">
          <p className="rounded border border-neon-amber/40 bg-neon-amber/5 px-2 py-1 text-[10px] text-neon-amber">
            Studio replay cannot execute this — full traced refresh required.
          </p>
          <div className="grid grid-cols-2 gap-2">
            <label className={labelCls}>
              <span className={captionCls}>server_id</span>
              <input
                className={inputCls}
                value={step.server_id}
                onChange={e => onChange({ ...step, server_id: e.target.value })}
              />
            </label>
            <label className={labelCls}>
              <span className={captionCls}>tool_name</span>
              <input
                className={inputCls}
                value={step.tool_name}
                onChange={e => onChange({ ...step, tool_name: e.target.value })}
              />
            </label>
          </div>
          <ValueEditor
            label="arguments (optional)"
            value={step.arguments}
            onChange={value => {
              if (value === undefined) {
                const { arguments: _drop, ...rest } = step;
                onChange(rest as typeof step);
              } else {
                onChange({ ...step, arguments: value });
              }
            }}
          />
        </div>
      );
  }
}

function MapEditor({
  step,
  onChange,
}: {
  step: Extract<PipelineStep, { kind: 'map' }>;
  onChange: (next: PipelineStep) => void;
}) {
  const [fieldsText, setFieldsText] = useState(() =>
    (step.fields ?? []).join(', '),
  );
  const [renameText, setRenameText] = useState(() =>
    JSON.stringify(step.rename ?? {}, null, 2),
  );
  const [renameError, setRenameError] = useState<string | null>(null);

  const commitFields = (raw: string) => {
    setFieldsText(raw);
    const fields = raw
      .split(',')
      .map(s => s.trim())
      .filter(Boolean);
    onChange({ ...step, fields });
  };

  const commitRename = (raw: string) => {
    setRenameText(raw);
    if (!raw.trim()) {
      setRenameError(null);
      onChange({ ...step, rename: {} });
      return;
    }
    try {
      const parsed = JSON.parse(raw);
      if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
        setRenameError(null);
        onChange({ ...step, rename: parsed as Record<string, string> });
      } else {
        setRenameError('rename must be an object');
      }
    } catch (err) {
      setRenameError(err instanceof Error ? err.message : 'invalid JSON');
    }
  };

  return (
    <div className="space-y-1">
      <label className={labelCls}>
        <span className={captionCls}>fields (comma-separated)</span>
        <input
          className={inputCls}
          value={fieldsText}
          placeholder="id, name, status"
          onChange={e => commitFields(e.target.value)}
        />
      </label>
      <label className={labelCls}>
        <span className={captionCls}>rename (JSON object)</span>
        <textarea
          rows={3}
          className={inputCls}
          value={renameText}
          onChange={e => commitRename(e.target.value)}
        />
      </label>
      {renameError && (
        <div className="text-[11px] text-destructive">{renameError}</div>
      )}
    </div>
  );
}

function AggregateEditor({
  step,
  onChange,
}: {
  step: Extract<PipelineStep, { kind: 'aggregate' }>;
  onChange: (next: PipelineStep) => void;
}) {
  const metric = step.metric;
  const needsField = metric.kind !== 'count';
  return (
    <div className="space-y-1">
      <div className="grid grid-cols-3 gap-2">
        <label className={labelCls}>
          <span className={captionCls}>metric</span>
          <select
            className={selectCls}
            value={metric.kind}
            onChange={e => {
              const next = e.target.value as AggregateMetric['kind'];
              if (next === 'count') {
                onChange({ ...step, metric: { kind: 'count' } });
              } else {
                const field = 'field' in metric ? metric.field : '';
                onChange({ ...step, metric: { kind: next, field } });
              }
            }}
          >
            {AGGREGATE_METRIC_OPTIONS.map(m => (
              <option key={m} value={m}>
                {m}
              </option>
            ))}
          </select>
        </label>
        {needsField && 'field' in metric && (
          <label className={labelCls}>
            <span className={captionCls}>field</span>
            <input
              className={inputCls}
              value={metric.field}
              onChange={e =>
                onChange({
                  ...step,
                  metric: { kind: metric.kind, field: e.target.value },
                })
              }
            />
          </label>
        )}
        <label className={labelCls}>
          <span className={captionCls}>group_by (optional)</span>
          <input
            className={inputCls}
            value={step.group_by ?? ''}
            onChange={e => {
              const v = e.target.value.trim();
              if (v) onChange({ ...step, group_by: v });
              else {
                const { group_by: _drop, ...rest } = step;
                onChange(rest as typeof step);
              }
            }}
          />
        </label>
      </div>
      <label className={labelCls}>
        <span className={captionCls}>output_key</span>
        <input
          className={inputCls}
          value={step.output_key ?? 'value'}
          onChange={e =>
            onChange({ ...step, output_key: e.target.value || 'value' })
          }
        />
      </label>
    </div>
  );
}

function ValueEditor({
  label,
  value,
  onChange,
}: {
  label: string;
  value: unknown;
  onChange: (next: unknown) => void;
}) {
  const initial =
    value === undefined
      ? ''
      : typeof value === 'string'
        ? value
        : JSON.stringify(value);
  const [text, setText] = useState(initial);
  const [error, setError] = useState<string | null>(null);

  const commit = (raw: string) => {
    setText(raw);
    const trimmed = raw.trim();
    if (!trimmed) {
      setError(null);
      onChange('');
      return;
    }
    // Treat numbers and booleans/null as their JSON shapes; anything that
    // parses as JSON wins; otherwise keep as raw string.
    try {
      const parsed = JSON.parse(trimmed);
      setError(null);
      onChange(parsed);
    } catch {
      setError(null);
      onChange(raw);
    }
  };

  return (
    <label className={labelCls}>
      <span className={captionCls}>{label}</span>
      <input
        className={inputCls}
        value={text}
        placeholder='"open" or 42 or true'
        onChange={e => commit(e.target.value)}
      />
      {error && <span className="text-[10px] text-destructive">{error}</span>}
    </label>
  );
}

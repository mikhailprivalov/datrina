import { useEffect, useMemo, useState } from 'react';

interface JsonSchema {
  type?: string | string[];
  properties?: Record<string, JsonSchema>;
  required?: string[];
  enum?: unknown[];
  description?: string;
  default?: unknown;
  items?: JsonSchema;
}

interface Props {
  schema: unknown;
  value: Record<string, unknown>;
  onChange: (next: Record<string, unknown>) => void;
}

export function ArgumentsForm({ schema, value, onChange }: Props) {
  const parsed = useMemo<JsonSchema | null>(() => {
    if (!schema || typeof schema !== 'object') return null;
    return schema as JsonSchema;
  }, [schema]);

  const required = useMemo(() => new Set(parsed?.required ?? []), [parsed]);
  const properties = parsed?.properties ?? {};
  const propertyEntries = Object.entries(properties);

  if (!parsed) {
    return (
      <RawJsonEditor
        value={value}
        onChange={onChange}
        label="Tool has no schema — edit arguments as JSON"
      />
    );
  }

  if (propertyEntries.length === 0) {
    return (
      <p className="text-xs text-muted-foreground">
        This tool takes no arguments.
      </p>
    );
  }

  return (
    <div className="space-y-3">
      {propertyEntries.map(([name, prop]) => (
        <FieldRow
          key={name}
          name={name}
          schema={prop}
          required={required.has(name)}
          value={value[name]}
          onChange={next => onChange({ ...value, [name]: next })}
        />
      ))}
    </div>
  );
}

interface FieldProps {
  name: string;
  schema: JsonSchema;
  required: boolean;
  value: unknown;
  onChange: (next: unknown) => void;
}

function FieldRow({ name, schema, required, value, onChange }: FieldProps) {
  const type = normalizeType(schema.type);
  const label = (
    <label className="flex flex-col gap-1 text-xs text-muted-foreground">
      <span className="font-medium text-foreground">
        {name}
        {required && <span className="ml-1 text-destructive">*</span>}
        {type && <span className="ml-2 text-muted-foreground/70">{type}</span>}
      </span>
      {schema.description && (
        <span className="text-[11px] leading-snug text-muted-foreground/80">
          {schema.description}
        </span>
      )}
    </label>
  );

  if (Array.isArray(schema.enum) && schema.enum.length > 0) {
    return (
      <div className="space-y-1">
        {label}
        <select
          value={String(value ?? '')}
          onChange={e => onChange(parsePrimitive(e.target.value, type))}
          className="w-full rounded-md border border-border bg-background px-2 py-1.5 text-sm focus:outline-none focus:ring-1 focus:ring-primary/50"
        >
          <option value="">— select —</option>
          {schema.enum.map((option, index) => (
            <option key={index} value={String(option)}>
              {String(option)}
            </option>
          ))}
        </select>
      </div>
    );
  }

  if (type === 'boolean') {
    return (
      <div className="flex items-center gap-2">
        <input
          type="checkbox"
          checked={Boolean(value)}
          onChange={e => onChange(e.target.checked)}
          className="h-4 w-4"
          id={`field-${name}`}
        />
        <label htmlFor={`field-${name}`} className="text-xs">
          <span className="font-medium text-foreground">{name}</span>
          {schema.description && (
            <span className="ml-2 text-muted-foreground">{schema.description}</span>
          )}
        </label>
      </div>
    );
  }

  if (type === 'number' || type === 'integer') {
    return (
      <div className="space-y-1">
        {label}
        <input
          type="number"
          value={value === undefined || value === null ? '' : String(value)}
          onChange={e => {
            if (e.target.value === '') {
              onChange(undefined);
              return;
            }
            const parsed = type === 'integer' ? parseInt(e.target.value, 10) : Number(e.target.value);
            onChange(Number.isNaN(parsed) ? undefined : parsed);
          }}
          className="w-full rounded-md border border-border bg-background px-2 py-1.5 text-sm font-mono focus:outline-none focus:ring-1 focus:ring-primary/50"
        />
      </div>
    );
  }

  if (type === 'object' || type === 'array') {
    return (
      <div className="space-y-1">
        {label}
        <NestedJsonInput value={value} onChange={onChange} />
      </div>
    );
  }

  return (
    <div className="space-y-1">
      {label}
      <input
        type="text"
        value={value === undefined || value === null ? '' : String(value)}
        onChange={e => onChange(e.target.value === '' ? undefined : e.target.value)}
        className="w-full rounded-md border border-border bg-background px-2 py-1.5 text-sm font-mono focus:outline-none focus:ring-1 focus:ring-primary/50"
      />
    </div>
  );
}

function NestedJsonInput({ value, onChange }: { value: unknown; onChange: (next: unknown) => void }) {
  const [draft, setDraft] = useState(() => stringify(value));
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setDraft(stringify(value));
  }, [value]);

  return (
    <div className="space-y-1">
      <textarea
        value={draft}
        onChange={e => {
          setDraft(e.target.value);
          if (e.target.value.trim() === '') {
            setError(null);
            onChange(undefined);
            return;
          }
          try {
            onChange(JSON.parse(e.target.value));
            setError(null);
          } catch (err) {
            setError(err instanceof Error ? err.message : 'Invalid JSON');
          }
        }}
        rows={4}
        spellCheck={false}
        className="w-full rounded-md border border-border bg-background px-2 py-1.5 text-xs font-mono focus:outline-none focus:ring-1 focus:ring-primary/50"
      />
      {error && <p className="text-[11px] text-destructive">{error}</p>}
    </div>
  );
}

function RawJsonEditor({
  value,
  onChange,
  label,
}: {
  value: Record<string, unknown>;
  onChange: (next: Record<string, unknown>) => void;
  label: string;
}) {
  const [draft, setDraft] = useState(() => stringify(value));
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setDraft(stringify(value));
  }, [value]);
  return (
    <div className="space-y-2">
      <p className="text-xs text-muted-foreground">{label}</p>
      <textarea
        value={draft}
        onChange={e => {
          setDraft(e.target.value);
          if (e.target.value.trim() === '') {
            setError(null);
            onChange({});
            return;
          }
          try {
            const parsed = JSON.parse(e.target.value);
            if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
              onChange(parsed as Record<string, unknown>);
              setError(null);
            } else {
              setError('Expected a JSON object');
            }
          } catch (err) {
            setError(err instanceof Error ? err.message : 'Invalid JSON');
          }
        }}
        rows={6}
        spellCheck={false}
        className="w-full rounded-md border border-border bg-background px-2 py-1.5 text-xs font-mono focus:outline-none focus:ring-1 focus:ring-primary/50"
      />
      {error && <p className="text-[11px] text-destructive">{error}</p>}
    </div>
  );
}

function stringify(value: unknown): string {
  if (value === undefined || value === null) return '';
  if (typeof value === 'string') return value;
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return '';
  }
}

function normalizeType(type: string | string[] | undefined): string | undefined {
  if (!type) return undefined;
  if (Array.isArray(type)) return type.find(t => t !== 'null') ?? type[0];
  return type;
}

function parsePrimitive(raw: string, type: string | undefined): unknown {
  if (raw === '') return undefined;
  if (type === 'number') return Number(raw);
  if (type === 'integer') return parseInt(raw, 10);
  if (type === 'boolean') return raw === 'true';
  return raw;
}

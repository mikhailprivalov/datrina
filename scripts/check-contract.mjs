import { readFileSync } from 'node:fs';

const apiSource = readFileSync('src/lib/api.ts', 'utf8');
const libSource = readFileSync('src-tauri/src/lib.rs', 'utf8');
const pipelineSource = readFileSync('src-tauri/src/models/pipeline.rs', 'utf8');
const validationSource = readFileSync('src-tauri/src/models/validation.rs', 'utf8');

const invoked = new Set(
  [...apiSource.matchAll(/\bcall(?:Nullable|Void)?(?:<[^)]*>)?\(\s*['"`]([a-z_]+)['"`]/g)].map(
    (match) => match[1],
  ),
);

const registered = new Set(
  [...libSource.matchAll(/\$crate::commands::[a-z_]+::([a-z_]+)/g)].map((match) => match[1]),
);

const missingRegistrations = [...invoked].filter((command) => !registered.has(command));
const missingWrappers = [...registered].filter((command) => !invoked.has(command));

const semanticChecks = [
  {
    ok: /configApi[\s\S]*get:\s*\([^)]*\)\s*=>\s*callNullable<string>\('get_config'/.test(apiSource),
    message: 'configApi.get must use nullable success semantics for get_config.',
  },
  {
    ok: /systemApi[\s\S]*openUrl:\s*\([^)]*\)\s*=>\s*callVoid\('open_url'/.test(apiSource),
    message: 'systemApi.openUrl must use void success semantics for open_url.',
  },
  {
    ok: /dashboard_id:\s*dashboardId/.test(apiSource),
    message: 'create_session nested payload must send dashboard_id.',
  },
  {
    ok: /server_id:\s*serverId/.test(apiSource) && /tool_name:\s*toolName/.test(apiSource),
    message: 'call_tool nested payload must send server_id and tool_name.',
  },
];

// W30: nested-enum parity. Rust `#[serde(tag = "kind", rename_all =
// "snake_case")]` enums must have a TypeScript `kind: '<snake>'`
// counterpart, otherwise the frontend silently drops valid runtime
// payloads. We extract Rust variant names from each enum block and
// require a matching `kind: '<snake>'` literal in `api.ts`.
function pascalToSnake(name) {
  return name
    .replace(/([a-z0-9])([A-Z])/g, '$1_$2')
    .replace(/([A-Z]+)([A-Z][a-z])/g, '$1_$2')
    .toLowerCase();
}

function extractRustVariants(source, enumName) {
  const re = new RegExp(`pub\\s+enum\\s+${enumName}\\s*\\{([\\s\\S]*?)\\n\\}`, 'm');
  const match = source.match(re);
  if (!match) return [];
  const body = match[1];
  // Each variant starts on its own line with optional doc comments above.
  // Capture `Variant {`, `Variant(`, or `Variant,` / `Variant\n`.
  const variants = [];
  for (const line of body.split('\n')) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith('//') || trimmed.startsWith('#[')) continue;
    const m = trimmed.match(/^([A-Z][A-Za-z0-9]*)\b/);
    if (m) variants.push(m[1]);
  }
  return [...new Set(variants)];
}

function parityCheck(source, enumName, snakeVariants, missing) {
  for (const snake of snakeVariants) {
    const literal = `kind: '${snake}'`;
    if (!source.includes(literal)) {
      missing.push(`${enumName}::${snake} (expected TS \`${literal}\`)`);
    }
  }
}

const parityMissing = [];
parityCheck(
  apiSource,
  'PipelineStep',
  extractRustVariants(pipelineSource, 'PipelineStep').map(pascalToSnake),
  parityMissing,
);
parityCheck(
  apiSource,
  'ValidationIssue',
  extractRustVariants(validationSource, 'ValidationIssue').map(pascalToSnake),
  parityMissing,
);

const failures = [];

if (parityMissing.length > 0) {
  failures.push(
    `Rust enum variants without a TypeScript twin in src/lib/api.ts: ${parityMissing.join(', ')}`,
  );
}

if (missingRegistrations.length > 0) {
  failures.push(`Frontend invokes are not registered in Rust: ${missingRegistrations.join(', ')}`);
}

if (missingWrappers.length > 0) {
  failures.push(`Rust commands are registered without frontend wrappers: ${missingWrappers.join(', ')}`);
}

for (const check of semanticChecks) {
  if (!check.ok) {
    failures.push(check.message);
  }
}

if (failures.length > 0) {
  console.error('Contract check failed:');
  for (const failure of failures) {
    console.error(`- ${failure}`);
  }
  process.exit(1);
}

console.log(`Contract check passed: ${invoked.size} frontend commands match Rust registrations.`);

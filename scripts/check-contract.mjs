import { readFileSync } from 'node:fs';

const apiSource = readFileSync('src/lib/api.ts', 'utf8');
const libSource = readFileSync('src-tauri/src/lib.rs', 'utf8');

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

const failures = [];

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

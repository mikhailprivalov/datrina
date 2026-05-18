#!/usr/bin/env node
// W33 — acceptance runner.
//
// Orchestrates the static + replay-eval gates that mirror the
// W33_REAL_PROVIDER_ACCEPTANCE_AND_AGENT_EVAL_V2 validation checklist:
//
//   1. JSON parse of tauri.conf.json
//   2. bun run check:contract
//   3. bun run typecheck
//   4. cargo fmt --all --check
//   5. cargo check --workspace --all-targets
//   6. cargo test --test agent_eval (replay lane)
//
// On success, writes a compact JSON + human-readable report. Failures
// point to the exact gate that broke; the report is what an operator
// archives as acceptance evidence for a release cut.
//
// The live-provider lane (cargo test --features expensive_evals ...
// -- --ignored) is intentionally NOT run by default because it
// requires real credentials and costs real money. Pass --include-live
// to opt in.

import { spawn } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));

const args = new Set(process.argv.slice(2));
const includeLive = args.has('--include-live');
const reportPath = (() => {
  const explicit = process.argv.find((a) => a.startsWith('--report='));
  if (explicit) return resolve(repoRoot, explicit.slice('--report='.length));
  return resolve(repoRoot, 'docs/acceptance-report.json');
})();

const gates = [
  {
    name: 'tauri.conf.json parses',
    command: 'node',
    args: [
      '-e',
      "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))",
    ],
  },
  {
    name: 'check:contract',
    command: 'bun',
    args: ['run', 'check:contract'],
  },
  {
    name: 'typecheck',
    command: 'bun',
    args: ['run', 'typecheck'],
  },
  {
    name: 'cargo fmt --check',
    command: 'cargo',
    args: ['fmt', '--all', '--manifest-path', 'src-tauri/Cargo.toml', '--check'],
  },
  {
    name: 'cargo check --all-targets',
    command: 'cargo',
    args: [
      'check',
      '--workspace',
      '--all-targets',
      '--manifest-path',
      'src-tauri/Cargo.toml',
    ],
  },
  {
    name: 'agent_eval replay lane',
    command: 'cargo',
    args: [
      'test',
      '--test',
      'agent_eval',
      '--manifest-path',
      'src-tauri/Cargo.toml',
    ],
  },
];

if (includeLive) {
  gates.push({
    name: 'agent_eval live lane (expensive_evals)',
    command: 'cargo',
    args: [
      'test',
      '--features',
      'expensive_evals',
      '--test',
      'agent_eval',
      '--manifest-path',
      'src-tauri/Cargo.toml',
      '--',
      '--ignored',
    ],
  });
}

const runStarted = Date.now();
const results = [];
let firstFailureIdx = -1;

for (let i = 0; i < gates.length; i += 1) {
  const gate = gates[i];
  process.stdout.write(`▸ ${gate.name}\n`);
  const started = Date.now();
  // eslint-disable-next-line no-await-in-loop
  const { code, signal, stderrTail } = await runChild(gate);
  const durationMs = Date.now() - started;
  const ok = code === 0;
  results.push({
    name: gate.name,
    ok,
    code,
    signal,
    duration_ms: durationMs,
    stderr_tail: ok ? '' : stderrTail,
  });
  process.stdout.write(`  ${ok ? '✓' : '✗'} ${gate.name} (${durationMs}ms)\n`);
  if (!ok && firstFailureIdx === -1) firstFailureIdx = i;
  if (!ok) break;
}

const report = {
  generated_at: new Date(runStarted).toISOString(),
  duration_ms: Date.now() - runStarted,
  include_live: includeLive,
  success: firstFailureIdx === -1,
  first_failure: firstFailureIdx === -1 ? null : results[firstFailureIdx].name,
  gates: results,
};

mkdirSync(dirname(reportPath), { recursive: true });
writeFileSync(reportPath, JSON.stringify(report, null, 2));

const summary = [
  `Acceptance run finished in ${report.duration_ms}ms.`,
  `  success=${report.success}`,
  `  gates_run=${results.length}/${gates.length}`,
  report.first_failure ? `  first_failure=${report.first_failure}` : '',
  `  report=${reportPath}`,
].filter(Boolean).join('\n');
process.stdout.write(`${summary}\n`);

if (!report.success) {
  const failed = results[firstFailureIdx];
  if (failed.stderr_tail) {
    process.stderr.write(`\nLast stderr lines from ${failed.name}:\n${failed.stderr_tail}\n`);
  }
  process.exit(1);
}

async function runChild(gate) {
  return new Promise((resolvePromise) => {
    const child = spawn(gate.command, gate.args, {
      cwd: repoRoot,
      stdio: ['ignore', 'inherit', 'pipe'],
      shell: process.platform === 'win32',
      env: process.env,
    });
    let stderr = '';
    child.stderr.on('data', (chunk) => {
      const text = chunk.toString();
      process.stderr.write(text);
      stderr += text;
      if (stderr.length > 16_384) stderr = stderr.slice(-16_384);
    });
    child.on('close', (code, signal) => {
      const tailLines = stderr.split('\n').slice(-25).join('\n');
      resolvePromise({ code: code ?? -1, signal: signal ?? null, stderrTail: tailLines });
    });
    child.on('error', (err) => {
      stderr += `\n[spawn error] ${err.message}\n`;
      resolvePromise({ code: -1, signal: null, stderrTail: stderr });
    });
  });
}

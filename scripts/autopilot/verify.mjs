#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const cwd = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const args = new Set(process.argv.slice(2));
const includeRealApp = !args.has('--skip-real-app');
const runId = new Date().toISOString().replace(/[:.]/g, '-');
const runDir = resolve(process.env.DATRINA_AUTOPILOT_RUN_DIR || join(tmpdir(), `datrina-verify-${runId}`));
mkdirSync(runDir, { recursive: true });

const checks = [
  {
    name: 'tauri-config-json',
    command: process.execPath,
    args: ['-e', "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"],
    timeoutMs: 30_000,
  },
  { name: 'contract', command: 'bun', args: ['run', 'check:contract'], timeoutMs: 60_000 },
  { name: 'typecheck', command: 'bun', args: ['run', 'typecheck'], timeoutMs: 120_000 },
  { name: 'build', command: 'bun', args: ['run', 'build'], timeoutMs: 180_000 },
  { name: 'cargo-fmt-check', command: 'cargo', args: ['fmt', '--all', '--check'], timeoutMs: 120_000 },
  { name: 'cargo-check', command: 'cargo', args: ['check', '--workspace', '--all-targets'], timeoutMs: 240_000 },
];

if (includeRealApp) {
  checks.push({
    name: 'real-tauri-app-smoke',
    command: process.execPath,
    args: ['scripts/autopilot/real-app-smoke.mjs'],
    timeoutMs: Number(process.env.DATRINA_E2E_TIMEOUT_MS || 300_000),
  });
}

const results = [];

for (const check of checks) {
  console.log(`\n==> ${check.name}`);
  const result = await runCheck(check);
  results.push(result);
  writeFileSync(join(runDir, `${check.name}.log`), result.output);
  if (result.code !== 0 || result.timedOut) {
    writeSummary(results);
    process.exit(result.code || 1);
  }
}

writeSummary(results);
console.log(`\nAll checks passed. Logs: ${runDir}`);

function runCheck(check) {
  return new Promise(resolveCheck => {
    const child = spawn(check.command, check.args, {
      cwd,
      env: {
        ...process.env,
        DATRINA_AUTOPILOT_RUN_DIR: join(runDir, check.name),
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    });

    let output = '';
    const append = chunk => {
      const text = chunk.toString();
      output += text;
      process.stdout.write(text);
    };
    child.stdout.on('data', append);
    child.stderr.on('data', append);

    const timeout = setTimeout(() => {
      child.kill('SIGTERM');
      setTimeout(() => child.kill('SIGKILL'), 5_000).unref();
      resolveCheck({ ...check, code: 1, timedOut: true, output });
    }, check.timeoutMs);

    child.on('exit', code => {
      clearTimeout(timeout);
      resolveCheck({ ...check, code: code ?? 1, timedOut: false, output });
    });
  });
}

function writeSummary(results) {
  writeFileSync(join(runDir, 'summary.json'), JSON.stringify({
    ok: results.every(result => result.code === 0 && !result.timedOut),
    runDir,
    results: results.map(({ name, command, args, code, timedOut, timeoutMs }) => ({
      name,
      command,
      args,
      code,
      timedOut,
      timeoutMs,
    })),
  }, null, 2));
}

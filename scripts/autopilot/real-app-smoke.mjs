#!/usr/bin/env node
import { spawn, spawnSync } from 'node:child_process';
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const cwd = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const runId = new Date().toISOString().replace(/[:.]/g, '-');
const runDir = resolve(process.env.DATRINA_AUTOPILOT_RUN_DIR || join(tmpdir(), `datrina-e2e-${runId}`));
const appDataDir = join(runDir, 'app-data');
const reportPath = join(runDir, 'startup-smoke-report.json');
const logPath = join(runDir, 'tauri-dev.log');
const timeoutMs = Number(process.env.DATRINA_E2E_TIMEOUT_MS || 240_000);

mkdirSync(runDir, { recursive: true });
rmSync(appDataDir, { recursive: true, force: true });
mkdirSync(appDataDir, { recursive: true });

const child = spawn('bun', ['run', 'tauri:dev'], {
  cwd,
  env: {
    ...process.env,
    DATRINA_APP_DATA_DIR: appDataDir,
    DATRINA_E2E_REPORT: reportPath,
    RUST_LOG: process.env.RUST_LOG || 'datrina=info,tauri=info',
  },
  stdio: ['ignore', 'pipe', 'pipe'],
});

let output = '';
const append = chunk => {
  output += chunk.toString();
  writeFileSync(logPath, output);
};
child.stdout.on('data', append);
child.stderr.on('data', append);

const result = await new Promise(resolveResult => {
  const timeout = setTimeout(() => {
    child.kill('SIGTERM');
    setTimeout(() => child.kill('SIGKILL'), 5_000).unref();
    resolveResult({ timedOut: true, code: null });
  }, timeoutMs);

  child.on('exit', code => {
    clearTimeout(timeout);
    resolveResult({ timedOut: false, code });
  });
});

if (result.timedOut) {
  console.error(`real-app smoke timed out after ${timeoutMs} ms`);
  console.error(`log: ${logPath}`);
  process.exit(1);
}

if (result.code !== 0) {
  console.error(`tauri dev exited with code ${result.code}`);
  console.error(`log: ${logPath}`);
  process.exit(result.code ?? 1);
}

let report;
try {
  report = JSON.parse(readFileSync(reportPath, 'utf8'));
} catch (error) {
  console.error(`startup smoke report was not written: ${reportPath}`);
  console.error(error instanceof Error ? error.message : String(error));
  console.error(`log: ${logPath}`);
  process.exit(1);
}

const dbPath = join(appDataDir, 'app.db');
const dbCheck = spawnSync(
  'sqlite3',
  ['--readonly', dbPath, 'SELECT COUNT(*) FROM dashboards; SELECT COUNT(*) FROM workflow_runs;'],
  { encoding: 'utf8' },
);

if (dbCheck.status !== 0) {
  console.error(`sqlite readonly verification failed for ${dbPath}`);
  console.error(dbCheck.stderr.trim());
  process.exit(dbCheck.status ?? 1);
}

const [dashboardCount, workflowRunCount] = dbCheck.stdout
  .trim()
  .split(/\s+/)
  .map(value => Number(value));

const ok = report.success === true
  && report.output_value === 72
  && dashboardCount >= 1
  && workflowRunCount >= 1;

console.log(JSON.stringify({
  ok,
  runDir,
  appDataDir,
  reportPath,
  logPath,
  report,
  dashboardCount,
  workflowRunCount,
}, null, 2));

process.exit(ok ? 0 : 1);

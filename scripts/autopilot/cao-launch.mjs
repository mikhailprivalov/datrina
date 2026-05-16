#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const cwd = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const message = process.argv.slice(2).join(' ').trim()
  || 'Run the next narrow Datrina task from the current user request. Respect AGENTS.md and run node scripts/autopilot/verify.mjs before reporting completion.';
const sessionName = process.env.CAO_SESSION_NAME || `datrina-${new Date().toISOString().replace(/[:.]/g, '-')}`;

const result = spawnSync('cao', [
  'launch',
  '--agents', 'datrina_developer',
  '--provider', 'claude_code',
  '--session-name', sessionName,
  '--working-directory', cwd,
  '--headless',
  '--async',
  '--auto-approve',
  message,
], {
  cwd,
  stdio: 'inherit',
});

process.exit(result.status ?? 1);

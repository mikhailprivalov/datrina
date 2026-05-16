#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const cwd = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const profile = resolve(cwd, '.cao/agents/datrina_developer.md');
const result = spawnSync('cao', ['install', profile, '--provider', 'claude_code'], {
  cwd,
  stdio: 'inherit',
});
process.exit(result.status ?? 1);

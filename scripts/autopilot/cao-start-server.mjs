#!/usr/bin/env node
import { spawn, spawnSync } from 'node:child_process';
import { mkdirSync, openSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const cwd = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const logDir = resolve(cwd, '.autopilot');
mkdirSync(logDir, { recursive: true });

const probe = spawnSync('cao', ['session', 'list'], { encoding: 'utf8' });
if (probe.status === 0) {
  console.log('CAO server is already reachable.');
  process.exit(0);
}

const out = openSync(resolve(logDir, 'cao-server.log'), 'a');
const child = spawn('cao-server', ['--host', '127.0.0.1', '--port', '9889'], {
  cwd,
  detached: true,
  stdio: ['ignore', out, out],
});
child.unref();
console.log(`Started cao-server pid=${child.pid}; log=${resolve(logDir, 'cao-server.log')}`);

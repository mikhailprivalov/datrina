#!/usr/bin/env node
import { spawn, spawnSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const cwd = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const startedAt = new Date();
const runId = startedAt.toISOString().replace(/[:.]/g, '-');
const queueDir = resolve(cwd, '.autopilot', 'queue', runId);
mkdirSync(queueDir, { recursive: true });

const options = parseArgs(process.argv.slice(2));
const tasks = options.tasks.length > 0 ? options.tasks : rangeTasks(17, 25);

if (options.dryRun) {
  console.log(JSON.stringify({
    dryRun: true,
    tasks,
    maxRepairs: options.maxRepairs,
    taskTimeoutMinutes: options.taskTimeoutMinutes,
    skipRealApp: options.skipRealApp,
    queueDir,
  }, null, 2));
  process.exit(0);
}

console.log(`CAO queue run: ${runId}`);
console.log(`Tasks: ${tasks.join(', ')}`);
console.log(`Logs: ${queueDir}`);

if (!options.noStartServer) {
  runChecked('node', ['scripts/autopilot/cao-start-server.mjs'], { name: 'cao-start-server' });
}
if (!options.noInstallProfile) {
  runChecked('node', ['scripts/autopilot/cao-install-profile.mjs'], { name: 'cao-install-profile' });
}

const summary = {
  ok: false,
  runId,
  queueDir,
  tasks: [],
};

for (const task of tasks) {
  const taskResult = await runTask(task, options);
  summary.tasks.push(taskResult);
  writeFileSync(resolve(queueDir, 'summary.json'), JSON.stringify(summary, null, 2));

  if (!taskResult.ok) {
    console.error(`Queue stopped at ${task}. See ${taskResult.taskDir}`);
    process.exit(1);
  }
}

summary.ok = true;
writeFileSync(resolve(queueDir, 'summary.json'), JSON.stringify(summary, null, 2));
console.log(`Queue completed: ${tasks.join(', ')}`);

async function runTask(task, opts) {
  const taskDir = resolve(queueDir, task);
  mkdirSync(taskDir, { recursive: true });
  const sessionStem = `${opts.sessionPrefix}-${task.toLowerCase()}-${Date.now()}`;
  const message = [
    `Execute ${task} from docs/RECONCILIATION_PLAN.md.`,
    'Read AGENTS.md and the dedicated workstream doc first.',
    'Keep the work strictly inside the requested workstream scope.',
    'Do not commit or push.',
    'When implementation is done, run node scripts/autopilot/verify.mjs and report exact results.',
  ].join(' ');

  console.log(`\n=== ${task}: launch ${sessionStem}`);
  const launch = runCapture('cao', [
    'launch',
    '--agents', 'datrina_developer',
    '--provider', 'claude_code',
    '--session-name', sessionStem,
    '--working-directory', cwd,
    '--headless',
    '--async',
    '--auto-approve',
    message,
  ], { timeoutMs: 180_000 });
  writeFileSync(resolve(taskDir, 'launch.log'), launch.output);
  if (launch.code !== 0) {
    return { task, ok: false, stage: 'launch', taskDir, output: tail(launch.output) };
  }

  const sessionName = parseSessionName(launch.output, sessionStem);
  console.log(`${task}: session ${sessionName}`);

  const firstWait = await waitForSession(sessionName, taskDir, opts);
  if (!firstWait.ok) {
    return { task, ok: false, stage: 'agent', sessionName, taskDir, wait: firstWait };
  }

  for (let attempt = 0; attempt <= opts.maxRepairs; attempt += 1) {
    const verify = runCapture(
      'node',
      ['scripts/autopilot/verify.mjs', ...(opts.skipRealApp ? ['--skip-real-app'] : [])],
      { timeoutMs: opts.verifyTimeoutMinutes * 60_000 },
    );
    writeFileSync(resolve(taskDir, `verify-${attempt}.log`), verify.output);

    if (verify.code === 0) {
      console.log(`${task}: verifier passed`);
      return {
        task,
        ok: true,
        sessionName,
        taskDir,
        repairsUsed: attempt,
      };
    }

    if (attempt >= opts.maxRepairs) {
      return {
        task,
        ok: false,
        stage: 'verify',
        sessionName,
        taskDir,
        repairsUsed: attempt,
        verifierTail: tail(verify.output),
      };
    }

    console.log(`${task}: verifier failed; sending repair ${attempt + 1}/${opts.maxRepairs}`);
    const repairMessage = [
      `Verifier failed after ${task}.`,
      'Fix only the reported failures within the same workstream boundary.',
      'Do not broaden scope, do not commit or push.',
      'Verifier output tail:',
      tail(verify.output),
    ].join('\n\n');

    const send = runCapture('cao', ['session', 'send', sessionName, repairMessage, '--async'], {
      timeoutMs: 60_000,
    });
    writeFileSync(resolve(taskDir, `repair-${attempt + 1}-send.log`), send.output);
    if (send.code !== 0) {
      return {
        task,
        ok: false,
        stage: 'repair-send',
        sessionName,
        taskDir,
        output: tail(send.output),
      };
    }

    const repairWait = await waitForSession(sessionName, taskDir, opts);
    if (!repairWait.ok) {
      return {
        task,
        ok: false,
        stage: 'repair-agent',
        sessionName,
        taskDir,
        wait: repairWait,
      };
    }
  }

  return { task, ok: false, stage: 'unknown', sessionName, taskDir };
}

async function waitForSession(sessionName, taskDir, opts) {
  const deadline = Date.now() + opts.taskTimeoutMinutes * 60_000;
  let sawProcessing = false;
  let idleWithOutputCount = 0;
  let lastStatusJson = null;

  while (Date.now() < deadline) {
    const status = runCapture('cao', ['session', 'status', sessionName, '--json'], {
      timeoutMs: 30_000,
    });
    writeFileSync(resolve(taskDir, 'last-status.json'), status.output);
    if (status.code !== 0) {
      return { ok: false, reason: 'status-command-failed', output: tail(status.output) };
    }

    let parsed;
    try {
      parsed = JSON.parse(status.output);
    } catch {
      return { ok: false, reason: 'status-json-invalid', output: tail(status.output) };
    }
    lastStatusJson = parsed;

    const conductor = parsed.conductor || {};
    const state = conductor.status;
    const lastOutput = String(conductor.last_output || '');
    console.log(`${sessionName}: ${state}`);

    if (state === 'processing') {
      sawProcessing = true;
      idleWithOutputCount = 0;
    } else if (state === 'completed') {
      return { ok: true, terminalStatus: state };
    } else if (state === 'error' || state === 'waiting_user_answer') {
      return { ok: false, reason: state, lastOutput: tail(lastOutput) };
    } else if (state === 'idle' && sawProcessing && lastOutput.trim()) {
      idleWithOutputCount += 1;
      if (idleWithOutputCount >= 2) {
        return { ok: true, terminalStatus: state };
      }
    }

    await sleep(opts.pollSeconds * 1000);
  }

  return {
    ok: false,
    reason: 'timeout',
    lastStatus: lastStatusJson,
  };
}

function runChecked(command, args, { name }) {
  const result = runCapture(command, args, { timeoutMs: 120_000 });
  writeFileSync(resolve(queueDir, `${name}.log`), result.output);
  if (result.code !== 0) {
    console.error(result.output);
    process.exit(result.code || 1);
  }
}

function runCapture(command, args, { timeoutMs }) {
  const child = spawnSync(command, args, {
    cwd,
    encoding: 'utf8',
    timeout: timeoutMs,
  });
  return {
    code: child.status ?? (child.signal ? 1 : 0),
    output: `${child.stdout || ''}${child.stderr || ''}`,
    signal: child.signal,
  };
}

function parseArgs(args) {
  const opts = {
    tasks: [],
    dryRun: false,
    skipRealApp: false,
    noStartServer: false,
    noInstallProfile: false,
    maxRepairs: 2,
    pollSeconds: 20,
    taskTimeoutMinutes: 90,
    verifyTimeoutMinutes: 20,
    sessionPrefix: 'datrina',
  };

  for (const arg of args) {
    if (arg === '--dry-run') opts.dryRun = true;
    else if (arg === '--skip-real-app') opts.skipRealApp = true;
    else if (arg === '--no-start-server') opts.noStartServer = true;
    else if (arg === '--no-install-profile') opts.noInstallProfile = true;
    else if (arg.startsWith('--max-repairs=')) opts.maxRepairs = numberOption(arg, 0);
    else if (arg.startsWith('--poll-seconds=')) opts.pollSeconds = numberOption(arg, 5);
    else if (arg.startsWith('--task-timeout-minutes=')) opts.taskTimeoutMinutes = numberOption(arg, 5);
    else if (arg.startsWith('--verify-timeout-minutes=')) opts.verifyTimeoutMinutes = numberOption(arg, 5);
    else if (arg.startsWith('--session-prefix=')) opts.sessionPrefix = arg.split('=').slice(1).join('=').trim() || opts.sessionPrefix;
    else if (/^W\d+[a-z]?$/i.test(arg)) opts.tasks.push(arg.toUpperCase());
    else if (/^W\d+\.\.W\d+$/i.test(arg)) {
      const [from, to] = arg.toUpperCase().split('..').map(value => Number(value.slice(1)));
      opts.tasks.push(...rangeTasks(from, to));
    } else {
      throw new Error(`Unknown queue argument: ${arg}`);
    }
  }

  return opts;
}

function numberOption(arg, min) {
  const value = Number(arg.split('=').slice(1).join('='));
  if (!Number.isFinite(value) || value < min) {
    throw new Error(`Invalid numeric option: ${arg}`);
  }
  return value;
}

function rangeTasks(from, to) {
  const tasks = [];
  for (let n = from; n <= to; n += 1) tasks.push(`W${n}`);
  return tasks;
}

function parseSessionName(output, requestedStem) {
  const match = output.match(/Session created:\s*(\S+)/);
  if (match) return match[1];
  return requestedStem.startsWith('cao-') ? requestedStem : `cao-${requestedStem}`;
}

function tail(text, max = 12_000) {
  return text.length > max ? text.slice(text.length - max) : text;
}

function sleep(ms) {
  return new Promise(resolveSleep => setTimeout(resolveSleep, ms));
}

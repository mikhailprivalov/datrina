# CAO Autopilot

Status: local maintainer setup

## Purpose

This is the local autonomous execution harness for `datrina`. CAO orchestrates
Claude Code sessions; deterministic scripts decide pass/fail.

The loop is:

1. install/start CAO,
2. launch `datrina_developer` in this repository,
3. let Claude Code implement one narrow task,
4. run `node scripts/autopilot/verify.mjs`,
5. if verification fails, send the exact failure log back to the same CAO
   session for one bounded repair pass.

## Installed Tools

Required locally:

- `tmux`
- `uv`
- `cao`
- `cao-server`
- `claude`
- `sqlite3`
- `bun`
- Rust/Cargo toolchain

This setup uses CAO with provider `claude_code`.

## Commands

Start the CAO server:

```bash
node scripts/autopilot/cao-start-server.mjs
```

Install the Datrina Claude Code worker profile:

```bash
node scripts/autopilot/cao-install-profile.mjs
```

Launch a headless CAO session:

```bash
node scripts/autopilot/cao-launch.mjs "Execute W17 from docs/RECONCILIATION_PLAN.md"
```

Run the overnight queue. With no task arguments it runs `W17..W25`, one task at
a time, and stops on the first unresolved verifier failure:

```bash
node scripts/autopilot/run-queue.mjs
```

Run a narrower queue:

```bash
node scripts/autopilot/run-queue.mjs W17 W18 W19
node scripts/autopilot/run-queue.mjs W17..W20
```

Preview without launching agents:

```bash
node scripts/autopilot/run-queue.mjs --dry-run W17..W25
```

Inspect sessions:

```bash
cao session list
tmux ls
```

Attach to a session:

```bash
tmux attach -t cao-<session-name>
```

Run deterministic verification directly:

```bash
node scripts/autopilot/verify.mjs
```

Run only static checks, without the real app launch smoke:

```bash
node scripts/autopilot/verify.mjs --skip-real-app
```

Run the real Tauri app smoke directly:

```bash
node scripts/autopilot/real-app-smoke.mjs
```

## Real App Smoke

`real-app-smoke.mjs` launches the real app with:

- `bun run tauri:dev`
- isolated app data directory via `DATRINA_APP_DATA_DIR`
- startup e2e report path via `DATRINA_E2E_REPORT`

When `DATRINA_E2E_REPORT` is set, the Rust app starts normally, initializes
storage, creates a local MVP dashboard, executes its workflow through the real
workflow engine, saves the workflow run, writes a JSON report, and exits.

The smoke then verifies:

- the Tauri app process exited successfully,
- the report says `success: true`,
- the deterministic workflow output is `72`,
- the isolated SQLite DB contains at least one dashboard and one workflow run.

This intentionally avoids the user's runtime DB. It writes under a temporary
directory unless `DATRINA_AUTOPILOT_RUN_DIR` is set.

## Boundaries

- CAO is an external maintainer runner, not a Datrina product feature.
- `run-queue.mjs` launches one CAO session per W task, runs the verifier after
  each session, sends at most two repair prompts to the same session, and stops
  on the first unresolved failure.
- The queue does not commit, push, or merge. Review the diff after each run.
- The app runtime only observes `DATRINA_APP_DATA_DIR` and
  `DATRINA_E2E_REPORT` when those env vars are explicitly set.
- Do not use `--yolo` by default. The provided launcher uses
  `--auto-approve`, but keeps CAO's profile-level tool restrictions.
- The macOS smoke is not WebDriver. Official Tauri WebDriver support does not
  provide a macOS WKWebView desktop driver, so this harness verifies the real
  app process and Rust/Tauri runtime path first. UI-driving can be added later
  with a dedicated accessibility or in-app test bridge.

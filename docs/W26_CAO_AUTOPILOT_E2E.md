# W26 CAO Autopilot E2E

Status: implemented locally

Date: 2026-05-16

## Context

The project needs an autonomous maintainer loop that can run Claude Code,
verify the repo, launch the real Tauri app, and iterate on failures without
turning Datrina itself into a self-modifying product runtime.

## Goal

Set up CAO as the external orchestrator for Claude Code and add deterministic
verification scripts with a real `bun run tauri:dev` startup smoke.

## Approach

- Install CAO and `tmux` on the local machine.
- Add a CAO `datrina_developer` profile scoped to this repo's `AGENTS.md`
  and reconciliation workstream rules.
- Add a verifier script that runs contract/type/build/Cargo checks.
- Add a real-app smoke that launches the Tauri app with an isolated app data
  directory and an env-gated startup e2e report.
- Add an overnight queue runner that launches W tasks one at a time, runs the
  verifier after each session, and stops on the first unresolved failure.
- Keep the pass/fail decision in scripts, not in the LLM prompt.

## Files

- `.cao/agents/datrina_developer.md`
- `scripts/autopilot/cao-start-server.mjs`
- `scripts/autopilot/cao-install-profile.mjs`
- `scripts/autopilot/cao-launch.mjs`
- `scripts/autopilot/run-queue.mjs`
- `scripts/autopilot/verify.mjs`
- `scripts/autopilot/real-app-smoke.mjs`
- `docs/CAO_AUTOPILOT.md`
- `src-tauri/src/modules/storage.rs`
- `src-tauri/src/commands/dashboard.rs`
- `src-tauri/src/lib.rs`

## Validation

- `tmux -V` reports an installed tmux.
- `cao --help` and `cao-server --help` work.
- `node scripts/autopilot/real-app-smoke.mjs` launches the real Tauri app,
  writes a report, and verifies the isolated SQLite DB.
- `node scripts/autopilot/verify.mjs --skip-real-app` runs static validation.
- `node scripts/autopilot/verify.mjs` runs static validation plus the real
  app smoke.
- `node scripts/autopilot/run-queue.mjs --dry-run W17..W25` prints the planned
  overnight queue without launching agents.

## Out of scope

- WebDriver UI automation on macOS.
- Unbounded autonomous repair loops.
- `--yolo` CAO sessions.
- Real external provider credentials or external MCP servers.

## Related

- `docs/CAO_AUTOPILOT.md`
- `docs/W24_AGENT_EVAL_SUITE.md`
- `AGENTS.md`

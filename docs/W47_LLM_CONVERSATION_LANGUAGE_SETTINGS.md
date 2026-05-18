# W47 LLM Conversation Language Settings

Status: shipped (v1)

Date: 2026-05-17

## Context

Users need a visible way to choose the language the assistant uses in normal
chat, Build Chat, and LLM-backed dashboard/widget text. Today language behavior
is implicit: the provider usually follows the user's prompt language, but there
is no app-level contract, persisted preference, or provider prompt rule that
keeps GPT, Claude, and Kimi responses consistent across sessions and generated
runtime text.

Provider language support is not exposed as one exact shared enum across GPT,
Claude, and Kimi. W47 should therefore implement a curated major-language
catalog with stable BCP-47 identifiers, keep provider-specific support metadata
explicit, and avoid claiming identical quality for every provider/language
combination.

## Goal

- Users can choose assistant language globally and, where useful, override it
  per dashboard/chat context without editing prompts manually.
- The default behavior remains `auto`: follow the user's latest natural
  language unless an explicit language is selected.
- The explicit language catalog covers the main GPT/Claude/Kimi practical
  languages at minimum:
  - `en` English
  - `ru` Russian
  - `zh-Hans` Chinese, Simplified
  - `zh-Hant` Chinese, Traditional
  - `ja` Japanese
  - `ko` Korean
  - `es` Spanish
  - `fr` French
  - `de` German
  - `pt` Portuguese
  - `it` Italian
  - `nl` Dutch
  - `pl` Polish
  - `uk` Ukrainian
  - `tr` Turkish
  - `ar` Arabic
  - `he` Hebrew
  - `hi` Hindi
  - `bn` Bengali
  - `ur` Urdu
  - `id` Indonesian
  - `vi` Vietnamese
  - `th` Thai
  - `ms` Malay
  - `cs` Czech
  - `el` Greek
  - `sv` Swedish
  - `no` Norwegian
  - `da` Danish
  - `fi` Finnish
- Rust-owned prompt assembly injects the selected language policy consistently
  for chat, Build Chat, LLM postprocess steps, and LLM-backed text widgets.
- The UI shows the effective language source: auto, app default, dashboard
  override, chat/session override, or widget/runtime override.
- Unsupported or low-confidence language/provider combinations are surfaced as
  typed warnings or unsupported states, not fake success.

## Approach

1. Define the language policy model.
   - Add `AssistantLanguagePolicy` and `AssistantLanguageOption` shapes in Rust,
     mirrored in TypeScript.
   - Store BCP-47 language tags, display names, direction (`ltr`/`rtl`), and
     provider support hints.
   - Support at least `auto`, app default, dashboard override, chat/session
     override, and widget/runtime override.

2. Add the catalog and provider metadata.
   - Keep the initial catalog static and local-first.
   - Mark GPT, Claude, and Kimi support as practical prompt-level support unless
     provider docs expose a stricter capability flag.
   - Refresh provider documentation during W47 execution before freezing the
     exact catalog labels and support hints.
   - Do not block normal use only because a provider lacks a formal language
     enum; warn when support is uncertain.

3. Persist and resolve effective language.
   - Persist the app default language in Rust-owned settings storage.
   - Persist dashboard/session/widget overrides where those scopes already have
     durable config.
   - Resolve effective language in one shared Rust helper so chat, Build Chat,
     and runtime LLM paths do not drift.

4. Inject language into provider prompts.
   - Add a short, high-priority system instruction such as "Respond in Russian"
     or "Respond in the user's language" from the resolved policy.
   - Preserve tool/schema instructions and do not localize JSON keys, command
     names, widget ids, datasource ids, or validation issue codes.
   - Keep translation behavior separate from datasource values unless the user
     explicitly asks to translate content.

5. Build the settings UX.
   - Add a compact language selector near provider/model settings and expose
     contextual overrides where the app already has dashboard/chat settings.
   - Show RTL-aware rendering for Arabic/Hebrew assistant text where the UI
     container supports it.
   - Display effective language provenance in relevant details panels.

6. Extend validation and eval coverage.
   - Add replay eval cases that verify language-policy prompt injection without
     requiring live credentials.
   - Add live optional smoke lanes for GPT, Claude, and Kimi when credentials are
     available.
   - Include at least English, Russian, Simplified Chinese, Spanish, French,
     German, Japanese, Korean, Arabic, and Portuguese in manual/live smoke
     coverage.

## Files

- `src-tauri/src/models/provider.rs`
- `src-tauri/src/models/dashboard.rs`
- `src-tauri/src/models/chat.rs`
- `src-tauri/src/models/widget.rs`
- `src-tauri/src/modules/ai.rs`
- `src-tauri/src/modules/storage.rs`
- `src-tauri/src/commands/provider.rs`
- `src-tauri/src/commands/chat.rs`
- `src-tauri/src/commands/dashboard.rs`
- `src/lib/api.ts`
- `src/App.tsx`
- `src/components/layout/ProviderSettings.tsx`
- `src/components/layout/ChatPanel.tsx`
- `src/components/widgets/*`
- `src-tauri/tests/agent_eval.rs`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W47_LLM_CONVERSATION_LANGUAGE_SETTINGS.md`

## Validation

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`
- `bun run check:contract`
- `bun run typecheck`
- `bun run build`
- `cargo fmt --all --check` or targeted `rustfmt --edition 2021` for changed
  Rust files if unrelated format drift exists.
- `cargo check --workspace --all-targets`
- `bun run eval`
- `bun run acceptance`
- Unit or integration checks for:
  - language catalog parity between Rust and TypeScript,
  - app default persistence,
  - dashboard/session/widget override persistence where implemented,
  - effective language resolution order,
  - prompt injection for chat, Build Chat, LLM postprocess, and LLM-backed text
    widgets,
  - no localization of schema keys, tool names, ids, or validation issue codes,
  - RTL rendering metadata for Arabic/Hebrew messages.
- Manual running-app smoke:
  - select `auto` and confirm the assistant follows Russian and English user
    prompts,
  - select Russian and confirm GPT/Claude/Kimi-backed chat responses stay in
    Russian when credentials/providers are available,
  - select Simplified Chinese, Japanese, Arabic, and Portuguese and confirm the
    visible assistant text follows the selected language,
  - confirm Build Chat proposal JSON remains schema-compatible while preview
    text follows the selected language,
  - confirm changing language does not expose provider secrets to React state.

## Out of scope

- Full UI localization/i18n for the whole application shell.
- Automatic translation of datasource values, uploaded documents, or external
  tool results unless the user explicitly asks the LLM to translate them.
- Per-user/team language policy and RBAC.
- Provider-side fine-tuning or language quality benchmarking.
- Voice input/output language selection.
- Claiming exhaustive official language support for every GPT/Claude/Kimi
  model variant.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W14_CHAT_STREAMING_TRACE_UI.md`
- `docs/W15_CHAT_RUNTIME_REPLACEMENT.md`
- `docs/W22_TOKEN_COST_TRACKING.md`
- `docs/W29_REAL_PROVIDER_RUNTIME_GATE.md`
- `docs/W33_REAL_PROVIDER_ACCEPTANCE_AND_AGENT_EVAL_V2.md`
- `docs/W41_WIDGET_EXECUTION_OBSERVABILITY.md`
- `docs/W42_WIDGET_STREAMING_REASONING.md`
- `docs/W43_DASHBOARD_WIDGET_MODEL_SELECTION.md`

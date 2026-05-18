# W37 External Open Source And Free-Use Source Catalog

Status: shipped (HTTP/JSON adapters)

Date: 2026-05-17

## Delivered

- `ExternalSource` model + reviewed catalog seed (9 entries:
  `hacker_news_search`, `wikipedia_summary`, `coingecko_price`,
  `github_public_repo`, `defillama_protocol`, `mediawiki_search`,
  `stackexchange_search`, `hn_top_stories`,
  `brave_search_web` shipped `needs_review` until per-plan rate-limits
  surface in UI).
- `external_source_state` table + 6 commands: `list_external_sources`,
  `set_external_source_enabled`, `set_external_source_credential`,
  `test_external_source`, `save_external_source_as_datasource`,
  `preview_external_source_impact`.
- Enabled runnable sources are injected into the chat tool list as
  typed `source_<id>` tool specs (streaming + silent retry paths).
  Disabled / blocked / credential-missing sources fail closed before
  any network I/O.
- "Save as datasource" creates a regular `DatasourceDefinition` so the
  saved request flows through the same workflow engine widgets consume.
  Sources with `credential_policy == Required` are rejected to avoid
  baking real keys into workflow JSON.
- **BYOK at runtime**: saved external-source datasources carry a
  `_external_source_id` marker on the http_request node. At workflow
  execution time `WorkflowEngine.with_storage` resolves the current
  credential from `external_source_state` and injects the catalog-defined
  header — the credential never sits in the workflow JSON, so scheduled
  refreshes and live runs both see the latest user-managed key.
- `DatasourceDefinition.originated_external_source_id` tracks which
  catalog entry a saved datasource was generated from. Workbench shows
  a "from catalog: <id>" badge; Source Catalog shows the originating
  datasources and confirms before disabling a source that still has
  active consumers.
- React `#/sources` route with a Sidebar entry. License/terms metadata,
  attribution, review notes, originating datasources, and an inline
  Test runner all render verbatim.

## Test coverage

- `cargo test --lib` (105 passed) — adds:
  - `external_source_state_credential_round_trip`
  - `catalog_is_non_empty_and_ids_are_unique`
  - `every_runnable_entry_has_a_review_date`
  - `required_credential_entries_declare_a_header`
  - `tool_name_is_prefixed_and_round_trips_via_parser`
  - `substitute_tokens_replaces_known_args_and_drops_unknown`
  - `build_http_call_for_save_omits_credential_and_skips_blank_query_values`
  - `build_http_call_rejects_missing_required_param`
  - `build_http_call_with_credential_injects_header_with_prefix`
  - `parse_external_source_tool_name_recognises_prefix`
- `cargo test --test agent_eval` (5 passed) — no regression.
- `bun run check:contract` (107 commands) / `bun run typecheck` /
  `bun run build` — clean.

## W37++: out-of-scope items closed

The original v1 left three items deferred. All three are now closed via
catalog metadata + a new safe adapter:

### Safe `web_fetch` adapter (closed)

- New `tool_engine.web_fetch(url, max_bytes)` method:
  - Reuses existing URL policy (https/http only, blocks localhost,
    private IPs, link-local, unique-local IPv6).
  - Fetches host robots.txt once (30-minute cache, 500 KiB cap on the
    robots body per Google's recommendation) and refuses paths
    disallowed for Datrina's User-Agent. Network failures fall through
    as `Unrestricted` after a `warn!` audit line.
  - Caps the response body at 500 KiB by default (`max_bytes` override
    1 KiB..1 MiB). Returns `{ status, url, content_type, bytes,
    max_bytes, truncated, body: { text|json } }`.
- New `ExternalSourceAdapter::WebFetch` variant routes through the
  dispatcher. Catalog entry `web_fetch` exposes it with one required
  `url` param and an optional `max_bytes`. Save-as-datasource refuses
  this adapter because workflow nodes don't yet host `web_fetch`.
- `texting_robots = "0.2"` was added to `src-tauri/Cargo.toml`.

### Brave Search 2026 repricing (closed)

- Catalog entry now records the Feb-2026 changes verbatim: free tier
  removed, $5/mo credit (~1 000 calls), $5 per 1k Search requests, $4
  per 1k + $5 per million tokens for Answers, 1 req/s anonymous cap,
  Pro AI up to 50 req/s, attribution + storage-rights opt-in
  mandatory.
- New `ExternalSourceRateLimit` field surfaces these facts (plan name,
  free quota, paid tier, qps, `attribution_required`,
  `storage_rights_required`). The UI renders them under "Plan / rate".
- The entry stays `needs_review` until Datrina ships an in-product
  storage-rights opt-in surface — a deliberate gate, not a regression.

### MCP server install metadata (closed)

- New `ExternalSourceAdapter::McpRecommended` variant for informational
  catalog rows. Three entries: Fetch MCP (`uvx mcp-server-fetch`),
  GitHub MCP (`npx -y @modelcontextprotocol/server-github` with
  `GITHUB_PERSONAL_ACCESS_TOKEN` env hint), MediaWiki MCP
  (`npx -y @professional-wiki/mediawiki-mcp-server` with
  `MEDIAWIKI_API_URL` env hint).
- New `McpInstallRecommendation` field carries command/args/env_hints.
  Catalog UI shows a copy-to-clipboard button next to the install
  snippet and hides credential/test/save panels for these rows.
- Backend refuses to expose McpRecommended rows as chat tools or to
  save them as datasources — they cannot trip Datrina into running an
  arbitrary command. Install happens through MCP Settings.

## Real out-of-scope (still)

- A workflow-time `web_fetch` node — saved web_fetch datasources would
  need a new workflow tool kind; deferred.
- Automated MCP server install (Datrina shelling out to `npx` / `uvx`).
  We don't want catalog-driven code execution; user pastes the snippet
  themselves.
- In-product storage-rights opt-in for Brave — required before
  promoting `brave_search_web` to `allowed_with_conditions`.

## Context

Datrina can already build dashboards from saved datasources, pipelines, widget
runtime state, and chat-driven analysis. The next product gap is a built-in set
of ready external sources that are safe to use in the product without turning
every dashboard into a custom MCP/tool setup task.

The source set should cover practical domains such as web search, crypto/market
data, developer/technology signals, public datasets, RSS/Atom feeds, and other
sources that can enrich dashboard analysis. "Free to use" here means the source
license or terms allow Datrina's intended local product use. It does not mean
the upstream service has unlimited request volume, no rate limits, or no API key
option.

The important product flow is: a user opens a dashboard, opens chat from that
dashboard, asks for analysis of current dashboard data, and the LLM can use
enabled external sources to clarify missing facts, search the web, inspect a
technology signal, or enrich crypto/market context before proposing changes or
answering.

## Market Research Snapshot

Research date: 2026-05-17.

This workstream starts from a researched MCP/source shortlist, not from a blank
generic connector framework. The first product catalog should include the
following candidates, gated by the license/terms metadata below.

| Priority | Candidate | Domain | Why add it | Adapter license / source status | Terms and product gate |
| --- | --- | --- | --- | --- | --- |
| P0 | [Brave Search MCP Server](https://github.com/brave/brave-search-mcp-server) | Web search, news, images, local, LLM context | Best first "websearch" candidate: official Brave MCP server, broad search tools, stdio/HTTP support, current maintenance signal. | MIT repo; `BRAVE_API_KEY` required. | `allowed_with_conditions`: must store API key in Rust-owned secrets, expose rate/plan constraints, and record Brave Search API terms before enabling by default. |
| P0 | [Fetch MCP Server](https://github.com/modelcontextprotocol/servers/tree/main/src/fetch) | Web page fetch/read | Complements search: after search returns URLs, chat can fetch a specific source page into markdown. | MIT reference server in `modelcontextprotocol/servers`. | `allowed_with_conditions`: enable only with URL allow/deny policy, robots.txt behavior kept on, private/internal IP protection, timeouts, size caps, and citation/provenance. |
| P0 | [CoinGecko MCP Server](https://docs.coingecko.com/docs/ai-agent-hub/mcp-server) / [`@coingecko/coingecko-mcp`](https://www.npmjs.com/package/@coingecko/coingecko-mcp) | Crypto price, market, on-chain, NFT data | Strong first crypto source: official CoinGecko MCP, public keyless beta for quick tests and local/API-key mode for product use. | npm package is Apache-2.0; official docs describe public, authenticated, and local server modes. | `allowed_with_conditions`: beta status and shared/public rate limits must be visible; local/BYOK mode preferred for saved datasources. |
| P1 | [DefiLlama MCP Server](https://github.com/dcSpark/mcp-server-defillama) | DeFi TVL, chains, token prices, stablecoins | Useful no-key-ish DeFi context for crypto dashboards and market analysis beyond spot prices. | MIT repo, small community footprint. | `needs_review`: verify DefiLlama API/data terms and maintenance quality before shipping as supported; good candidate for "experimental" catalog lane. |
| P1 | [GitHub MCP Server](https://github.com/github/github-mcp-server) | Developer/technology signals, repos, issues, PRs, releases, Actions | Strong technology-data source: official GitHub server, useful for dashboards over open-source projects, releases, issue volume, CI health, and repo metadata. | MIT repo; official GitHub docs say availability is broad but some tools inherit paid feature requirements. | `allowed_with_conditions`: default to read-only tools for public repo analytics; write tools disabled unless explicitly enabled by user. |
| P1 | [ArXiv MCP Server](https://github.com/blazickjp/arxiv-mcp-server) | Research / technology papers | Useful for technology dashboards, research trend widgets, and chat clarification against current papers. | Apache-2.0 repo, has prompt-injection warning and mitigation notes. | `allowed_with_conditions`: treat paper content as untrusted external input; read-only profile only; no chained write/tool actions from paper text. |
| P1 | [MediaWiki MCP Server](https://github.com/ProfessionalWiki/MediaWiki-MCP-Server) | Wikipedia / MediaWiki knowledge | Good general knowledge source and can point at internal/public MediaWiki instances. | MIT repo. | `allowed_with_conditions`: ship read-only profile first; create/edit/delete page tools must be disabled by default. |
| P2 | [FreshRSS MCP Server](https://pypi.org/project/freshrss-mcp/) or [MCP RSS Aggregator](https://github.com/imprvhub/mcp-rss-aggregator) | RSS/Atom feeds, OPML | RSS is a natural dashboard source for tech/news monitoring; market is fragmented, so use a conservative adapter. | FreshRSS MCP lists MIT; RSS Aggregator lists MPL-2.0. | `needs_review`: choose one maintained path or implement a native read-only RSS adapter if third-party MCP quality is too low. |
| P2 | [TinyHNews MCP](https://github.com/fguillen/TinyHNewsMCP) | Hacker News / tech trend feed | Good lightweight tech-signal source for "what is trending" analysis. | MIT repo, but tiny/low-maturity project. | `needs_review`: likely better to implement a tiny native read-only HN adapter unless MCP quality improves. |
| P2 | [SearXNG MCP Server](https://github.com/tisDDM/searxng-mcp) | Self-hosted / metasearch web search | Open/self-hosted search option for users who do not want Brave/hosted APIs. | MIT repo, but deprecated in favor of successor; SearXNG instance terms and public-instance reliability vary. | `needs_review`: do not rely on random public instances; support only user-configured/self-hosted SearXNG endpoint or our own adapter after review. |

Market conclusion:

- Use MCP as an import/runtime adapter where the server is maintained and has a
  permissive adapter license.
- Do not blindly execute arbitrary marketplace MCP servers. Datrina should ship
  a curated catalog with reviewed metadata, disabled-by-default write tools, and
  source-specific safety profiles.
- For small/read-only sources such as RSS or Hacker News, a native Rust adapter
  may be safer than depending on a low-maturity third-party MCP server. The
  catalog can still expose them through the same UX/tool policy.
- "Free to use" must be recorded as two separate facts: adapter code license
  and upstream API/data terms. A permissive MCP server license does not make the
  upstream data unlimited or unrestricted.

## Goal

- Add a curated catalog of built-in source connectors that can be enabled or
  disabled from Datrina UX.
- Each catalog entry records source kind, license/terms status, attribution
  needs, request constraints, required credentials if any, and local safety
  policy.
- Include initial connector families for web search, crypto/market data,
  technology/developer data, RSS/Atom, and simple public HTTP/JSON sources.
- Seed the first catalog with 5-10 researched candidates from the market
  snapshot, each with a status of `allowed_with_conditions`, `needs_review`, or
  `blocked`.
- Connectors are available both as saved datasources/workbench sources and as
  chat tools when enabled.
- Dashboard-scoped chat can analyze current dashboard/widget/datasource data
  and call enabled external source tools for clarification.
- External-source calls are visible in chat/tool trace UI, workflow operations,
  and datasource/debug surfaces.
- Disabled sources are not callable by chat, workflows, or Build proposals.
- No source ships as "supported" until its adapter license and upstream data/API
  terms are verified and recorded.

## Approach

1. Define the source catalog model.
   - Add `ExternalSourceDefinition` or equivalent model with id, kind, display
     name, description, adapter type, config schema, enablement state,
     credential policy, rate-limit metadata, attribution requirements, and
     license/terms review fields.
   - Store built-in catalog entries separately from user-created saved
     datasources so default source metadata can be upgraded without overwriting
     user configuration.
   - Persist per-profile enable/disable state and user-provided credentials or
     endpoint overrides through Rust-owned secret/config paths.

2. Add license and terms gating.
   - Every built-in entry must carry a reviewed status: `allowed`,
     `allowed_with_conditions`, `blocked`, or `needs_review`.
   - `needs_review` and `blocked` entries may be visible as unavailable
     candidates, but cannot be executed.
   - Record exact upstream license/terms URL, review date, constraints, and any
     attribution text required in UI/export.
   - Treat adapter code license and upstream data/API terms as separate checks.

3. Implement connector families through existing engines.
   - Web search: support an open/self-hostable or terms-compatible search
     backend first; do not hardcode scraping of search result pages.
   - Crypto/market data: support one or more public/terms-compatible market data
     APIs with explicit rate-limit and attribution metadata.
   - Technology/developer data: support sources such as package registries,
     GitHub-style repository metadata, release feeds, security/advisory feeds,
     Stack Exchange-style APIs, Hacker News-style APIs, or arXiv-like feeds only
     after terms review.
   - RSS/Atom and generic JSON HTTP: provide configurable low-risk sources with
     explicit schema/pipeline shaping.
   - Prefer typed connector adapters over arbitrary user JavaScript.

4. Integrate with Workbench and saved datasources.
   - Workbench has a "Source Catalog" or equivalent entrypoint.
   - Users can enable a source, test it, inspect example output, then save it as
     a datasource or bind it to widgets.
   - Saved datasource definitions keep explicit provenance back to the built-in
     source definition and version.
   - Pipeline shaping remains in the existing typed pipeline DSL.

5. Integrate with dashboard-scoped chat.
   - Opening chat from a dashboard injects compact dashboard context: selected
     dashboard id, visible widget summaries, datasource ids, recent run status,
     relevant parameter values, and cached/live freshness state.
   - Chat tool policy exposes only enabled external sources plus sources already
     allowed by the dashboard/profile.
   - Build Chat can ask an enabled external source for clarification before
     proposing a widget, but preview/apply still goes through validation and
     explicit user confirmation.
   - Normal analysis chat can cite which external source calls informed the
     answer and distinguish live external evidence from stale dashboard data.

6. Keep operations and audit visible.
   - External source calls appear in chat traces, datasource run traces, and W35
     workflow operations where applicable.
   - Errors distinguish disabled source, missing credential, license/terms
     blocked, rate limited, network failure, parsing failure, and unsupported
     shape.
   - Users can disable a source globally and see which dashboards/datasources
     would be affected.

7. Ship an initial curated set conservatively.
   - Start with the P0/P1 entries from the market snapshot rather than a large
     unreviewed list.
   - P0 acceptance target: Brave Search, Fetch, and CoinGecko wired through the
     same enable/test/save/use-in-chat flow, unless a terms/security review
     blocks one of them.
   - P1 acceptance target: at least two of GitHub, arXiv, MediaWiki, or
     DefiLlama available as reviewed/experimental catalog entries.
   - Keep P2 entries as candidate backlog if maturity, maintenance, or terms are
     not strong enough for a supported default.
   - Do not represent request quotas, uptime, or third-party service continuity
     as guaranteed.

## Files

- `src-tauri/src/models/datasource.rs`
- `src-tauri/src/models/workflow.rs`
- new external source model under `src-tauri/src/models/` if cleaner
- `src-tauri/src/modules/storage.rs`
- `src-tauri/src/modules/tool_engine.rs`
- `src-tauri/src/modules/workflow_engine.rs`
- `src-tauri/src/modules/ai.rs`
- `src-tauri/src/commands/datasource.rs`
- `src-tauri/src/commands/tool.rs`
- `src-tauri/src/commands/chat.rs`
- new external source command file under `src-tauri/src/commands/` if cleaner
- `src-tauri/src/lib.rs`
- `src/lib/api.ts`
- `src/components/datasource/Workbench.tsx`
- new source catalog components under `src/components/datasource/` or
  `src/components/sources/`
- `src/components/layout/ChatPanel.tsx`
- `src/components/debug/PipelineDebugModal.tsx`
- `src/components/operations/*` if W35 has landed
- `docs/RECONCILIATION_PLAN.md`
- `docs/W37_EXTERNAL_OPEN_SOURCE_CATALOG.md`

## Validation

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`
- `bun run check:contract`
- `bun run typecheck`
- `bun run build`
- `cargo fmt --all --check` or targeted `rustfmt --edition 2021` for changed
  Rust files if unrelated format drift exists.
- `cargo check --workspace --all-targets`
- Unit or integration checks for:
  - catalog listing and enable/disable persistence,
  - blocked/needs-review source cannot execute,
  - credential-required source fails with typed remediation when unset,
  - enabled source can run through the tool engine,
  - saved datasource preserves source definition provenance,
  - dashboard-scoped chat only sees enabled/allowed source tools,
  - external source calls are recorded in chat/tool traces,
  - disabled source is removed from chat/workflow tool availability.
- Manual source smoke:
  - enable one reviewed no-key or local/self-hosted source,
  - test it from the Source Catalog,
  - save it as a datasource,
  - bind it to a dashboard widget,
  - open chat from that dashboard and ask for analysis that requires an
    external clarification call,
  - confirm the chat answer references dashboard data and the external call
    separately,
  - disable the source and confirm chat/workflow calls fail closed with a typed
    disabled-source error.
- License/terms acceptance check:
  - every shipped built-in source has adapter license, upstream terms URL,
    review date, allowed status, attribution needs, credential policy, and
    request constraints recorded in the catalog metadata or companion docs.

## Out of scope

- Claiming unlimited request volume or guaranteed free upstream service access.
- Scraping web pages or search results in violation of upstream terms.
- Shipping unreviewed sources as executable defaults.
- Marketplace, team sharing, or remote plugin distribution.
- Arbitrary JavaScript connectors.
- Bypassing W29 real-provider/no-fake-success behavior.
- Auto-applying Build Chat changes without preview and explicit confirmation.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W5_TOOL_SECURITY_BASELINE.md`
- `docs/W13_DURABLE_REAL_RUNTIME_PIPELINE.md`
- `docs/W23_PIPELINE_DEBUG_VIEW.md`
- `docs/W29_REAL_PROVIDER_RUNTIME_GATE.md`
- `docs/W30_DATASOURCE_PIPELINE_WORKBENCH.md`
- `docs/W31_DATASOURCE_IDENTITY_BINDING_PROVENANCE.md`
- `docs/W32_TYPED_PIPELINE_STUDIO.md`
- `docs/W35_WORKFLOW_OPERATIONS_COCKPIT.md`

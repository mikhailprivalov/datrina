# W17 Agent Memory And Local RAG

Status: shipped (v1, FTS5-only; vec/embedding swap deferred to v2)

Date: 2026-05-16

## v1 vs spec

Shipped in v1:

- `agent_memory` table with FTS5 mirror + triggers, scope/kind indexes.
- `mcp_tool_observed_shape` table with `(server_id, tool_name, args_fingerprint)` upsert.
- `MemoryEngine` (`modules/memory.rs`) with `remember` / `forget` / `retrieve` /
  `observe_tool_shape` / `lookup_tool_shape` / `list_tool_shapes`.
- Hybrid scoring: FTS5 BM25 + scope priority boost (dashboard > mcp_server >
  session > global) + recency decay + access-count boost.
- Auto-injection of top-N relevant memories into the chat system prompt
  for both Build and Context modes (capped at ~6 KB / ~1.5K tokens).
- `mcp_tool` calls observe their result shape automatically (background task).
- New `recall` agent tool exposed alongside `http_request`, `mcp_tool`,
  `dry_run_widget`.
- Tauri commands: `list_memories`, `delete_memory`, `remember_memory`,
  `recall_memories`, `list_tool_shapes`, `list_memory_kinds`.
- Frontend: `memoryApi` namespace + Settings → Agent Memory pane that lists
  records grouped by scope, with filter/search/forget.

Deferred to v2 (intentional, not gaps):

- sqlite-vec virtual table + `fastembed` ONNX embeddings. Adds ~150 MB to
  the binary + a first-run model download failure mode; v1 ships keyword
  retrieval that already serves the user-visible behavior described below.
  The `Embedder` swap point is a single module.
- LLM-based decay/compression scheduled task (`compress_old`). The schema
  has `compressed_into` ready; the cron entry is a v2 follow-up.
- Post-session `remember_lessons` LLM extractor; the agent can write
  durable facts on its own via `recall` + the `remember_memory` command,
  which is the closer-loop variant.
- "Re-embed all" admin button (unused without embeddings).

## Context

Every chat session starts cold. The agent re-discovers the same MCP tool
shapes, the same data quirks, and the same user preferences on every turn.
There is no `agent_memory`, no `tool_schema_cache`, and no persistent
state beyond `chat_sessions.messages` JSON. Token cost is paid repeatedly
for context the agent already produced once.

W17 introduces a **local hybrid RAG** system as the foundation for
"unbounded" agent memory. The stack stays inside SQLite (consistent with
W3's storage baseline) and adds no separate process or cloud dependency.

## Goal

The agent has scoped, persistent memory across sessions:

- Project-scoped facts ("user prefers gauge over stat for percentages").
- Dashboard-scoped facts ("this dashboard's source returns ISO dates as
  `data.items[].publishedAt`").
- MCP-server-scoped facts (e.g., "server `<id>` tool `<tool_name>`
  returns `{content: [{text: "<json>"}]}` envelope; unwrap before
  pipeline").
- Tool-result schema cache ("for tool X with args fingerprint Y the
  response shape was Z").

At session start, the agent receives a compact, relevance-ranked extract of
this memory, retrieved via a hybrid of FTS5 keyword search and sqlite-vec
semantic search.

## Approach

### Storage layer

#### Dependencies

- **sqlite-vec** (Alex Garcia, v1.0) loaded as a runtime SQLite extension.
  Ship the `.dylib`/`.so`/`.dll` for the three desktop targets in
  `src-tauri/resources/sqlite-vec/`. Loaded via `rusqlite::LoadExtensionGuard`
  in `Storage::open`.
- **fastembed-rs** as a `Cargo.toml` dependency. Models downloaded to
  `<app_data>/models/` on first use. Default model:
  `intfloat/multilingual-e5-small` (~120 MB, multilingual including
  Russian, good cost/quality ratio for short snippets).

#### Schema (new migration)

```sql
CREATE TABLE agent_memory (
  id TEXT PRIMARY KEY,
  scope TEXT NOT NULL,                -- 'global' | 'dashboard' | 'mcp_server' | 'session'
  scope_id TEXT,                      -- nullable for 'global'
  kind TEXT NOT NULL,                 -- 'fact' | 'preference' | 'tool_shape' | 'lesson'
  content TEXT NOT NULL,
  metadata TEXT NOT NULL,             -- JSON: { source_session_id, source_tool_call_id, confidence, ... }
  created_at INTEGER NOT NULL,
  accessed_count INTEGER NOT NULL DEFAULT 0,
  last_accessed_at INTEGER,
  expires_at INTEGER,                 -- nullable; for soft TTL
  compressed_into TEXT                -- nullable; points at a parent memory after summarization
);

CREATE INDEX agent_memory_scope_idx ON agent_memory (scope, scope_id);
CREATE INDEX agent_memory_kind_idx ON agent_memory (kind);

CREATE VIRTUAL TABLE agent_memory_fts USING fts5 (
  content, scope, scope_id, kind,
  content='agent_memory', content_rowid='rowid',
  tokenize='unicode61 remove_diacritics 2'
);

CREATE VIRTUAL TABLE agent_memory_vec USING vec0 (
  memory_id TEXT PRIMARY KEY,
  embedding FLOAT[384]                -- e5-small returns 384-dim
);

-- Specialized view of tool result shapes for cheap lookup outside RAG.
CREATE TABLE mcp_tool_observed_shape (
  id TEXT PRIMARY KEY,
  server_id TEXT NOT NULL,
  tool_name TEXT NOT NULL,
  args_fingerprint TEXT NOT NULL,     -- sha1 of sorted-keys canonical JSON of args (with values masked to types)
  shape_summary TEXT NOT NULL,        -- compact human-readable: "object with keys: data.items[*].{id,name,version}"
  shape_full TEXT NOT NULL,           -- pruned JSON schema-like sketch
  sample_path TEXT,                   -- known good JMESPath suggestion
  observed_at INTEGER NOT NULL,
  observation_count INTEGER NOT NULL DEFAULT 1
);
CREATE UNIQUE INDEX mcp_tool_observed_shape_key
  ON mcp_tool_observed_shape (server_id, tool_name, args_fingerprint);
```

FTS and vec mirror tables are kept in sync by triggers on `agent_memory`.

### Memory engine (`src-tauri/src/modules/memory.rs`, new)

Public surface:

```rust
pub struct MemoryEngine { /* sqlite handle + fastembed instance */ }

impl MemoryEngine {
    pub async fn new(...) -> Result<Self>;
    pub async fn remember(&self, scope: Scope, kind: MemoryKind, content: &str, metadata: Value) -> Result<MemoryId>;
    pub async fn forget(&self, id: &MemoryId) -> Result<()>;
    pub async fn retrieve(&self, query: &str, scopes: &[Scope], top_n: usize) -> Result<Vec<MemoryHit>>;
    pub async fn observe_tool_shape(&self, server_id: &str, tool_name: &str, args: &Value, result: &Value) -> Result<()>;
    pub async fn lookup_tool_shape(&self, server_id: &str, tool_name: &str, args: &Value) -> Option<ToolShape>;
    pub async fn compress_old(&self, llm: &AIEngine, older_than: Duration) -> Result<usize>;
}
```

#### Retrieval (hybrid)

`retrieve` runs both legs in parallel:

1. **FTS5 BM25** — `SELECT memory_id, rank FROM agent_memory_fts WHERE
   agent_memory_fts MATCH ?` filtered by scope. Top 20.
2. **sqlite-vec cosine** — `SELECT memory_id, distance FROM
   agent_memory_vec WHERE embedding MATCH ? AND k=20` after embedding the
   query once.

Results are fused via **Reciprocal Rank Fusion** (`RRF(d) = sum(1/(60 +
rank_in_leg))`), then re-ranked by RRF + scope priority (current dashboard
> current server > global). Top N returned.

#### Embedding

`fastembed::TextEmbedding::new(InitOptions::default()
  .with_model(EmbeddingModel::MultilingualE5Small))`. Single-threaded
embedding (~10–30 ms per item). Persistent instance held in `MemoryEngine`.
Cold start ~200 ms (model load).

#### Tool-shape observation

After every successful `mcp_tool` call (`commands/chat.rs` MCP path):

1. Compute `args_fingerprint`: canonical JSON of `args` with leaf string
   values replaced by `"<string>"`, numbers by `"<number>"`, etc., then
   sha1. Two calls with the same arg shape collide; different shapes don't.
2. Derive `shape_summary`: walk the result JSON to depth 4, collect leaf
   paths + types, join into a compact line.
3. Upsert into `mcp_tool_observed_shape` (increment `observation_count` on
   conflict).
4. Also `remember(Scope::McpServer(server_id), MemoryKind::ToolShape,
   shape_summary, metadata)` so RAG retrieval can surface it too.

#### Decay & compression

A scheduled task (added to `Scheduler`) every 24h calls `compress_old`:

- Find groups of >5 memories with overlap (same scope + similar
  embeddings) older than 7 days.
- LLM-summarize the group into a single distilled memory; set
  `compressed_into` on the originals; delete or archive originals after
  N days.

This is the "infinite memory" property: old chatter compresses, signal
survives.

### Chat integration (`src-tauri/src/commands/chat.rs`)

At the top of `send_message_stream_inner`:

1. Build retrieval query = first user message in turn + active dashboard
   id + relevant MCP server ids extracted from the prompt body.
2. Call `MemoryEngine::retrieve(query, scopes, top_n=8)`.
3. Inject hits into the system prompt under a new section:

   ```
   ## Known facts from prior sessions (relevance-ranked, may be stale)
   - [dashboard] User prefers gauge widgets for percentage metrics.
   - [mcp:<server_id>] tool `<tool_name>` returns
     {content:[{text:"<json>"}]} envelope; data.items[*].{id,name,version,...}.
   - ...
   ```

4. Token budget for memory section is capped at `MEMORY_INJECTION_TOKENS =
   1500`. Truncate by lowest RRF first.

After each session, run a small `remember_lessons` LLM pass that extracts
new durable facts from the session transcript and writes them as
`MemoryKind::Lesson` memories scoped to the dashboard and any MCP servers
used.

### Agent tools

New tool registered in `chat_tool_specs`:

- `recall(query: string, scope?: "global"|"dashboard"|"mcp_server")` →
  returns top 5 memory hits with content + metadata. Lets the agent
  explicitly query its own memory mid-turn.

- `forget(memory_id: string)` — admin-style; agent can suggest forgetting a
  stale memory after the user confirms (user-facing UI in W17 v2).

### UI (`src/components/settings/MemorySettings.tsx`, new)

Simple admin pane under Settings → Memory:

- Scrollable list of memories grouped by scope.
- Edit / delete actions.
- Toggle: "Auto-extract lessons after each session" (default on).
- "Re-embed all" button (after model upgrade).

## Files to touch

- `src-tauri/Cargo.toml` — add `fastembed`, `sqlite-vec` (FFI binding crate
  or direct extension load), `schemars` optional.
- `src-tauri/src/modules/memory.rs` — new module.
- `src-tauri/src/modules/storage.rs` — load sqlite-vec extension; new
  migration for `agent_memory*` and `mcp_tool_observed_shape`.
- `src-tauri/src/commands/chat.rs` — system-prompt injection;
  observe-shape hook on every mcp_tool result; `recall` tool spec.
- `src-tauri/src/commands/memory.rs` (new) — `list_memories`, `delete_memory`,
  `reembed_memories` Tauri commands for the UI.
- `src-tauri/src/lib.rs` — register memory commands; manage `MemoryEngine`
  in `AppState`.
- `src/lib/api.ts` — `Memory`, `MemoryScope`, `MemoryHit`, `memoryApi`.
- `src/components/settings/MemorySettings.tsx` — admin UI.
- `src-tauri/resources/sqlite-vec/` — bundled extension binaries.

## Validation

- `cargo check --workspace --all-targets` after dependency adds.
- `bun run check:contract` after new commands.
- Integration test: write 100 synthetic memories, query a known phrase,
  assert the right item is in top-3.
- Manual: chat against any configured stdio MCP server, end session;
  start a new session; confirm the system prompt now contains the
  learned tool shape for that server.
- Manual: trigger `compress_old` on a seeded dataset; confirm
  `compressed_into` chain and reduced row count.
- Cold-start budget: full first launch + model download ≤ 90 s on a clean
  machine; subsequent starts ≤ 500 ms.

## Out of scope

- Cloud-synced memory.
- Multi-user / shared memory.
- Reranker model on top of RRF (could land in a follow-up if recall is
  insufficient).
- Embedding model swap UI (always e5-small in this iteration).

## Related

- W3 storage baseline — same migration pattern.
- W16 validator can read `mcp_tool_observed_shape` to refine its checks.
- W24 eval suite can use seeded memories to exercise the retrieval path.

use anyhow::Result;
use std::str::FromStr;

use sqlx::{sqlite::SqliteConnectOptions, sqlite::SqlitePoolOptions, Pool, Row, Sqlite};
use tauri::{App, Manager};
use tracing::info;

use crate::models::{
    alert::{AlertEvent, AlertSeverity, WidgetAlert},
    chat::ChatSession,
    dashboard::{Dashboard, DashboardVersion, DashboardVersionSummary, VersionSource},
    datasource::{DatasourceDefinition, DatasourceHealth},
    mcp::MCPServer,
    memory::{MemoryKind, MemoryRecord, Scope, ToolShape},
    playground::{PlaygroundPreset, PlaygroundToolKind},
    provider::LLMProvider,
    snapshot::WidgetRuntimeSnapshot,
    workflow::Workflow,
};

pub struct Storage {
    pool: Pool<Sqlite>,
    app_data_dir: std::path::PathBuf,
}

/// W51: post-redaction raw artifact record returned by
/// [`Storage::get_raw_artifact`]. The `payload_json` is the redacted
/// raw payload — secrets have already been stripped by the
/// compressor's redaction pass — so this struct is safe to surface in
/// debug UIs and `inspect_artifact` slices.
#[derive(Debug, Clone)]
pub struct RawArtifactRecord {
    pub id: String,
    pub owner_kind: String,
    pub owner_id: String,
    pub profile: String,
    pub raw_size: usize,
    pub compact_size: usize,
    pub checksum: String,
    pub redaction_version: u32,
    pub retention_class: String,
    pub payload_json: String,
    pub created_at: i64,
}

/// FTS5 MATCH requires a syntactically valid query. User-typed text often
/// contains `-`, `(`, `*`, `"`, etc., which FTS5 parses as operators and
/// rejects. We turn the input into a permissive OR of bare tokens so any
/// non-empty query still matches by keyword.
fn sanitize_fts_query(input: &str) -> String {
    let tokens: Vec<String> = input
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .filter(|t| t.chars().count() >= 2)
        .collect();
    if tokens.is_empty() {
        return String::new();
    }
    tokens
        .into_iter()
        .map(|t| format!("\"{}\"", t))
        .collect::<Vec<_>>()
        .join(" OR ")
}

impl Storage {
    #[cfg(test)]
    pub(crate) async fn new_for_tests() -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await?;

        let app_data_dir = std::env::temp_dir().join("datrina-tests");
        let _ = std::fs::create_dir_all(&app_data_dir);
        let storage = Self { pool, app_data_dir };
        storage.migrate().await?;
        Ok(storage)
    }

    pub async fn new(app: &App) -> Result<Self> {
        let app_dir = std::env::var_os("DATRINA_APP_DATA_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or(app.path().app_data_dir()?);
        std::fs::create_dir_all(&app_dir)?;

        let db_path = app_dir.join("app.db");
        let db_url = format!("sqlite://{}", db_path.to_string_lossy());

        info!("📦 Database path: {}", db_path.display());

        let connect_options = SqliteConnectOptions::from_str(&db_url)?.create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(connect_options)
            .await?;

        Ok(Self {
            pool,
            app_data_dir: app_dir,
        })
    }

    /// W22: filesystem location of `pricing_overrides.json`. Lives alongside
    /// `app.db` in the OS app-data dir so the same backup covers both.
    pub fn pricing_overrides_path(&self) -> std::path::PathBuf {
        self.app_data_dir.join("pricing_overrides.json")
    }

    pub async fn migrate(&self) -> Result<()> {
        // Dashboards table
        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS dashboards (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    description TEXT,
                    layout TEXT NOT NULL DEFAULT '[]',
                    workflows TEXT NOT NULL DEFAULT '[]',
                    is_default INTEGER DEFAULT 0,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Chat sessions table
        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS chat_sessions (
                    id TEXT PRIMARY KEY,
                    mode TEXT NOT NULL CHECK(mode IN ('build', 'context')),
                    dashboard_id TEXT,
                    widget_id TEXT,
                    title TEXT NOT NULL,
                    messages TEXT NOT NULL DEFAULT '[]',
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // W18: plan artifact + per-step status persisted per session so
        // Build continuations resume with accurate plan state. Both
        // columns are JSON-encoded; nullable when no plan exists yet.
        // Errors here are tolerated because they're "duplicate column"
        // on already-migrated databases.
        let _ = sqlx::query("ALTER TABLE chat_sessions ADD COLUMN current_plan TEXT")
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("ALTER TABLE chat_sessions ADD COLUMN plan_status TEXT")
            .execute(&self.pool)
            .await;

        // W22: per-session token + cost totals + optional budget cap. All
        // updated transactionally by `update_chat_session`. Existing rows
        // start at zero / NULL on first read.
        // W49: `cost_unknown_turns` counts assistant turns whose tokens
        // were recorded but pricing was unknown — `total_cost_usd` is a
        // lower bound when this is positive.
        for stmt in [
            "ALTER TABLE chat_sessions ADD COLUMN total_input_tokens INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE chat_sessions ADD COLUMN total_output_tokens INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE chat_sessions ADD COLUMN total_reasoning_tokens INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE chat_sessions ADD COLUMN total_cost_usd REAL NOT NULL DEFAULT 0.0",
            "ALTER TABLE chat_sessions ADD COLUMN max_cost_usd REAL",
            "ALTER TABLE chat_sessions ADD COLUMN cost_unknown_turns INTEGER NOT NULL DEFAULT 0",
        ] {
            let _ = sqlx::query(stmt).execute(&self.pool).await;
        }

        // W47: per-session assistant language override (JSON-encoded
        // AssistantLanguagePolicy). Nullable; missing on legacy rows.
        let _ = sqlx::query("ALTER TABLE chat_sessions ADD COLUMN language_override TEXT")
            .execute(&self.pool)
            .await;

        // Workflows table
        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS workflows (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    description TEXT,
                    nodes TEXT NOT NULL DEFAULT '[]',
                    edges TEXT NOT NULL DEFAULT '[]',
                    trigger TEXT NOT NULL DEFAULT '{}',
                    is_enabled INTEGER DEFAULT 1,
                    last_run TEXT,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // W50: pause/resume + reason metadata. Distinct from `is_enabled`
        // so an operator can stop automatic refresh without disabling the
        // workflow entirely. Errors here are ignored because the column
        // exists on every freshly migrated DB after the first run.
        let _ = sqlx::query(
            "ALTER TABLE workflows ADD COLUMN pause_state TEXT NOT NULL DEFAULT 'active'",
        )
        .execute(&self.pool)
        .await;
        let _ = sqlx::query("ALTER TABLE workflows ADD COLUMN last_paused_at INTEGER")
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("ALTER TABLE workflows ADD COLUMN last_pause_reason TEXT")
            .execute(&self.pool)
            .await;

        // Providers table
        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS providers (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    base_url TEXT NOT NULL,
                    api_key TEXT,
                    default_model TEXT NOT NULL,
                    models TEXT NOT NULL DEFAULT '[]',
                    is_enabled INTEGER DEFAULT 1
                )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // MCP servers table
        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS mcp_servers (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    transport TEXT NOT NULL,
                    command TEXT,
                    args TEXT,
                    env TEXT,
                    url TEXT,
                    is_enabled INTEGER DEFAULT 1
                )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // App config table
        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS app_config (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL,
                    updated_at INTEGER NOT NULL
                )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Workflow runs
        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS workflow_runs (
                    id TEXT PRIMARY KEY,
                    workflow_id TEXT NOT NULL,
                    started_at INTEGER NOT NULL,
                    finished_at INTEGER,
                    status TEXT NOT NULL,
                    node_results TEXT,
                    error TEXT
                )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // W35: Operations cockpit lists runs filtered by workflow ordered
        // by start time descending. Without this index every list scan
        // performs a full-table sort.
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS workflow_runs_workflow_idx \
             ON workflow_runs (workflow_id, started_at DESC)",
        )
        .execute(&self.pool)
        .await?;

        // W17: agent memory (long-lived facts, preferences, lessons,
        // observed tool shapes). Retrieval is keyword via FTS5 today;
        // sqlite-vec embedding column is a deliberate v2 follow-up.
        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS agent_memory (
                    id TEXT PRIMARY KEY,
                    scope TEXT NOT NULL,
                    scope_id TEXT,
                    kind TEXT NOT NULL,
                    content TEXT NOT NULL,
                    metadata TEXT NOT NULL DEFAULT '{}',
                    created_at INTEGER NOT NULL,
                    accessed_count INTEGER NOT NULL DEFAULT 0,
                    last_accessed_at INTEGER,
                    expires_at INTEGER,
                    compressed_into TEXT
                )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS agent_memory_scope_idx ON agent_memory (scope, scope_id)",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS agent_memory_kind_idx ON agent_memory (kind)")
            .execute(&self.pool)
            .await?;

        // External-content FTS5 mirror over agent_memory.content. Triggers
        // keep rowid alignment so MATCH queries cheaply join back.
        sqlx::query(
            r#"
                CREATE VIRTUAL TABLE IF NOT EXISTS agent_memory_fts USING fts5(
                    content,
                    content='agent_memory',
                    content_rowid='rowid',
                    tokenize='unicode61 remove_diacritics 2'
                )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
                CREATE TRIGGER IF NOT EXISTS agent_memory_ai AFTER INSERT ON agent_memory BEGIN
                    INSERT INTO agent_memory_fts(rowid, content) VALUES (new.rowid, new.content);
                END
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
                CREATE TRIGGER IF NOT EXISTS agent_memory_ad AFTER DELETE ON agent_memory BEGIN
                    INSERT INTO agent_memory_fts(agent_memory_fts, rowid, content)
                    VALUES('delete', old.rowid, old.content);
                END
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
                CREATE TRIGGER IF NOT EXISTS agent_memory_au AFTER UPDATE ON agent_memory BEGIN
                    INSERT INTO agent_memory_fts(agent_memory_fts, rowid, content)
                    VALUES('delete', old.rowid, old.content);
                    INSERT INTO agent_memory_fts(rowid, content) VALUES (new.rowid, new.content);
                END
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Specialized view of tool result shapes for cheap lookup outside
        // the RAG path (used directly by the planner before it calls a
        // tool whose shape it has already learned).
        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS mcp_tool_observed_shape (
                    id TEXT PRIMARY KEY,
                    server_id TEXT NOT NULL,
                    tool_name TEXT NOT NULL,
                    args_fingerprint TEXT NOT NULL,
                    shape_summary TEXT NOT NULL,
                    shape_full TEXT NOT NULL,
                    sample_path TEXT,
                    observed_at INTEGER NOT NULL,
                    observation_count INTEGER NOT NULL DEFAULT 1
                )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS mcp_tool_observed_shape_key \
             ON mcp_tool_observed_shape (server_id, tool_name, args_fingerprint)",
        )
        .execute(&self.pool)
        .await?;

        // W19: per-dashboard version history. Every mutation
        // (apply/manual edit/restore/pre-delete) writes one row here so the
        // user can list, diff, and restore prior states from the UI. Ring
        // buffer cap (`MAX_VERSIONS_PER_DASHBOARD`) is enforced inside
        // `record_dashboard_version`.
        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS dashboard_versions (
                    id TEXT PRIMARY KEY,
                    dashboard_id TEXT NOT NULL REFERENCES dashboards(id) ON DELETE CASCADE,
                    snapshot_json TEXT NOT NULL,
                    applied_at INTEGER NOT NULL,
                    source TEXT NOT NULL,
                    source_session_id TEXT,
                    summary TEXT NOT NULL,
                    widget_count INTEGER NOT NULL,
                    parent_version_id TEXT
                )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS dashboard_versions_dashboard_idx \
             ON dashboard_versions (dashboard_id, applied_at DESC)",
        )
        .execute(&self.pool)
        .await?;

        // W20: Data Playground saved presets. `server_id` is nullable
        // because HTTP / builtin tools don't bind to an MCP server.
        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS playground_presets (
                    id TEXT PRIMARY KEY,
                    tool_kind TEXT NOT NULL,
                    server_id TEXT,
                    tool_name TEXT NOT NULL,
                    display_name TEXT NOT NULL,
                    arguments TEXT NOT NULL DEFAULT '{}',
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS playground_presets_source_idx \
             ON playground_presets (tool_kind, server_id, tool_name)",
        )
        .execute(&self.pool)
        .await?;

        // W21: per-widget alert definitions. Stored separately from the
        // widget JSON so the 10 widget variants stay untouched. One row
        // per widget; `alerts_json` is the array of `WidgetAlert`s.
        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS widget_alerts (
                    widget_id TEXT PRIMARY KEY,
                    dashboard_id TEXT NOT NULL,
                    alerts_json TEXT NOT NULL DEFAULT '[]',
                    updated_at INTEGER NOT NULL
                )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS widget_alerts_dashboard_idx \
             ON widget_alerts (dashboard_id)",
        )
        .execute(&self.pool)
        .await?;

        // W21: firing-event log. The Sidebar badge, AlertsView feed, and
        // autonomous-trigger budget all read from here.
        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS alert_events (
                    id TEXT PRIMARY KEY,
                    widget_id TEXT NOT NULL,
                    dashboard_id TEXT NOT NULL,
                    alert_id TEXT NOT NULL,
                    fired_at INTEGER NOT NULL,
                    severity TEXT NOT NULL,
                    message TEXT NOT NULL,
                    context_json TEXT NOT NULL,
                    acknowledged_at INTEGER,
                    triggered_session_id TEXT,
                    workflow_run_id TEXT
                )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // W35: ALTER for databases that pre-date the column.
        let _ = sqlx::query("ALTER TABLE alert_events ADD COLUMN workflow_run_id TEXT")
            .execute(&self.pool)
            .await;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS alert_events_widget_idx \
             ON alert_events (widget_id, fired_at DESC)",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS alert_events_unack_idx \
             ON alert_events (acknowledged_at) WHERE acknowledged_at IS NULL",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS alert_events_alert_idx \
             ON alert_events (alert_id, fired_at DESC)",
        )
        .execute(&self.pool)
        .await?;

        // W23: per-widget pipeline traces. Ring-buffer up to 5 entries per
        // widget. Capture is opt-in via widget datasource flag; persisted
        // traces feed the Debug view history and the W18 reflection turn.
        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS widget_traces (
                    widget_id TEXT NOT NULL,
                    captured_at INTEGER NOT NULL,
                    trace_json TEXT NOT NULL,
                    PRIMARY KEY (widget_id, captured_at)
                )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS widget_traces_widget_idx \
             ON widget_traces (widget_id, captured_at DESC)",
        )
        .execute(&self.pool)
        .await?;

        // W25: per-dashboard parameter declarations live alongside layout
        // as a JSON column. Per-user selections are tracked separately so a
        // dropdown change doesn't rewrite the entire dashboard row.
        let _ =
            sqlx::query("ALTER TABLE dashboards ADD COLUMN parameters TEXT NOT NULL DEFAULT '[]'")
                .execute(&self.pool)
                .await;

        // W43: optional dashboard-level default LLM policy for LLM-backed
        // widgets. Nullable so older rows continue to mean "no policy",
        // which is the same as having no override at all — the runtime
        // falls back to the app-level active provider.
        let _ = sqlx::query("ALTER TABLE dashboards ADD COLUMN model_policy TEXT")
            .execute(&self.pool)
            .await;

        // W47: optional dashboard-level assistant language policy.
        // Nullable; falls back to the app default (then to Auto).
        let _ = sqlx::query("ALTER TABLE dashboards ADD COLUMN language_policy TEXT")
            .execute(&self.pool)
            .await;

        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS dashboard_parameter_values (
                    dashboard_id TEXT NOT NULL,
                    param_name TEXT NOT NULL,
                    value_json TEXT NOT NULL,
                    updated_at INTEGER NOT NULL,
                    PRIMARY KEY (dashboard_id, param_name)
                )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // W30: saved datasource definitions surfaced in the Workbench.
        // Stored as JSON so additive fields don't require schema migrations;
        // health snapshot is broken out so frequent test-runs only rewrite
        // the snapshot row.
        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS datasource_definitions (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    server_id TEXT,
                    tool_name TEXT,
                    workflow_id TEXT NOT NULL,
                    definition_json TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS datasource_definitions_workflow_idx \
             ON datasource_definitions (workflow_id)",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS datasource_definitions_signature_idx \
             ON datasource_definitions (kind, server_id, tool_name)",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS datasource_health (
                    datasource_id TEXT PRIMARY KEY,
                    health_json TEXT NOT NULL,
                    updated_at INTEGER NOT NULL
                )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // W36: per-widget runtime snapshot. One row per (dashboard,
        // widget) — only the latest successful render is kept so the
        // table stays bounded by the size of the user's dashboard set.
        // Fingerprints guard against showing stale data after the
        // widget's binding, pipeline, or parameter values change.
        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS widget_runtime_snapshots (
                    dashboard_id TEXT NOT NULL REFERENCES dashboards(id) ON DELETE CASCADE,
                    widget_id TEXT NOT NULL,
                    widget_kind TEXT NOT NULL,
                    runtime_data TEXT NOT NULL,
                    captured_at INTEGER NOT NULL,
                    workflow_id TEXT,
                    workflow_run_id TEXT,
                    datasource_definition_id TEXT,
                    config_fingerprint TEXT NOT NULL,
                    parameter_fingerprint TEXT NOT NULL,
                    PRIMARY KEY (dashboard_id, widget_id)
                )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS widget_runtime_snapshots_dashboard_idx \
             ON widget_runtime_snapshots (dashboard_id)",
        )
        .execute(&self.pool)
        .await?;

        // W37: per-user state for the built-in external source catalog.
        // The catalog itself is static Rust data; only enablement and
        // optional credentials live in the DB.
        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS external_source_state (
                    source_id TEXT PRIMARY KEY,
                    is_enabled INTEGER NOT NULL DEFAULT 0,
                    credential TEXT,
                    updated_at INTEGER NOT NULL
                )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // W51: bounded local raw-artifact retention. One row per
        // compressed provider-visible artifact; the compact summary
        // that actually shipped is kept on the originating
        // `ToolResult` / `WidgetTrace` row, while this table holds the
        // redacted raw payload so debug surfaces and the
        // `inspect_artifact` tool can return bounded slices later.
        // `payload_json` is post-redaction — we never persist headers,
        // API keys, or bearer tokens we wouldn't be willing to copy
        // into Pipeline Debug.
        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS raw_artifacts (
                    id TEXT PRIMARY KEY,
                    owner_kind TEXT NOT NULL,
                    owner_id TEXT NOT NULL,
                    profile TEXT NOT NULL,
                    raw_size INTEGER NOT NULL,
                    compact_size INTEGER NOT NULL,
                    checksum TEXT NOT NULL,
                    redaction_version INTEGER NOT NULL,
                    retention_class TEXT NOT NULL,
                    payload_json TEXT NOT NULL,
                    created_at INTEGER NOT NULL
                )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS raw_artifacts_owner_idx \
             ON raw_artifacts (owner_kind, owner_id, created_at DESC)",
        )
        .execute(&self.pool)
        .await?;

        info!("✅ Database migrations complete");
        Ok(())
    }

    // ─── W51: raw artifact retention ───────────────────────────────────────

    /// Persist the redacted raw payload that backed a compressed
    /// artifact. Returns the generated id which the caller stashes on
    /// the `ToolResult` / `WidgetTrace` so the model and the UI can
    /// reference it later through `inspect_artifact`.
    ///
    /// `retention_class` controls cleanup: `"session"` keeps the row
    /// alive while the session is open, `"ephemeral"` is eligible for
    /// trim as soon as the per-owner cap kicks in, `"audit"` is kept
    /// regardless of the cap.
    #[allow(clippy::too_many_arguments)]
    pub async fn store_raw_artifact(
        &self,
        owner_kind: &str,
        owner_id: &str,
        profile: &str,
        raw_size: usize,
        compact_size: usize,
        checksum: &str,
        redaction_version: u32,
        retention_class: &str,
        payload_json: &str,
    ) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            r#"
                INSERT INTO raw_artifacts (
                    id, owner_kind, owner_id, profile, raw_size, compact_size,
                    checksum, redaction_version, retention_class, payload_json, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&id)
        .bind(owner_kind)
        .bind(owner_id)
        .bind(profile)
        .bind(raw_size as i64)
        .bind(compact_size as i64)
        .bind(checksum)
        .bind(redaction_version as i64)
        .bind(retention_class)
        .bind(payload_json)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    /// Fetch a raw artifact by id. Returns `None` when the row was
    /// pruned, never written, or belongs to a different installation.
    pub async fn get_raw_artifact(&self, id: &str) -> Result<Option<RawArtifactRecord>> {
        let row = sqlx::query(
            r#"
                SELECT id, owner_kind, owner_id, profile, raw_size, compact_size,
                       checksum, redaction_version, retention_class, payload_json, created_at
                FROM raw_artifacts
                WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            None => Ok(None),
            Some(row) => Ok(Some(RawArtifactRecord {
                id: row.try_get("id")?,
                owner_kind: row.try_get("owner_kind")?,
                owner_id: row.try_get("owner_id")?,
                profile: row.try_get("profile")?,
                raw_size: row.try_get::<i64, _>("raw_size")? as usize,
                compact_size: row.try_get::<i64, _>("compact_size")? as usize,
                checksum: row.try_get("checksum")?,
                redaction_version: row.try_get::<i64, _>("redaction_version")? as u32,
                retention_class: row.try_get("retention_class")?,
                payload_json: row.try_get("payload_json")?,
                created_at: row.try_get("created_at")?,
            })),
        }
    }

    /// Trim `ephemeral` raw artifacts per owner, keeping the most
    /// recent `keep_last` rows. `session` / `audit` retention classes
    /// are exempt — only ephemeral debug captures get bounded here.
    pub async fn prune_raw_artifacts(
        &self,
        owner_kind: &str,
        owner_id: &str,
        keep_last: u32,
    ) -> Result<u64> {
        let result = sqlx::query(
            r#"
                DELETE FROM raw_artifacts
                WHERE owner_kind = ?
                  AND owner_id = ?
                  AND retention_class = 'ephemeral'
                  AND id NOT IN (
                      SELECT id FROM raw_artifacts
                      WHERE owner_kind = ?
                        AND owner_id = ?
                        AND retention_class = 'ephemeral'
                      ORDER BY created_at DESC
                      LIMIT ?
                  )
            "#,
        )
        .bind(owner_kind)
        .bind(owner_id)
        .bind(owner_kind)
        .bind(owner_id)
        .bind(keep_last as i64)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    // ─── W37: external source state ────────────────────────────────────────

    pub async fn get_external_source_state(
        &self,
        source_id: &str,
    ) -> Result<Option<(bool, Option<String>, i64)>> {
        let row = sqlx::query(
            "SELECT is_enabled, credential, updated_at FROM external_source_state WHERE source_id = ?",
        )
        .bind(source_id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            None => Ok(None),
            Some(r) => {
                let enabled: i64 = r.try_get("is_enabled")?;
                let credential: Option<String> = r.try_get("credential").ok().flatten();
                let updated_at: i64 = r.try_get("updated_at")?;
                Ok(Some((enabled != 0, credential, updated_at)))
            }
        }
    }

    pub async fn upsert_external_source_state(
        &self,
        source_id: &str,
        is_enabled: bool,
        credential: Option<&str>,
        updated_at: i64,
    ) -> Result<()> {
        sqlx::query(
            r#"
                INSERT INTO external_source_state (source_id, is_enabled, credential, updated_at)
                VALUES (?, ?, ?, ?)
                ON CONFLICT(source_id) DO UPDATE SET
                    is_enabled = excluded.is_enabled,
                    credential = COALESCE(excluded.credential, external_source_state.credential),
                    updated_at = excluded.updated_at
            "#,
        )
        .bind(source_id)
        .bind(if is_enabled { 1i64 } else { 0i64 })
        .bind(credential)
        .bind(updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn clear_external_source_credential(&self, source_id: &str) -> Result<()> {
        sqlx::query(
            "UPDATE external_source_state SET credential = NULL, updated_at = ? WHERE source_id = ?",
        )
        .bind(chrono::Utc::now().timestamp_millis())
        .bind(source_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ─── Agent Memory ───────────────────────────────────────────────────────

    pub async fn insert_memory(&self, record: &MemoryRecord) -> Result<()> {
        sqlx::query(
            r#"
                INSERT INTO agent_memory (
                    id, scope, scope_id, kind, content, metadata,
                    created_at, accessed_count, last_accessed_at, expires_at, compressed_into
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&record.id)
        .bind(record.scope.discriminator())
        .bind(record.scope.scope_id())
        .bind(record.kind.as_str())
        .bind(&record.content)
        .bind(serde_json::to_string(&record.metadata)?)
        .bind(record.created_at)
        .bind(record.accessed_count)
        .bind(record.last_accessed_at)
        .bind(record.expires_at)
        .bind(record.compressed_into.as_deref())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_memory(&self, id: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM agent_memory WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn list_memories(&self) -> Result<Vec<MemoryRecord>> {
        let rows = sqlx::query(
            "SELECT id, scope, scope_id, kind, content, metadata, created_at, \
             accessed_count, last_accessed_at, expires_at, compressed_into \
             FROM agent_memory ORDER BY created_at DESC LIMIT 500",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(Self::row_to_memory).collect()
    }

    pub async fn touch_memory_access(&self, id: &str) -> Result<()> {
        sqlx::query(
            "UPDATE agent_memory SET accessed_count = accessed_count + 1, last_accessed_at = ? \
             WHERE id = ?",
        )
        .bind(chrono::Utc::now().timestamp_millis())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// FTS5 BM25 keyword search restricted to the given scope filters.
    /// `scope_filters` is a list of `(scope_discriminator, scope_id?)`;
    /// rows matching ANY pair are returned. Empty filter == all rows.
    pub async fn search_memories_fts(
        &self,
        query: &str,
        scope_filters: &[(String, Option<String>)],
        limit: usize,
    ) -> Result<Vec<(MemoryRecord, f64)>> {
        let trimmed = sanitize_fts_query(query);
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }

        let mut sql = String::from(
            "SELECT m.id, m.scope, m.scope_id, m.kind, m.content, m.metadata, m.created_at, \
             m.accessed_count, m.last_accessed_at, m.expires_at, m.compressed_into, \
             bm25(agent_memory_fts) AS rank \
             FROM agent_memory_fts JOIN agent_memory m ON m.rowid = agent_memory_fts.rowid \
             WHERE agent_memory_fts MATCH ? AND m.compressed_into IS NULL",
        );

        if !scope_filters.is_empty() {
            sql.push_str(" AND (");
            for idx in 0..scope_filters.len() {
                if idx > 0 {
                    sql.push_str(" OR ");
                }
                sql.push_str("(m.scope = ? AND (? IS NULL OR m.scope_id = ?))");
            }
            sql.push(')');
        }

        sql.push_str(" ORDER BY rank LIMIT ?");

        let mut q = sqlx::query(&sql).bind(&trimmed);
        for (scope, scope_id) in scope_filters {
            q = q.bind(scope).bind(scope_id.clone()).bind(scope_id.clone());
        }
        q = q.bind(limit as i64);

        let rows = q.fetch_all(&self.pool).await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let rank: f64 = row.try_get::<f64, _>("rank").unwrap_or(0.0);
            out.push((Self::row_to_memory(&row)?, rank));
        }
        Ok(out)
    }

    /// Browse memories by scope (admin UI / lessons extractor).
    pub async fn list_memories_by_scope(
        &self,
        scope_discriminator: &str,
        scope_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>> {
        let rows = sqlx::query(
            "SELECT id, scope, scope_id, kind, content, metadata, created_at, \
             accessed_count, last_accessed_at, expires_at, compressed_into \
             FROM agent_memory WHERE scope = ? \
             AND (? IS NULL OR scope_id = ?) AND compressed_into IS NULL \
             ORDER BY created_at DESC LIMIT ?",
        )
        .bind(scope_discriminator)
        .bind(scope_id)
        .bind(scope_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(Self::row_to_memory).collect()
    }

    fn row_to_memory(row: &sqlx::sqlite::SqliteRow) -> Result<MemoryRecord> {
        let scope_kind: String = row.try_get("scope")?;
        let scope_id: Option<String> = row.try_get("scope_id")?;
        let scope = match (scope_kind.as_str(), scope_id) {
            ("dashboard", Some(id)) => Scope::Dashboard(id),
            ("mcp_server", Some(id)) => Scope::McpServer(id),
            ("session", Some(id)) => Scope::Session(id),
            _ => Scope::Global,
        };
        let metadata_json: String = row.try_get("metadata")?;
        let metadata: serde_json::Value =
            serde_json::from_str(&metadata_json).unwrap_or(serde_json::Value::Null);
        Ok(MemoryRecord {
            id: row.try_get("id")?,
            scope,
            kind: MemoryKind::from_str(&row.try_get::<String, _>("kind")?),
            content: row.try_get("content")?,
            metadata,
            created_at: row.try_get("created_at")?,
            accessed_count: row.try_get("accessed_count")?,
            last_accessed_at: row.try_get("last_accessed_at")?,
            expires_at: row.try_get("expires_at")?,
            compressed_into: row.try_get("compressed_into")?,
        })
    }

    // ─── MCP Tool Observed Shapes ───────────────────────────────────────────

    pub async fn upsert_tool_shape(&self, shape: &ToolShape) -> Result<()> {
        sqlx::query(
            r#"
                INSERT INTO mcp_tool_observed_shape (
                    id, server_id, tool_name, args_fingerprint, shape_summary, shape_full,
                    sample_path, observed_at, observation_count
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(server_id, tool_name, args_fingerprint) DO UPDATE SET
                    shape_summary = excluded.shape_summary,
                    shape_full = excluded.shape_full,
                    sample_path = COALESCE(excluded.sample_path, mcp_tool_observed_shape.sample_path),
                    observed_at = excluded.observed_at,
                    observation_count = mcp_tool_observed_shape.observation_count + 1
            "#,
        )
        .bind(&shape.id)
        .bind(&shape.server_id)
        .bind(&shape.tool_name)
        .bind(&shape.args_fingerprint)
        .bind(&shape.shape_summary)
        .bind(&shape.shape_full)
        .bind(shape.sample_path.as_deref())
        .bind(shape.observed_at)
        .bind(shape.observation_count)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn lookup_tool_shape(
        &self,
        server_id: &str,
        tool_name: &str,
        args_fingerprint: &str,
    ) -> Result<Option<ToolShape>> {
        let row = sqlx::query(
            "SELECT id, server_id, tool_name, args_fingerprint, shape_summary, shape_full, \
             sample_path, observed_at, observation_count \
             FROM mcp_tool_observed_shape \
             WHERE server_id = ? AND tool_name = ? AND args_fingerprint = ?",
        )
        .bind(server_id)
        .bind(tool_name)
        .bind(args_fingerprint)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => Ok(Some(Self::row_to_tool_shape(&r)?)),
            None => Ok(None),
        }
    }

    pub async fn list_tool_shapes_for_server(
        &self,
        server_id: &str,
        limit: usize,
    ) -> Result<Vec<ToolShape>> {
        let rows = sqlx::query(
            "SELECT id, server_id, tool_name, args_fingerprint, shape_summary, shape_full, \
             sample_path, observed_at, observation_count \
             FROM mcp_tool_observed_shape WHERE server_id = ? \
             ORDER BY observation_count DESC, observed_at DESC LIMIT ?",
        )
        .bind(server_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(Self::row_to_tool_shape).collect()
    }

    fn row_to_tool_shape(row: &sqlx::sqlite::SqliteRow) -> Result<ToolShape> {
        Ok(ToolShape {
            id: row.try_get("id")?,
            server_id: row.try_get("server_id")?,
            tool_name: row.try_get("tool_name")?,
            args_fingerprint: row.try_get("args_fingerprint")?,
            shape_summary: row.try_get("shape_summary")?,
            shape_full: row.try_get("shape_full")?,
            sample_path: row.try_get("sample_path")?,
            observed_at: row.try_get("observed_at")?,
            observation_count: row.try_get("observation_count")?,
        })
    }

    // ─── Dashboards ─────────────────────────────────────────────────────────

    pub async fn list_dashboards(&self) -> Result<Vec<Dashboard>> {
        let rows = sqlx::query("SELECT * FROM dashboards ORDER BY updated_at DESC")
            .fetch_all(&self.pool)
            .await?;

        let mut dashboards = Vec::new();
        for row in rows {
            dashboards.push(Self::row_to_dashboard(&row)?);
        }
        Ok(dashboards)
    }

    pub async fn get_dashboard(&self, id: &str) -> Result<Option<Dashboard>> {
        let row = sqlx::query("SELECT * FROM dashboards WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

        match row {
            Some(r) => Ok(Some(Self::row_to_dashboard(&r)?)),
            None => Ok(None),
        }
    }

    pub async fn create_dashboard(&self, dashboard: &Dashboard) -> Result<()> {
        let model_policy_json = match dashboard.model_policy.as_ref() {
            Some(policy) => Some(serde_json::to_string(policy)?),
            None => None,
        };
        let language_policy_json = match dashboard.language_policy.as_ref() {
            Some(policy) => Some(serde_json::to_string(policy)?),
            None => None,
        };
        sqlx::query(r#"
            INSERT INTO dashboards (id, name, description, layout, workflows, is_default, created_at, updated_at, parameters, model_policy, language_policy)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#)
        .bind(&dashboard.id)
        .bind(&dashboard.name)
        .bind(&dashboard.description)
        .bind(serde_json::to_string(&dashboard.layout)?)
        .bind(serde_json::to_string(&dashboard.workflows)?)
        .bind(if dashboard.is_default { 1i64 } else { 0i64 })
        .bind(dashboard.created_at)
        .bind(dashboard.updated_at)
        .bind(serde_json::to_string(&dashboard.parameters)?)
        .bind(model_policy_json)
        .bind(language_policy_json)
        .execute(&self.pool).await?;

        Ok(())
    }

    pub async fn update_dashboard(&self, dashboard: &Dashboard) -> Result<()> {
        let model_policy_json = match dashboard.model_policy.as_ref() {
            Some(policy) => Some(serde_json::to_string(policy)?),
            None => None,
        };
        let language_policy_json = match dashboard.language_policy.as_ref() {
            Some(policy) => Some(serde_json::to_string(policy)?),
            None => None,
        };
        sqlx::query(
            r#"
            UPDATE dashboards SET name = ?, description = ?, layout = ?, workflows = ?,
            is_default = ?, updated_at = ?, parameters = ?, model_policy = ?,
            language_policy = ? WHERE id = ?
        "#,
        )
        .bind(&dashboard.name)
        .bind(&dashboard.description)
        .bind(serde_json::to_string(&dashboard.layout)?)
        .bind(serde_json::to_string(&dashboard.workflows)?)
        .bind(if dashboard.is_default { 1i64 } else { 0i64 })
        .bind(dashboard.updated_at)
        .bind(serde_json::to_string(&dashboard.parameters)?)
        .bind(model_policy_json)
        .bind(language_policy_json)
        .bind(&dashboard.id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn delete_dashboard(&self, id: &str) -> Result<()> {
        // FK cascade is declarative only — SQLite needs `PRAGMA
        // foreign_keys = ON` per connection to enforce it, and the
        // pool here doesn't set that. Defensive deletes keep child
        // tables clean even on older databases.
        sqlx::query("DELETE FROM widget_runtime_snapshots WHERE dashboard_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM dashboards WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ─── W36: widget runtime snapshots ──────────────────────────────────────

    pub async fn upsert_widget_snapshot(&self, snapshot: &WidgetRuntimeSnapshot) -> Result<()> {
        let runtime_json = serde_json::to_string(&snapshot.runtime_data)?;
        sqlx::query(
            r#"
                INSERT INTO widget_runtime_snapshots (
                    dashboard_id, widget_id, widget_kind, runtime_data,
                    captured_at, workflow_id, workflow_run_id,
                    datasource_definition_id, config_fingerprint, parameter_fingerprint
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(dashboard_id, widget_id) DO UPDATE SET
                    widget_kind = excluded.widget_kind,
                    runtime_data = excluded.runtime_data,
                    captured_at = excluded.captured_at,
                    workflow_id = excluded.workflow_id,
                    workflow_run_id = excluded.workflow_run_id,
                    datasource_definition_id = excluded.datasource_definition_id,
                    config_fingerprint = excluded.config_fingerprint,
                    parameter_fingerprint = excluded.parameter_fingerprint
            "#,
        )
        .bind(&snapshot.dashboard_id)
        .bind(&snapshot.widget_id)
        .bind(&snapshot.widget_kind)
        .bind(runtime_json)
        .bind(snapshot.captured_at)
        .bind(snapshot.workflow_id.as_deref())
        .bind(snapshot.workflow_run_id.as_deref())
        .bind(snapshot.datasource_definition_id.as_deref())
        .bind(&snapshot.config_fingerprint)
        .bind(&snapshot.parameter_fingerprint)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_widget_snapshots(
        &self,
        dashboard_id: &str,
    ) -> Result<Vec<WidgetRuntimeSnapshot>> {
        let rows = sqlx::query(
            "SELECT dashboard_id, widget_id, widget_kind, runtime_data, captured_at, \
             workflow_id, workflow_run_id, datasource_definition_id, \
             config_fingerprint, parameter_fingerprint \
             FROM widget_runtime_snapshots WHERE dashboard_id = ?",
        )
        .bind(dashboard_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(Self::row_to_widget_snapshot).collect()
    }

    pub async fn delete_widget_snapshot(&self, dashboard_id: &str, widget_id: &str) -> Result<()> {
        sqlx::query(
            "DELETE FROM widget_runtime_snapshots WHERE dashboard_id = ? AND widget_id = ?",
        )
        .bind(dashboard_id)
        .bind(widget_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    fn row_to_widget_snapshot(row: &sqlx::sqlite::SqliteRow) -> Result<WidgetRuntimeSnapshot> {
        let runtime_json: String = row.try_get("runtime_data")?;
        let runtime_data: serde_json::Value =
            serde_json::from_str(&runtime_json).unwrap_or(serde_json::Value::Null);
        Ok(WidgetRuntimeSnapshot {
            dashboard_id: row.try_get("dashboard_id")?,
            widget_id: row.try_get("widget_id")?,
            widget_kind: row.try_get("widget_kind")?,
            runtime_data,
            captured_at: row.try_get("captured_at")?,
            workflow_id: row.try_get("workflow_id").ok(),
            workflow_run_id: row.try_get("workflow_run_id").ok(),
            datasource_definition_id: row.try_get("datasource_definition_id").ok(),
            config_fingerprint: row.try_get("config_fingerprint")?,
            parameter_fingerprint: row.try_get("parameter_fingerprint")?,
        })
    }

    // ─── W25: parameter selections ──────────────────────────────────────────

    /// Load every persisted (param_name -> value) selection for a dashboard.
    pub async fn get_dashboard_parameter_values(
        &self,
        dashboard_id: &str,
    ) -> Result<std::collections::BTreeMap<String, crate::models::dashboard::ParameterValue>> {
        let rows = sqlx::query(
            "SELECT param_name, value_json FROM dashboard_parameter_values WHERE dashboard_id = ?",
        )
        .bind(dashboard_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = std::collections::BTreeMap::new();
        for row in rows {
            let name: String = row.try_get("param_name")?;
            let value_json: String = row.try_get("value_json")?;
            if let Ok(value) =
                serde_json::from_str::<crate::models::dashboard::ParameterValue>(&value_json)
            {
                out.insert(name, value);
            }
        }
        Ok(out)
    }

    pub async fn set_dashboard_parameter_value(
        &self,
        dashboard_id: &str,
        param_name: &str,
        value: &crate::models::dashboard::ParameterValue,
        now: i64,
    ) -> Result<()> {
        let value_json = serde_json::to_string(value)?;
        sqlx::query(
            r#"
                INSERT INTO dashboard_parameter_values (dashboard_id, param_name, value_json, updated_at)
                VALUES (?, ?, ?, ?)
                ON CONFLICT(dashboard_id, param_name) DO UPDATE SET
                    value_json = excluded.value_json,
                    updated_at = excluded.updated_at
            "#,
        )
        .bind(dashboard_id)
        .bind(param_name)
        .bind(value_json)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_dashboard_parameter_value(
        &self,
        dashboard_id: &str,
        param_name: &str,
    ) -> Result<()> {
        sqlx::query(
            "DELETE FROM dashboard_parameter_values WHERE dashboard_id = ? AND param_name = ?",
        )
        .bind(dashboard_id)
        .bind(param_name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    fn row_to_dashboard(row: &sqlx::sqlite::SqliteRow) -> Result<Dashboard> {
        let layout_json: String = row.try_get("layout")?;
        let workflows_json: String = row.try_get("workflows")?;
        // W25: `parameters` column is added in a separate ALTER, so older
        // rows may not have it; treat missing / null / invalid as empty.
        let parameters_json: Option<String> = row.try_get("parameters").ok();
        let parameters = parameters_json
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        // W43: `model_policy` is another late-ALTER column. Null/missing
        // means "no dashboard-level policy", which is the same as never
        // setting one — runtime falls back to the app active provider.
        let model_policy_json: Option<String> = row.try_get("model_policy").ok().flatten();
        let model_policy = model_policy_json
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .and_then(|s| serde_json::from_str(s).ok());

        // W47: dashboard-level assistant language override; same
        // missing/empty handling as model_policy.
        let language_policy_json: Option<String> = row.try_get("language_policy").ok().flatten();
        let language_policy = language_policy_json
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .and_then(|s| serde_json::from_str(s).ok());

        Ok(Dashboard {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            layout: serde_json::from_str(&layout_json)?,
            workflows: serde_json::from_str(&workflows_json)?,
            is_default: row.try_get::<i64, _>("is_default")? == 1,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            parameters,
            language_policy,
            model_policy,
        })
    }

    // ─── W19: Dashboard versions ────────────────────────────────────────────

    /// Newest entries kept; older ones pruned in the same transaction as
    /// the insert. 30 keeps roughly a month of normal use without
    /// ballooning the SQLite db for users who hammer Apply.
    const MAX_VERSIONS_PER_DASHBOARD: i64 = 30;

    /// Insert a snapshot row for `dashboard`. Wraps the insert + prune in
    /// a single transaction so we never observe a half-pruned window.
    pub async fn insert_dashboard_version(
        &self,
        version_id: &str,
        dashboard: &Dashboard,
        source: VersionSource,
        summary: &str,
        source_session_id: Option<&str>,
        parent_version_id: Option<&str>,
        applied_at: i64,
    ) -> Result<DashboardVersionSummary> {
        let snapshot_json = serde_json::to_string(dashboard)?;
        let widget_count = dashboard.layout.len() as i64;

        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
                INSERT INTO dashboard_versions (
                    id, dashboard_id, snapshot_json, applied_at, source,
                    source_session_id, summary, widget_count, parent_version_id
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(version_id)
        .bind(&dashboard.id)
        .bind(&snapshot_json)
        .bind(applied_at)
        .bind(source.as_str())
        .bind(source_session_id)
        .bind(summary)
        .bind(widget_count)
        .bind(parent_version_id)
        .execute(&mut *tx)
        .await?;

        // Prune any rows beyond the most recent MAX_VERSIONS_PER_DASHBOARD.
        sqlx::query(
            r#"
                DELETE FROM dashboard_versions
                WHERE dashboard_id = ?
                  AND id NOT IN (
                    SELECT id FROM dashboard_versions
                    WHERE dashboard_id = ?
                    ORDER BY applied_at DESC, id DESC
                    LIMIT ?
                  )
            "#,
        )
        .bind(&dashboard.id)
        .bind(&dashboard.id)
        .bind(Self::MAX_VERSIONS_PER_DASHBOARD)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(DashboardVersionSummary {
            id: version_id.to_string(),
            dashboard_id: dashboard.id.clone(),
            applied_at,
            source,
            summary: summary.to_string(),
            widget_count: widget_count as i32,
            source_session_id: source_session_id.map(str::to_string),
            parent_version_id: parent_version_id.map(str::to_string),
        })
    }

    pub async fn list_dashboard_versions(
        &self,
        dashboard_id: &str,
    ) -> Result<Vec<DashboardVersionSummary>> {
        let rows = sqlx::query(
            "SELECT id, dashboard_id, applied_at, source, source_session_id, \
             summary, widget_count, parent_version_id \
             FROM dashboard_versions WHERE dashboard_id = ? \
             ORDER BY applied_at DESC, id DESC",
        )
        .bind(dashboard_id)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(Self::row_to_version_summary(&row)?);
        }
        Ok(out)
    }

    pub async fn get_dashboard_version(
        &self,
        version_id: &str,
    ) -> Result<Option<DashboardVersion>> {
        let row = sqlx::query(
            "SELECT id, dashboard_id, snapshot_json, applied_at, source, \
             source_session_id, summary, widget_count, parent_version_id \
             FROM dashboard_versions WHERE id = ?",
        )
        .bind(version_id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => Ok(Some(Self::row_to_version_full(&r)?)),
            None => Ok(None),
        }
    }

    fn row_to_version_summary(row: &sqlx::sqlite::SqliteRow) -> Result<DashboardVersionSummary> {
        let source_str: String = row.try_get("source")?;
        let source = VersionSource::from_str(&source_str)
            .ok_or_else(|| anyhow::anyhow!("unknown version source '{}'", source_str))?;
        Ok(DashboardVersionSummary {
            id: row.try_get("id")?,
            dashboard_id: row.try_get("dashboard_id")?,
            applied_at: row.try_get("applied_at")?,
            source,
            summary: row.try_get("summary")?,
            widget_count: row.try_get::<i64, _>("widget_count")? as i32,
            source_session_id: row.try_get("source_session_id")?,
            parent_version_id: row.try_get("parent_version_id")?,
        })
    }

    fn row_to_version_full(row: &sqlx::sqlite::SqliteRow) -> Result<DashboardVersion> {
        let source_str: String = row.try_get("source")?;
        let source = VersionSource::from_str(&source_str)
            .ok_or_else(|| anyhow::anyhow!("unknown version source '{}'", source_str))?;
        let snapshot_json: String = row.try_get("snapshot_json")?;
        let snapshot: Dashboard = serde_json::from_str(&snapshot_json)?;
        Ok(DashboardVersion {
            id: row.try_get("id")?,
            dashboard_id: row.try_get("dashboard_id")?,
            applied_at: row.try_get("applied_at")?,
            source,
            summary: row.try_get("summary")?,
            widget_count: row.try_get::<i64, _>("widget_count")? as i32,
            source_session_id: row.try_get("source_session_id")?,
            parent_version_id: row.try_get("parent_version_id")?,
            snapshot,
        })
    }

    // ─── Chat Sessions ──────────────────────────────────────────────────────

    pub async fn list_chat_sessions(&self) -> Result<Vec<ChatSession>> {
        let rows = sqlx::query("SELECT * FROM chat_sessions ORDER BY updated_at DESC")
            .fetch_all(&self.pool)
            .await?;

        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(Self::row_to_chat_session(&row)?);
        }
        Ok(sessions)
    }

    /// Lightweight listing for the sidebar: no full `messages` array, just
    /// id/title/mode/dashboard, message count, and a 200-char preview of the
    /// first user message. Keeps IPC payload tiny even with many sessions.
    pub async fn list_chat_session_summaries(
        &self,
    ) -> Result<Vec<crate::models::chat::ChatSessionSummary>> {
        let rows = sqlx::query(
            "SELECT id, mode, dashboard_id, widget_id, title, created_at, updated_at,
                json_array_length(messages) AS message_count,
                json_extract(messages, '$[0].content') AS preview
             FROM chat_sessions
             ORDER BY updated_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut sessions = Vec::new();
        for row in rows {
            let preview: Option<String> = row
                .try_get::<Option<String>, _>("preview")
                .ok()
                .flatten()
                .map(|s| {
                    let trimmed = s.trim().replace('\n', " ");
                    if trimmed.chars().count() > 200 {
                        format!("{}...", trimmed.chars().take(200).collect::<String>())
                    } else {
                        trimmed
                    }
                });
            sessions.push(crate::models::chat::ChatSessionSummary {
                id: row.try_get("id")?,
                mode: match row.try_get::<String, _>("mode")?.as_str() {
                    "build" => crate::models::chat::ChatMode::Build,
                    _ => crate::models::chat::ChatMode::Context,
                },
                dashboard_id: row.try_get("dashboard_id")?,
                widget_id: row.try_get("widget_id")?,
                title: row.try_get("title")?,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
                message_count: row
                    .try_get::<Option<i64>, _>("message_count")?
                    .unwrap_or(0)
                    .max(0) as u32,
                preview,
            });
        }
        Ok(sessions)
    }

    pub async fn get_chat_session(&self, id: &str) -> Result<Option<ChatSession>> {
        let row = sqlx::query("SELECT * FROM chat_sessions WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

        match row {
            Some(r) => Ok(Some(Self::row_to_chat_session(&r)?)),
            None => Ok(None),
        }
    }

    pub async fn create_chat_session(&self, session: &ChatSession) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO chat_sessions (
                id, mode, dashboard_id, widget_id, title, messages,
                current_plan, plan_status,
                total_input_tokens, total_output_tokens, total_reasoning_tokens,
                total_cost_usd, max_cost_usd, language_override,
                cost_unknown_turns,
                created_at, updated_at
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
        )
        .bind(&session.id)
        .bind(match session.mode {
            crate::models::chat::ChatMode::Build => "build",
            crate::models::chat::ChatMode::Context => "context",
        })
        .bind(&session.dashboard_id)
        .bind(&session.widget_id)
        .bind(&session.title)
        .bind(serde_json::to_string(&session.messages)?)
        .bind(match session.current_plan.as_ref() {
            Some(plan) => Some(serde_json::to_string(plan)?),
            None => None,
        })
        .bind(match session.plan_status.as_ref() {
            Some(status) => Some(serde_json::to_string(status)?),
            None => None,
        })
        .bind(session.total_input_tokens as i64)
        .bind(session.total_output_tokens as i64)
        .bind(session.total_reasoning_tokens as i64)
        .bind(session.total_cost_usd)
        .bind(session.max_cost_usd)
        .bind(match session.language_override.as_ref() {
            Some(policy) => Some(serde_json::to_string(policy)?),
            None => None,
        })
        .bind(session.cost_unknown_turns as i64)
        .bind(session.created_at)
        .bind(session.updated_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn update_chat_session(&self, session: &ChatSession) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE chat_sessions SET
                mode = ?, dashboard_id = ?, widget_id = ?, title = ?, messages = ?,
                current_plan = ?, plan_status = ?,
                total_input_tokens = ?, total_output_tokens = ?, total_reasoning_tokens = ?,
                total_cost_usd = ?, max_cost_usd = ?, language_override = ?,
                cost_unknown_turns = ?,
                updated_at = ?
            WHERE id = ?
        "#,
        )
        .bind(match session.mode {
            crate::models::chat::ChatMode::Build => "build",
            crate::models::chat::ChatMode::Context => "context",
        })
        .bind(&session.dashboard_id)
        .bind(&session.widget_id)
        .bind(&session.title)
        .bind(serde_json::to_string(&session.messages)?)
        .bind(match session.current_plan.as_ref() {
            Some(plan) => Some(serde_json::to_string(plan)?),
            None => None,
        })
        .bind(match session.plan_status.as_ref() {
            Some(status) => Some(serde_json::to_string(status)?),
            None => None,
        })
        .bind(session.total_input_tokens as i64)
        .bind(session.total_output_tokens as i64)
        .bind(session.total_reasoning_tokens as i64)
        .bind(session.total_cost_usd)
        .bind(session.max_cost_usd)
        .bind(match session.language_override.as_ref() {
            Some(policy) => Some(serde_json::to_string(policy)?),
            None => None,
        })
        .bind(session.cost_unknown_turns as i64)
        .bind(session.updated_at)
        .bind(&session.id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// W47: write the per-session language override and return the
    /// refreshed session row. Mirrors [`Self::set_session_max_cost`] so
    /// flipping a language doesn't require re-serialising the whole
    /// `messages` blob.
    pub async fn set_session_language_override(
        &self,
        session_id: &str,
        policy: Option<&crate::models::language::AssistantLanguagePolicy>,
    ) -> Result<Option<ChatSession>> {
        let payload = match policy {
            Some(policy) => Some(serde_json::to_string(policy)?),
            None => None,
        };
        sqlx::query("UPDATE chat_sessions SET language_override = ?, updated_at = ? WHERE id = ?")
            .bind(payload)
            .bind(chrono::Utc::now().timestamp_millis())
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        self.get_chat_session(session_id).await
    }

    /// W22: lightweight update path used by the streaming chat command
    /// when only the running totals need to be flushed (e.g. after a
    /// resume turn). Avoids re-serialising the whole `messages` blob.
    pub async fn update_chat_session_totals(
        &self,
        session_id: &str,
        total_input_tokens: u64,
        total_output_tokens: u64,
        total_reasoning_tokens: u64,
        total_cost_usd: f64,
        updated_at: i64,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE chat_sessions SET
                total_input_tokens = ?,
                total_output_tokens = ?,
                total_reasoning_tokens = ?,
                total_cost_usd = ?,
                updated_at = ?
            WHERE id = ?
        "#,
        )
        .bind(total_input_tokens as i64)
        .bind(total_output_tokens as i64)
        .bind(total_reasoning_tokens as i64)
        .bind(total_cost_usd)
        .bind(updated_at)
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// W22: set / clear the per-session budget cap. Returns the updated
    /// session row.
    pub async fn set_session_max_cost(
        &self,
        session_id: &str,
        max_cost_usd: Option<f64>,
    ) -> Result<Option<ChatSession>> {
        sqlx::query("UPDATE chat_sessions SET max_cost_usd = ? WHERE id = ?")
            .bind(max_cost_usd)
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        self.get_chat_session(session_id).await
    }

    pub async fn delete_chat_session(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM chat_sessions WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ─── W22: cost queries ──────────────────────────────────────────────────

    /// Sum of `total_cost_usd` across every session whose `updated_at`
    /// falls in `[since_ms, until_ms)`. The footer uses this to compute
    /// "today $X.XX" cheaply.
    pub async fn sum_cost_between(&self, since_ms: i64, until_ms: i64) -> Result<f64> {
        let row = sqlx::query(
            "SELECT COALESCE(SUM(total_cost_usd), 0.0) AS total \
             FROM chat_sessions WHERE updated_at >= ? AND updated_at < ?",
        )
        .bind(since_ms)
        .bind(until_ms)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get::<f64, _>("total").unwrap_or(0.0))
    }

    /// `(bucket_start_ms, cost_usd)` rows grouped by UTC day, sorted
    /// ascending by bucket. `since_ms` is inclusive; `until_ms` is
    /// exclusive. Used by Settings → Costs to draw the 30-day bar chart.
    pub async fn daily_cost_buckets(
        &self,
        since_ms: i64,
        until_ms: i64,
    ) -> Result<Vec<(i64, f64)>> {
        let rows = sqlx::query(
            "SELECT CAST(updated_at / 86400000 AS INTEGER) * 86400000 AS bucket,
                    SUM(total_cost_usd) AS total
             FROM chat_sessions
             WHERE updated_at >= ? AND updated_at < ?
             GROUP BY bucket
             ORDER BY bucket ASC",
        )
        .bind(since_ms)
        .bind(until_ms)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let bucket = row.try_get::<i64, _>("bucket").unwrap_or(0);
                let total = row.try_get::<f64, _>("total").unwrap_or(0.0);
                (bucket, total)
            })
            .collect())
    }

    /// Top-N sessions by `total_cost_usd`. Returns the lightweight
    /// summary rows the cost view needs (id, title, total, updated_at).
    pub async fn top_sessions_by_cost(
        &self,
        limit: i64,
    ) -> Result<Vec<crate::models::chat::CostSessionEntry>> {
        let rows = sqlx::query(
            "SELECT id, title, mode, total_cost_usd, total_input_tokens, total_output_tokens, \
             total_reasoning_tokens, updated_at \
             FROM chat_sessions \
             WHERE total_cost_usd > 0 \
             ORDER BY total_cost_usd DESC \
             LIMIT ?",
        )
        .bind(limit.max(1))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| crate::models::chat::CostSessionEntry {
                session_id: row.try_get::<String, _>("id").unwrap_or_default(),
                title: row.try_get::<String, _>("title").unwrap_or_default(),
                mode: row
                    .try_get::<String, _>("mode")
                    .map(|m| {
                        if m == "build" {
                            crate::models::chat::ChatMode::Build
                        } else {
                            crate::models::chat::ChatMode::Context
                        }
                    })
                    .unwrap_or(crate::models::chat::ChatMode::Context),
                cost_usd: row.try_get::<f64, _>("total_cost_usd").unwrap_or(0.0),
                input_tokens: row
                    .try_get::<i64, _>("total_input_tokens")
                    .unwrap_or(0)
                    .max(0) as u64,
                output_tokens: row
                    .try_get::<i64, _>("total_output_tokens")
                    .unwrap_or(0)
                    .max(0) as u64,
                reasoning_tokens: row
                    .try_get::<i64, _>("total_reasoning_tokens")
                    .unwrap_or(0)
                    .max(0) as u64,
                updated_at: row.try_get::<i64, _>("updated_at").unwrap_or(0),
            })
            .collect())
    }

    fn row_to_chat_session(row: &sqlx::sqlite::SqliteRow) -> Result<ChatSession> {
        let messages_json: String = row.try_get("messages")?;
        let current_plan_json: Option<String> = row
            .try_get::<Option<String>, _>("current_plan")
            .unwrap_or(None);
        let plan_status_json: Option<String> = row
            .try_get::<Option<String>, _>("plan_status")
            .unwrap_or(None);

        Ok(ChatSession {
            id: row.try_get("id")?,
            mode: match row.try_get::<String, _>("mode")?.as_str() {
                "build" => crate::models::chat::ChatMode::Build,
                _ => crate::models::chat::ChatMode::Context,
            },
            dashboard_id: row.try_get("dashboard_id")?,
            widget_id: row.try_get("widget_id")?,
            title: row.try_get("title")?,
            messages: serde_json::from_str(&messages_json)?,
            current_plan: current_plan_json
                .as_deref()
                .and_then(|json| serde_json::from_str(json).ok()),
            plan_status: plan_status_json
                .as_deref()
                .and_then(|json| serde_json::from_str(json).ok()),
            total_input_tokens: row
                .try_get::<Option<i64>, _>("total_input_tokens")
                .unwrap_or(None)
                .unwrap_or(0)
                .max(0) as u64,
            total_output_tokens: row
                .try_get::<Option<i64>, _>("total_output_tokens")
                .unwrap_or(None)
                .unwrap_or(0)
                .max(0) as u64,
            total_reasoning_tokens: row
                .try_get::<Option<i64>, _>("total_reasoning_tokens")
                .unwrap_or(None)
                .unwrap_or(0)
                .max(0) as u64,
            total_cost_usd: row
                .try_get::<Option<f64>, _>("total_cost_usd")
                .unwrap_or(None)
                .unwrap_or(0.0),
            max_cost_usd: row
                .try_get::<Option<f64>, _>("max_cost_usd")
                .unwrap_or(None),
            cost_unknown_turns: row
                .try_get::<Option<i64>, _>("cost_unknown_turns")
                .unwrap_or(None)
                .unwrap_or(0)
                .max(0) as u32,
            language_override: row
                .try_get::<Option<String>, _>("language_override")
                .unwrap_or(None)
                .and_then(|json| serde_json::from_str(&json).ok()),
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }

    // ─── Config ─────────────────────────────────────────────────────────────

    pub async fn get_config(&self, key: &str) -> Result<Option<String>> {
        let row = sqlx::query("SELECT value FROM app_config WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;

        match row {
            Some(r) => Ok(Some(r.try_get("value")?)),
            None => Ok(None),
        }
    }

    pub async fn set_config(&self, key: &str, value: &str) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO app_config (key, value, updated_at) VALUES (?, ?, ?)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at
        "#,
        )
        .bind(key)
        .bind(value)
        .bind(chrono::Utc::now().timestamp_millis())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // ─── Workflows ──────────────────────────────────────────────────────────

    pub async fn list_workflows(&self) -> Result<Vec<Workflow>> {
        let rows = sqlx::query("SELECT * FROM workflows")
            .fetch_all(&self.pool)
            .await?;

        let mut workflows = Vec::new();
        for row in rows {
            workflows.push(Self::row_to_workflow(&row)?);
        }
        Ok(workflows)
    }

    pub async fn get_workflow(&self, id: &str) -> Result<Option<Workflow>> {
        let row = sqlx::query("SELECT * FROM workflows WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

        match row {
            Some(r) => Ok(Some(Self::row_to_workflow(&r)?)),
            None => Ok(None),
        }
    }

    pub async fn create_workflow(&self, workflow: &Workflow) -> Result<()> {
        sqlx::query(r#"
            INSERT INTO workflows (id, name, description, nodes, edges, trigger, is_enabled, last_run, created_at, updated_at, pause_state, last_paused_at, last_pause_reason)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#)
        .bind(&workflow.id)
        .bind(&workflow.name)
        .bind(&workflow.description)
        .bind(serde_json::to_string(&workflow.nodes)?)
        .bind(serde_json::to_string(&workflow.edges)?)
        .bind(serde_json::to_string(&workflow.trigger)?)
        .bind(if workflow.is_enabled { 1i64 } else { 0i64 })
        .bind(workflow.last_run.as_ref().map(serde_json::to_string).transpose()?)
        .bind(workflow.created_at)
        .bind(workflow.updated_at)
        .bind(workflow.pause_state.as_str())
        .bind(workflow.last_paused_at)
        .bind(workflow.last_pause_reason.as_deref())
        .execute(&self.pool).await?;

        Ok(())
    }

    pub async fn update_workflow_last_run(
        &self,
        workflow_id: &str,
        run: &crate::models::workflow::WorkflowRun,
    ) -> Result<()> {
        sqlx::query("UPDATE workflows SET last_run = ?, updated_at = ? WHERE id = ?")
            .bind(serde_json::to_string(run)?)
            .bind(chrono::Utc::now().timestamp_millis())
            .bind(workflow_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn save_workflow_run(
        &self,
        workflow_id: &str,
        run: &crate::models::workflow::WorkflowRun,
    ) -> Result<()> {
        sqlx::query(r#"
            INSERT INTO workflow_runs (id, workflow_id, started_at, finished_at, status, node_results, error)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
            workflow_id = excluded.workflow_id, started_at = excluded.started_at,
            finished_at = excluded.finished_at, status = excluded.status,
            node_results = excluded.node_results, error = excluded.error
        "#)
        .bind(&run.id)
        .bind(workflow_id)
        .bind(run.started_at)
        .bind(run.finished_at)
        .bind(serde_json::to_string(&run.status)?)
        .bind(run.node_results.as_ref().map(serde_json::to_string).transpose()?)
        .bind(&run.error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_workflow(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM workflows WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// W50: persist the pause flag + reason in one round-trip. Returns
    /// `false` when the workflow id is unknown so callers can surface a
    /// typed "not found" rather than a silent no-op.
    pub async fn set_workflow_pause_state(
        &self,
        workflow_id: &str,
        state: crate::models::workflow::SchedulePauseState,
        reason: Option<&str>,
        now: i64,
    ) -> Result<bool> {
        use crate::models::workflow::SchedulePauseState;
        let paused_at = if matches!(state, SchedulePauseState::Paused) {
            Some(now)
        } else {
            None
        };
        let stored_reason = if matches!(state, SchedulePauseState::Paused) {
            reason
        } else {
            None
        };
        let result = sqlx::query(
            "UPDATE workflows SET pause_state = ?, last_paused_at = ?, \
             last_pause_reason = ?, updated_at = ? WHERE id = ?",
        )
        .bind(state.as_str())
        .bind(paused_at)
        .bind(stored_reason)
        .bind(now)
        .bind(workflow_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// W50: replace the cron trigger config on an existing workflow. When
    /// `cron` is `None`, the trigger reverts to manual; passing the
    /// already-normalized cron string switches it to `Cron`. Returns
    /// `false` when the workflow id is unknown.
    pub async fn set_workflow_cron(
        &self,
        workflow_id: &str,
        cron: Option<&str>,
        now: i64,
    ) -> Result<bool> {
        use crate::models::workflow::{TriggerConfig, TriggerKind, WorkflowTrigger};
        let trigger = match cron {
            Some(value) => WorkflowTrigger {
                kind: TriggerKind::Cron,
                config: Some(TriggerConfig {
                    cron: Some(value.to_string()),
                    event: None,
                }),
            },
            None => WorkflowTrigger {
                kind: TriggerKind::Manual,
                config: None,
            },
        };
        let result = sqlx::query("UPDATE workflows SET trigger = ?, updated_at = ? WHERE id = ?")
            .bind(serde_json::to_string(&trigger)?)
            .bind(now)
            .bind(workflow_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// W35: Operations cockpit list. Cheap row summary that omits the
    /// potentially-large `node_results` JSON blob. `workflow_id` filters
    /// to a single workflow's run history; `None` returns all rows
    /// across workflows.
    pub async fn list_workflow_run_summaries(
        &self,
        workflow_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<crate::models::workflow::WorkflowRunSummary>> {
        let limit = limit.max(1).min(500);
        let rows = if let Some(wf) = workflow_id {
            sqlx::query(
                "SELECT id, workflow_id, started_at, finished_at, status, error, \
                 (node_results IS NOT NULL) AS has_node_results \
                 FROM workflow_runs WHERE workflow_id = ? \
                 ORDER BY started_at DESC LIMIT ?",
            )
            .bind(wf)
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT id, workflow_id, started_at, finished_at, status, error, \
                 (node_results IS NOT NULL) AS has_node_results \
                 FROM workflow_runs ORDER BY started_at DESC LIMIT ?",
            )
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?
        };
        rows.iter().map(Self::row_to_run_summary).collect()
    }

    /// W35: full run row including `node_results`. Used by the run
    /// detail view and by `retry_workflow_run` to look up the originating
    /// workflow id.
    pub async fn get_workflow_run(
        &self,
        run_id: &str,
    ) -> Result<Option<(String, crate::models::workflow::WorkflowRun)>> {
        let row = sqlx::query(
            "SELECT id, workflow_id, started_at, finished_at, status, node_results, error \
             FROM workflow_runs WHERE id = ?",
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else { return Ok(None) };
        let workflow_id: String = row.try_get("workflow_id")?;
        let status_raw: String = row.try_get("status")?;
        let status = serde_json::from_str(&status_raw)?;
        let node_results = row
            .try_get::<Option<String>, _>("node_results")?
            .map(|s| serde_json::from_str(&s))
            .transpose()?;
        let run = crate::models::workflow::WorkflowRun {
            id: row.try_get("id")?,
            started_at: row.try_get("started_at")?,
            finished_at: row.try_get("finished_at")?,
            status,
            node_results,
            error: row.try_get("error")?,
        };
        Ok(Some((workflow_id, run)))
    }

    fn row_to_run_summary(
        row: &sqlx::sqlite::SqliteRow,
    ) -> Result<crate::models::workflow::WorkflowRunSummary> {
        let status_raw: String = row.try_get("status")?;
        let status = serde_json::from_str(&status_raw)?;
        let started_at: i64 = row.try_get("started_at")?;
        let finished_at: Option<i64> = row.try_get("finished_at")?;
        let duration_ms = finished_at.map(|f| f.saturating_sub(started_at));
        let has_node_results: i64 = row.try_get("has_node_results")?;
        Ok(crate::models::workflow::WorkflowRunSummary {
            id: row.try_get("id")?,
            workflow_id: row.try_get("workflow_id")?,
            started_at,
            finished_at,
            status,
            duration_ms,
            error: row.try_get("error")?,
            has_node_results: has_node_results != 0,
        })
    }

    /// W19: restore path needs to put a workflow back to a known shape.
    /// We do not have an UPDATE on the full row elsewhere, so this is a
    /// DELETE + INSERT inside one transaction to keep the row atomically
    /// in sync with the restored snapshot.
    pub async fn upsert_workflow(&self, workflow: &Workflow) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM workflows WHERE id = ?")
            .bind(&workflow.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            r#"
                INSERT INTO workflows (
                    id, name, description, nodes, edges, trigger, is_enabled, last_run,
                    created_at, updated_at, pause_state, last_paused_at, last_pause_reason
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&workflow.id)
        .bind(&workflow.name)
        .bind(&workflow.description)
        .bind(serde_json::to_string(&workflow.nodes)?)
        .bind(serde_json::to_string(&workflow.edges)?)
        .bind(serde_json::to_string(&workflow.trigger)?)
        .bind(if workflow.is_enabled { 1i64 } else { 0i64 })
        .bind(
            workflow
                .last_run
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
        )
        .bind(workflow.created_at)
        .bind(workflow.updated_at)
        .bind(workflow.pause_state.as_str())
        .bind(workflow.last_paused_at)
        .bind(workflow.last_pause_reason.as_deref())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    fn row_to_workflow(row: &sqlx::sqlite::SqliteRow) -> Result<Workflow> {
        // W50: pause columns are nullable for rows written before the
        // migration; fall back to defaults so legacy rows hydrate as Active.
        let pause_state = row
            .try_get::<Option<String>, _>("pause_state")
            .ok()
            .flatten()
            .map(|raw| crate::models::workflow::SchedulePauseState::parse(&raw))
            .unwrap_or_default();
        let last_paused_at = row
            .try_get::<Option<i64>, _>("last_paused_at")
            .ok()
            .flatten();
        let last_pause_reason = row
            .try_get::<Option<String>, _>("last_pause_reason")
            .ok()
            .flatten();
        Ok(Workflow {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            nodes: serde_json::from_str(row.try_get::<String, _>("nodes")?.as_str())?,
            edges: serde_json::from_str(row.try_get::<String, _>("edges")?.as_str())?,
            trigger: serde_json::from_str(row.try_get::<String, _>("trigger")?.as_str())?,
            is_enabled: row.try_get::<i64, _>("is_enabled")? == 1,
            pause_state,
            last_paused_at,
            last_pause_reason,
            last_run: row
                .try_get::<Option<String>, _>("last_run")?
                .map(|s| serde_json::from_str(&s))
                .transpose()?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }

    // ─── Providers ──────────────────────────────────────────────────────────

    pub async fn list_providers(&self) -> Result<Vec<LLMProvider>> {
        let rows = sqlx::query("SELECT * FROM providers")
            .fetch_all(&self.pool)
            .await?;

        let mut providers = Vec::new();
        for row in rows {
            providers.push(Self::row_to_provider(&row)?);
        }
        Ok(providers)
    }

    pub async fn get_provider(&self, id: &str) -> Result<Option<LLMProvider>> {
        let row = sqlx::query("SELECT * FROM providers WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

        match row {
            Some(r) => Ok(Some(Self::row_to_provider(&r)?)),
            None => Ok(None),
        }
    }

    pub async fn save_provider(&self, provider: &LLMProvider) -> Result<()> {
        sqlx::query(r#"
            INSERT INTO providers (id, name, kind, base_url, api_key, default_model, models, is_enabled)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
            name = excluded.name, kind = excluded.kind, base_url = excluded.base_url,
            api_key = excluded.api_key, default_model = excluded.default_model,
            models = excluded.models, is_enabled = excluded.is_enabled
        "#)
        .bind(&provider.id)
        .bind(&provider.name)
        .bind(serde_json::to_string(&provider.kind)?)
        .bind(&provider.base_url)
        .bind(&provider.api_key)
        .bind(&provider.default_model)
        .bind(serde_json::to_string(&provider.models)?)
        .bind(if provider.is_enabled { 1i64 } else { 0i64 })
        .execute(&self.pool).await?;

        Ok(())
    }

    pub async fn delete_provider(&self, id: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM providers WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    fn row_to_provider(row: &sqlx::sqlite::SqliteRow) -> Result<LLMProvider> {
        let raw_kind: String = row.try_get("kind")?;
        let id: String = row.try_get("id")?;
        let name: String = row.try_get("name")?;
        let stored_enabled = row.try_get::<i64, _>("is_enabled")? == 1;
        let (kind, is_unsupported, is_enabled) =
            match crate::models::provider::LegacyProviderKind::parse(&raw_kind) {
                crate::models::provider::LegacyProviderKind::Supported(kind) => {
                    (kind, false, stored_enabled)
                }
                crate::models::provider::LegacyProviderKind::Unsupported(legacy_kind) => {
                    // W29: legacy `local_mock` (or any future unknown
                    // kind) is migrated to a force-disabled, marked
                    // `is_unsupported` row so the UI can surface a
                    // typed "this provider is no longer supported"
                    // banner without silently flipping it back on or
                    // letting chat select it.
                    tracing::warn!(
                        "provider {} has unsupported legacy kind '{}'; force-disabling and marking unsupported",
                        id,
                        legacy_kind
                    );
                    (crate::models::provider::ProviderKind::Custom, true, false)
                }
            };
        Ok(LLMProvider {
            id,
            name,
            kind,
            base_url: row.try_get("base_url")?,
            api_key: row.try_get("api_key")?,
            default_model: row.try_get("default_model")?,
            models: serde_json::from_str(row.try_get::<String, _>("models")?.as_str())?,
            is_enabled,
            is_unsupported,
        })
    }

    // ─── MCP Servers ────────────────────────────────────────────────────────

    pub async fn list_mcp_servers(&self) -> Result<Vec<MCPServer>> {
        let rows = sqlx::query("SELECT * FROM mcp_servers")
            .fetch_all(&self.pool)
            .await?;

        let mut servers = Vec::new();
        for row in rows {
            servers.push(Self::row_to_mcp_server(&row)?);
        }
        Ok(servers)
    }

    pub async fn get_mcp_server(&self, id: &str) -> Result<Option<MCPServer>> {
        let row = sqlx::query("SELECT * FROM mcp_servers WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

        match row {
            Some(r) => Ok(Some(Self::row_to_mcp_server(&r)?)),
            None => Ok(None),
        }
    }

    pub async fn save_mcp_server(&self, server: &MCPServer) -> Result<()> {
        let args_json = server
            .args
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let env_json = server.env.as_ref().map(serde_json::to_string).transpose()?;

        sqlx::query(r#"
            INSERT INTO mcp_servers (id, name, transport, command, args, env, url, is_enabled)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
            name = excluded.name, transport = excluded.transport, command = excluded.command,
            args = excluded.args, env = excluded.env, url = excluded.url, is_enabled = excluded.is_enabled
        "#)
        .bind(&server.id)
        .bind(&server.name)
        .bind(serde_json::to_string(&server.transport)?)
        .bind(&server.command)
        .bind(args_json)
        .bind(env_json)
        .bind(&server.url)
        .bind(if server.is_enabled { 1i64 } else { 0i64 })
        .execute(&self.pool).await?;

        Ok(())
    }

    pub async fn delete_mcp_server(&self, id: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM mcp_servers WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    fn row_to_mcp_server(row: &sqlx::sqlite::SqliteRow) -> Result<MCPServer> {
        let args_json: Option<String> = row.try_get("args")?;
        let env_json: Option<String> = row.try_get("env")?;

        Ok(MCPServer {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            transport: serde_json::from_str(row.try_get::<String, _>("transport")?.as_str())?,
            is_enabled: row.try_get::<i64, _>("is_enabled")? == 1,
            command: row.try_get("command")?,
            args: args_json.and_then(|s| serde_json::from_str(&s).ok()),
            env: env_json.and_then(|s| serde_json::from_str(&s).ok()),
            url: row.try_get("url")?,
        })
    }

    // ─── Playground Presets (W20) ───────────────────────────────────────────

    pub async fn upsert_playground_preset(&self, preset: &PlaygroundPreset) -> Result<()> {
        let arguments_json = serde_json::to_string(&preset.arguments)?;
        let kind = match preset.tool_kind {
            PlaygroundToolKind::Mcp => "mcp",
            PlaygroundToolKind::Http => "http",
        };
        sqlx::query(
            r#"
                INSERT INTO playground_presets (
                    id, tool_kind, server_id, tool_name, display_name,
                    arguments, created_at, updated_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(id) DO UPDATE SET
                    tool_kind = excluded.tool_kind,
                    server_id = excluded.server_id,
                    tool_name = excluded.tool_name,
                    display_name = excluded.display_name,
                    arguments = excluded.arguments,
                    updated_at = excluded.updated_at
            "#,
        )
        .bind(&preset.id)
        .bind(kind)
        .bind(preset.server_id.as_deref())
        .bind(&preset.tool_name)
        .bind(&preset.display_name)
        .bind(&arguments_json)
        .bind(preset.created_at)
        .bind(preset.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_playground_presets(&self) -> Result<Vec<PlaygroundPreset>> {
        let rows = sqlx::query(
            "SELECT id, tool_kind, server_id, tool_name, display_name, \
             arguments, created_at, updated_at \
             FROM playground_presets ORDER BY updated_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(Self::row_to_playground_preset).collect()
    }

    pub async fn delete_playground_preset(&self, id: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM playground_presets WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    fn row_to_playground_preset(row: &sqlx::sqlite::SqliteRow) -> Result<PlaygroundPreset> {
        let tool_kind: String = row.try_get("tool_kind")?;
        let kind = match tool_kind.as_str() {
            "http" => PlaygroundToolKind::Http,
            _ => PlaygroundToolKind::Mcp,
        };
        let arguments_json: String = row.try_get("arguments")?;
        let arguments: serde_json::Value =
            serde_json::from_str(&arguments_json).unwrap_or(serde_json::Value::Null);
        Ok(PlaygroundPreset {
            id: row.try_get("id")?,
            tool_kind: kind,
            server_id: row.try_get("server_id")?,
            tool_name: row.try_get("tool_name")?,
            display_name: row.try_get("display_name")?,
            arguments,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }

    // ─── W21: Widget alerts ─────────────────────────────────────────────────

    pub async fn get_widget_alerts(&self, widget_id: &str) -> Result<Vec<WidgetAlert>> {
        let row = sqlx::query("SELECT alerts_json FROM widget_alerts WHERE widget_id = ?")
            .bind(widget_id)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(r) => {
                let json: String = r.try_get("alerts_json")?;
                Ok(serde_json::from_str(&json).unwrap_or_default())
            }
            None => Ok(Vec::new()),
        }
    }

    pub async fn set_widget_alerts(
        &self,
        widget_id: &str,
        dashboard_id: &str,
        alerts: &[WidgetAlert],
    ) -> Result<()> {
        let alerts_json = serde_json::to_string(alerts)?;
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            r#"
                INSERT INTO widget_alerts (widget_id, dashboard_id, alerts_json, updated_at)
                VALUES (?, ?, ?, ?)
                ON CONFLICT(widget_id) DO UPDATE SET
                    dashboard_id = excluded.dashboard_id,
                    alerts_json = excluded.alerts_json,
                    updated_at = excluded.updated_at
            "#,
        )
        .bind(widget_id)
        .bind(dashboard_id)
        .bind(&alerts_json)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_widget_alerts_for_dashboard(
        &self,
        dashboard_id: &str,
    ) -> Result<Vec<(String, Vec<WidgetAlert>)>> {
        let rows =
            sqlx::query("SELECT widget_id, alerts_json FROM widget_alerts WHERE dashboard_id = ?")
                .bind(dashboard_id)
                .fetch_all(&self.pool)
                .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let widget_id: String = row.try_get("widget_id")?;
            let json: String = row.try_get("alerts_json")?;
            let alerts: Vec<WidgetAlert> = serde_json::from_str(&json).unwrap_or_default();
            out.push((widget_id, alerts));
        }
        Ok(out)
    }

    // ─── W21: Alert events ──────────────────────────────────────────────────

    pub async fn insert_alert_event(&self, event: &AlertEvent) -> Result<()> {
        sqlx::query(
            r#"
                INSERT INTO alert_events (
                    id, widget_id, dashboard_id, alert_id, fired_at,
                    severity, message, context_json, acknowledged_at,
                    triggered_session_id, workflow_run_id
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&event.id)
        .bind(&event.widget_id)
        .bind(&event.dashboard_id)
        .bind(&event.alert_id)
        .bind(event.fired_at)
        .bind(event.severity.as_str())
        .bind(&event.message)
        .bind(serde_json::to_string(&event.context)?)
        .bind(event.acknowledged_at)
        .bind(event.triggered_session_id.as_deref())
        .bind(event.workflow_run_id.as_deref())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_alert_events(
        &self,
        only_unacknowledged: bool,
        limit: usize,
    ) -> Result<Vec<AlertEvent>> {
        let sql = if only_unacknowledged {
            "SELECT id, widget_id, dashboard_id, alert_id, fired_at, severity, \
             message, context_json, acknowledged_at, triggered_session_id, \
             workflow_run_id \
             FROM alert_events WHERE acknowledged_at IS NULL \
             ORDER BY fired_at DESC LIMIT ?"
        } else {
            "SELECT id, widget_id, dashboard_id, alert_id, fired_at, severity, \
             message, context_json, acknowledged_at, triggered_session_id, \
             workflow_run_id \
             FROM alert_events ORDER BY fired_at DESC LIMIT ?"
        };
        let rows = sqlx::query(sql)
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(Self::row_to_alert_event).collect()
    }

    pub async fn last_fired_at_for_widget(
        &self,
        widget_id: &str,
    ) -> Result<std::collections::HashMap<String, i64>> {
        let rows = sqlx::query(
            "SELECT alert_id, MAX(fired_at) AS last_fired \
             FROM alert_events WHERE widget_id = ? GROUP BY alert_id",
        )
        .bind(widget_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = std::collections::HashMap::new();
        for row in rows {
            let alert_id: String = row.try_get("alert_id")?;
            let last_fired: i64 = row.try_get("last_fired")?;
            out.insert(alert_id, last_fired);
        }
        Ok(out)
    }

    pub async fn count_agent_actions_in_window(
        &self,
        alert_id: &str,
        since_ms: i64,
    ) -> Result<i64> {
        let row = sqlx::query(
            "SELECT COUNT(*) AS n FROM alert_events \
             WHERE alert_id = ? AND triggered_session_id IS NOT NULL AND fired_at >= ?",
        )
        .bind(alert_id)
        .bind(since_ms)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get::<i64, _>("n")?)
    }

    pub async fn acknowledge_alert_event(&self, event_id: &str) -> Result<bool> {
        let now = chrono::Utc::now().timestamp_millis();
        let result = sqlx::query(
            "UPDATE alert_events SET acknowledged_at = ? \
             WHERE id = ? AND acknowledged_at IS NULL",
        )
        .bind(now)
        .bind(event_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn count_unacknowledged_alerts(&self) -> Result<i64> {
        let row =
            sqlx::query("SELECT COUNT(*) AS n FROM alert_events WHERE acknowledged_at IS NULL")
                .fetch_one(&self.pool)
                .await?;
        Ok(row.try_get::<i64, _>("n")?)
    }

    // ─── W23: Pipeline traces ───────────────────────────────────────────────

    pub async fn insert_widget_trace(
        &self,
        widget_id: &str,
        captured_at: i64,
        trace_json: &str,
    ) -> Result<()> {
        const RING_BUFFER_LIMIT: i64 = 5;
        sqlx::query(
            "INSERT OR REPLACE INTO widget_traces (widget_id, captured_at, trace_json) \
             VALUES (?, ?, ?)",
        )
        .bind(widget_id)
        .bind(captured_at)
        .bind(trace_json)
        .execute(&self.pool)
        .await?;

        // Trim oldest entries beyond the ring-buffer cap.
        sqlx::query(
            "DELETE FROM widget_traces WHERE widget_id = ? AND captured_at NOT IN \
             (SELECT captured_at FROM widget_traces WHERE widget_id = ? \
              ORDER BY captured_at DESC LIMIT ?)",
        )
        .bind(widget_id)
        .bind(widget_id)
        .bind(RING_BUFFER_LIMIT)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_widget_traces(&self, widget_id: &str) -> Result<Vec<(i64, String)>> {
        let rows = sqlx::query(
            "SELECT captured_at, trace_json FROM widget_traces \
             WHERE widget_id = ? ORDER BY captured_at DESC",
        )
        .bind(widget_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let captured_at: i64 = row.try_get("captured_at")?;
            let trace_json: String = row.try_get("trace_json")?;
            out.push((captured_at, trace_json));
        }
        Ok(out)
    }

    pub async fn get_widget_trace(
        &self,
        widget_id: &str,
        captured_at: i64,
    ) -> Result<Option<String>> {
        let row = sqlx::query(
            "SELECT trace_json FROM widget_traces \
             WHERE widget_id = ? AND captured_at = ?",
        )
        .bind(widget_id)
        .bind(captured_at)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => Ok(Some(r.try_get::<String, _>("trace_json")?)),
            None => Ok(None),
        }
    }

    pub async fn latest_widget_trace(&self, widget_id: &str) -> Result<Option<(i64, String)>> {
        let row = sqlx::query(
            "SELECT captured_at, trace_json FROM widget_traces \
             WHERE widget_id = ? ORDER BY captured_at DESC LIMIT 1",
        )
        .bind(widget_id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => Ok(Some((
                r.try_get::<i64, _>("captured_at")?,
                r.try_get::<String, _>("trace_json")?,
            ))),
            None => Ok(None),
        }
    }

    fn row_to_alert_event(row: &sqlx::sqlite::SqliteRow) -> Result<AlertEvent> {
        let severity_str: String = row.try_get("severity")?;
        let severity = AlertSeverity::from_str(&severity_str)
            .ok_or_else(|| anyhow::anyhow!("unknown alert severity '{}'", severity_str))?;
        let context_json: String = row.try_get("context_json")?;
        let context: serde_json::Value =
            serde_json::from_str(&context_json).unwrap_or(serde_json::Value::Null);
        Ok(AlertEvent {
            id: row.try_get("id")?,
            widget_id: row.try_get("widget_id")?,
            dashboard_id: row.try_get("dashboard_id")?,
            alert_id: row.try_get("alert_id")?,
            fired_at: row.try_get("fired_at")?,
            severity,
            message: row.try_get("message")?,
            context,
            acknowledged_at: row.try_get("acknowledged_at")?,
            triggered_session_id: row.try_get("triggered_session_id")?,
            workflow_run_id: row.try_get("workflow_run_id")?,
        })
    }

    // ─── W30: Datasource definitions ────────────────────────────────────────

    fn row_to_datasource_definition(row: &sqlx::sqlite::SqliteRow) -> Result<DatasourceDefinition> {
        let definition_json: String = row.try_get("definition_json")?;
        let mut definition: DatasourceDefinition = serde_json::from_str(&definition_json)?;
        // Defensive: trust the row id / workflow id over the JSON copy in
        // case a manual export was re-imported with stale ids.
        definition.id = row.try_get("id")?;
        definition.workflow_id = row.try_get("workflow_id")?;
        definition.created_at = row.try_get("created_at")?;
        definition.updated_at = row.try_get("updated_at")?;
        Ok(definition)
    }

    pub async fn insert_datasource_definition(&self, def: &DatasourceDefinition) -> Result<()> {
        sqlx::query(
            r#"
                INSERT INTO datasource_definitions (
                    id, name, kind, server_id, tool_name, workflow_id,
                    definition_json, created_at, updated_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&def.id)
        .bind(&def.name)
        .bind(serde_json::to_value(&def.kind)?.as_str().unwrap_or(""))
        .bind(def.server_id.as_deref())
        .bind(def.tool_name.as_deref())
        .bind(&def.workflow_id)
        .bind(serde_json::to_string(def)?)
        .bind(def.created_at)
        .bind(def.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_datasource_definition(&self, def: &DatasourceDefinition) -> Result<()> {
        sqlx::query(
            r#"
                UPDATE datasource_definitions
                SET name = ?, kind = ?, server_id = ?, tool_name = ?,
                    workflow_id = ?, definition_json = ?, updated_at = ?
                WHERE id = ?
            "#,
        )
        .bind(&def.name)
        .bind(serde_json::to_value(&def.kind)?.as_str().unwrap_or(""))
        .bind(def.server_id.as_deref())
        .bind(def.tool_name.as_deref())
        .bind(&def.workflow_id)
        .bind(serde_json::to_string(def)?)
        .bind(def.updated_at)
        .bind(&def.id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_datasource_definition(&self, id: &str) -> Result<bool> {
        // Health row is deleted alongside the definition; the backing
        // workflow row is dropped by the caller via the standard
        // `delete_workflow` path so the scheduler can also unhook the cron.
        sqlx::query("DELETE FROM datasource_health WHERE datasource_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        let result = sqlx::query("DELETE FROM datasource_definitions WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn get_datasource_definition(
        &self,
        id: &str,
    ) -> Result<Option<DatasourceDefinition>> {
        let row = sqlx::query(
            "SELECT id, workflow_id, definition_json, created_at, updated_at \
             FROM datasource_definitions WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else { return Ok(None) };
        let mut def = Self::row_to_datasource_definition(&row)?;
        def.health = self.get_datasource_health(id).await?;
        Ok(Some(def))
    }

    pub async fn list_datasource_definitions(&self) -> Result<Vec<DatasourceDefinition>> {
        let rows = sqlx::query(
            "SELECT id, workflow_id, definition_json, created_at, updated_at \
             FROM datasource_definitions ORDER BY updated_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut defs: Vec<DatasourceDefinition> = rows
            .iter()
            .map(Self::row_to_datasource_definition)
            .collect::<Result<_>>()?;
        for def in defs.iter_mut() {
            def.health = self.get_datasource_health(&def.id).await?;
        }
        Ok(defs)
    }

    pub async fn get_datasource_by_workflow_id(
        &self,
        workflow_id: &str,
    ) -> Result<Option<DatasourceDefinition>> {
        let row = sqlx::query(
            "SELECT id, workflow_id, definition_json, created_at, updated_at \
             FROM datasource_definitions WHERE workflow_id = ?",
        )
        .bind(workflow_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else { return Ok(None) };
        let mut def = Self::row_to_datasource_definition(&row)?;
        def.health = self.get_datasource_health(&def.id).await?;
        Ok(Some(def))
    }

    pub async fn get_datasource_health(&self, id: &str) -> Result<Option<DatasourceHealth>> {
        let row = sqlx::query("SELECT health_json FROM datasource_health WHERE datasource_id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(row) => {
                let health_json: String = row.try_get("health_json")?;
                Ok(serde_json::from_str(&health_json).ok())
            }
            None => Ok(None),
        }
    }

    pub async fn upsert_datasource_health(
        &self,
        datasource_id: &str,
        health: &DatasourceHealth,
    ) -> Result<()> {
        sqlx::query(
            r#"
                INSERT INTO datasource_health (datasource_id, health_json, updated_at)
                VALUES (?, ?, ?)
                ON CONFLICT(datasource_id) DO UPDATE SET
                    health_json = excluded.health_json,
                    updated_at = excluded.updated_at
            "#,
        )
        .bind(datasource_id)
        .bind(serde_json::to_string(health)?)
        .bind(health.last_run_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::dashboard::Dashboard;
    use crate::models::mcp::{MCPServer, MCPTransport};
    use crate::models::memory::{MemoryKind, MemoryRecord, Scope, ToolShape};
    use crate::models::provider::{LLMProvider, ProviderKind};
    use crate::models::workflow::{RunStatus, TriggerKind, Workflow, WorkflowRun, WorkflowTrigger};
    use std::collections::HashMap;

    #[tokio::test]
    async fn external_source_state_credential_round_trip() -> Result<()> {
        let storage = Storage::new_for_tests().await?;
        let now = chrono::Utc::now().timestamp_millis();

        // Default-empty case.
        assert!(storage
            .get_external_source_state("brave_search_web")
            .await?
            .is_none());

        // Enable with no credential.
        storage
            .upsert_external_source_state("brave_search_web", true, None, now)
            .await?;
        let (enabled, credential, _) = storage
            .get_external_source_state("brave_search_web")
            .await?
            .unwrap();
        assert!(enabled);
        assert!(credential.is_none());

        // Set a credential and verify enablement is preserved.
        storage
            .upsert_external_source_state("brave_search_web", true, Some("k1"), now + 1)
            .await?;
        let (enabled, credential, _) = storage
            .get_external_source_state("brave_search_web")
            .await?
            .unwrap();
        assert!(enabled);
        assert_eq!(credential.as_deref(), Some("k1"));

        // Calling upsert with credential=None must NOT wipe an existing
        // credential — that flow is for set_enabled. Use the dedicated
        // clear method to remove credentials.
        storage
            .upsert_external_source_state("brave_search_web", false, None, now + 2)
            .await?;
        let (enabled, credential, _) = storage
            .get_external_source_state("brave_search_web")
            .await?
            .unwrap();
        assert!(!enabled);
        assert_eq!(credential.as_deref(), Some("k1"));

        storage
            .clear_external_source_credential("brave_search_web")
            .await?;
        let (_, credential, _) = storage
            .get_external_source_state("brave_search_web")
            .await?
            .unwrap();
        assert!(credential.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn agent_memory_fts_retrieval_round_trip() -> Result<()> {
        let storage = Storage::new_for_tests().await?;
        let now = chrono::Utc::now().timestamp_millis();
        let record = MemoryRecord {
            id: "mem-1".into(),
            scope: Scope::Dashboard("dash-x".into()),
            kind: MemoryKind::Preference,
            content: "User prefers gauge widgets for percentage metrics.".into(),
            metadata: serde_json::json!({}),
            created_at: now,
            accessed_count: 0,
            last_accessed_at: None,
            expires_at: None,
            compressed_into: None,
        };
        storage.insert_memory(&record).await?;

        let filters = vec![("dashboard".to_string(), Some("dash-x".to_string()))];
        let hits = storage
            .search_memories_fts("percentage gauge", &filters, 5)
            .await?;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0.id, "mem-1");

        // Empty/junk queries shouldn't crash FTS; they return zero.
        let empty = storage.search_memories_fts("???", &filters, 5).await?;
        assert!(empty.is_empty());

        assert!(storage.delete_memory("mem-1").await?);
        Ok(())
    }

    #[tokio::test]
    async fn tool_shape_upsert_increments_observation_count() -> Result<()> {
        let storage = Storage::new_for_tests().await?;
        let now = chrono::Utc::now().timestamp_millis();
        let shape = ToolShape {
            id: "shape-1".into(),
            server_id: "test_server".into(),
            tool_name: "test_tool".into(),
            args_fingerprint: "fp1".into(),
            shape_summary: "object with data.items[*].{id,name}".into(),
            shape_full: "{}".into(),
            sample_path: Some("data.items".into()),
            observed_at: now,
            observation_count: 1,
        };
        storage.upsert_tool_shape(&shape).await?;
        storage.upsert_tool_shape(&shape).await?;
        let stored = storage
            .lookup_tool_shape("test_server", "test_tool", "fp1")
            .await?
            .expect("shape stored");
        assert_eq!(stored.observation_count, 2);
        Ok(())
    }

    #[tokio::test]
    async fn dashboard_config_provider_mcp_and_workflow_persistence_smoke() -> Result<()> {
        let storage = Storage::new_for_tests().await?;
        let now = chrono::Utc::now().timestamp_millis();

        let dashboard = Dashboard {
            id: "dash-1".into(),
            name: "Ops".into(),
            description: Some("Local dashboard".into()),
            layout: vec![],
            workflows: vec![],
            is_default: false,
            created_at: now,
            updated_at: now,
            parameters: Vec::new(),
            model_policy: None,
            language_policy: None,
        };
        storage.create_dashboard(&dashboard).await?;
        assert_eq!(storage.get_dashboard("dash-1").await?.unwrap().name, "Ops");
        assert!(storage.delete_dashboard("dash-1").await.is_ok());
        assert!(storage.get_dashboard("dash-1").await?.is_none());

        storage.set_config("theme", "dark").await?;
        assert_eq!(storage.get_config("theme").await?, Some("dark".into()));

        let provider = LLMProvider {
            id: "provider-1".into(),
            name: "Local".into(),
            kind: ProviderKind::Custom,
            base_url: "http://localhost:11434".into(),
            api_key: Some("secret".into()),
            default_model: "mock".into(),
            models: vec!["mock".into()],
            is_enabled: true,
            is_unsupported: false,
        };
        storage.save_provider(&provider).await?;
        assert_eq!(
            storage.get_provider("provider-1").await?.unwrap().api_key,
            Some("secret".into())
        );
        assert!(storage.delete_provider("provider-1").await?);

        let mut env = HashMap::new();
        env.insert("TOKEN".into(), "secret".into());
        let server = MCPServer {
            id: "mcp-1".into(),
            name: "Local MCP".into(),
            transport: MCPTransport::Stdio,
            is_enabled: true,
            command: Some("node".into()),
            args: Some(vec!["server.js".into()]),
            env: Some(env),
            url: None,
        };
        storage.save_mcp_server(&server).await?;
        assert!(storage
            .get_mcp_server("mcp-1")
            .await?
            .unwrap()
            .env
            .is_some());
        assert!(storage.delete_mcp_server("mcp-1").await?);

        let workflow = Workflow {
            id: "workflow-1".into(),
            name: "Manual".into(),
            description: None,
            nodes: vec![],
            edges: vec![],
            trigger: WorkflowTrigger {
                kind: TriggerKind::Manual,
                config: None,
            },
            is_enabled: true,
            pause_state: Default::default(),
            last_paused_at: None,
            last_pause_reason: None,
            last_run: None,
            created_at: now,
            updated_at: now,
        };
        storage.create_workflow(&workflow).await?;
        let run = WorkflowRun {
            id: "run-1".into(),
            started_at: now,
            finished_at: Some(now + 1),
            status: RunStatus::Success,
            node_results: Some(serde_json::json!({"ok": true})),
            error: None,
        };
        storage.save_workflow_run("workflow-1", &run).await?;
        storage.update_workflow_last_run("workflow-1", &run).await?;
        assert_eq!(
            storage
                .get_workflow("workflow-1")
                .await?
                .unwrap()
                .last_run
                .unwrap()
                .id,
            "run-1"
        );

        Ok(())
    }

    /// W19: snapshot insert + listing + read-back, plus ring buffer prune
    /// at the 30-entry cap so spamming Apply does not balloon the table.
    #[tokio::test]
    async fn dashboard_version_round_trip_and_ring_buffer_prune() -> Result<()> {
        use crate::models::dashboard::VersionSource;

        let storage = Storage::new_for_tests().await?;
        let now = chrono::Utc::now().timestamp_millis();
        let dashboard = Dashboard {
            id: "dash-v".into(),
            name: "Versions".into(),
            description: None,
            layout: vec![],
            workflows: vec![],
            is_default: false,
            created_at: now,
            updated_at: now,
            parameters: Vec::new(),
            model_policy: None,
            language_policy: None,
        };
        storage.create_dashboard(&dashboard).await?;

        // Insert 35 versions; only the newest 30 should survive.
        for i in 0..35 {
            storage
                .insert_dashboard_version(
                    &format!("ver-{i:03}"),
                    &dashboard,
                    VersionSource::AgentApply,
                    &format!("apply #{i}"),
                    Some("sess-1"),
                    None,
                    now + i as i64,
                )
                .await?;
        }

        let listing = storage.list_dashboard_versions("dash-v").await?;
        assert_eq!(listing.len(), 30);
        assert_eq!(listing[0].id, "ver-034");
        assert_eq!(listing[29].id, "ver-005");

        let full = storage
            .get_dashboard_version("ver-020")
            .await?
            .expect("version stored");
        assert_eq!(full.snapshot.name, "Versions");
        assert_eq!(full.source, VersionSource::AgentApply);
        assert_eq!(full.source_session_id.as_deref(), Some("sess-1"));

        Ok(())
    }

    /// W30: datasource definition CRUD + health upsert. Verifies that the
    /// list endpoint reflects the latest health snapshot without rewriting
    /// the definition row.
    #[tokio::test]
    async fn datasource_definition_crud_and_health_round_trip() -> Result<()> {
        use crate::models::dashboard::BuildDatasourcePlanKind;
        use crate::models::datasource::{
            DatasourceDefinition, DatasourceHealth, DatasourceHealthStatus,
        };
        use crate::models::pipeline::PipelineStep;

        let storage = Storage::new_for_tests().await?;
        let now = chrono::Utc::now().timestamp_millis();
        let def = DatasourceDefinition {
            id: "ds-1".into(),
            name: "issues".into(),
            description: Some("Open GitHub issues".into()),
            kind: BuildDatasourcePlanKind::McpTool,
            tool_name: Some("list_issues".into()),
            server_id: Some("gh-mcp".into()),
            arguments: Some(serde_json::json!({"repo": "foo/bar"})),
            prompt: None,
            pipeline: vec![PipelineStep::Limit { count: 5 }],
            refresh_cron: Some("0 */5 * * * *".into()),
            workflow_id: "wf-ds-1".into(),
            created_at: now,
            updated_at: now,
            health: None,
            originated_external_source_id: None,
        };
        storage.insert_datasource_definition(&def).await?;
        let fetched = storage
            .get_datasource_definition("ds-1")
            .await?
            .expect("definition stored");
        assert_eq!(fetched.name, "issues");
        assert_eq!(fetched.pipeline.len(), 1);
        assert!(fetched.health.is_none());

        // List includes the row even before health exists.
        let listing = storage.list_datasource_definitions().await?;
        assert_eq!(listing.len(), 1);

        // Upsert health, then re-read; the health snapshot must round-trip.
        let health = DatasourceHealth {
            last_run_at: now + 1_000,
            last_status: DatasourceHealthStatus::Ok,
            last_error: None,
            last_duration_ms: 42,
            sample_preview: Some(serde_json::json!({"count": 3})),
            consumer_count: 2,
        };
        storage.upsert_datasource_health("ds-1", &health).await?;
        let with_health = storage
            .get_datasource_definition("ds-1")
            .await?
            .expect("definition stored");
        let stored_health = with_health.health.clone().expect("health snapshot");
        assert_eq!(stored_health.last_duration_ms, 42);
        assert_eq!(stored_health.consumer_count, 2);

        // Update (replace) re-writes the definition without dropping health.
        let mut updated = with_health.clone();
        updated.health = None;
        updated.name = "issues_renamed".into();
        updated.updated_at = now + 2_000;
        storage.update_datasource_definition(&updated).await?;
        let after_update = storage
            .get_datasource_definition("ds-1")
            .await?
            .expect("definition stored");
        assert_eq!(after_update.name, "issues_renamed");
        assert!(
            after_update.health.is_some(),
            "health should survive a definition update"
        );

        // Lookup by workflow id is used by the consumer-scan command.
        let by_workflow = storage
            .get_datasource_by_workflow_id("wf-ds-1")
            .await?
            .expect("workflow lookup");
        assert_eq!(by_workflow.id, "ds-1");

        // Delete removes both the definition row and the health row.
        assert!(storage.delete_datasource_definition("ds-1").await?);
        assert!(storage.get_datasource_definition("ds-1").await?.is_none());
        assert!(storage.get_datasource_health("ds-1").await?.is_none());
        Ok(())
    }

    /// W31: a [`DatasourceConfig`] that carries an explicit
    /// `datasource_definition_id` plus provenance must round-trip
    /// through the dashboard JSON column without losing fields. Legacy
    /// rows (no W31 fields) must still deserialize.
    #[tokio::test]
    async fn datasource_config_w31_binding_round_trip() -> Result<()> {
        use crate::models::dashboard::Dashboard;
        use crate::models::widget::{
            ChartConfig, ChartKind, DatasourceBindingSource, DatasourceConfig, Widget,
        };

        let storage = Storage::new_for_tests().await?;
        let now = chrono::Utc::now().timestamp_millis();
        let widget = Widget::Chart {
            id: "w1".into(),
            title: "Issues".into(),
            x: 0,
            y: 0,
            w: 6,
            h: 4,
            config: ChartConfig {
                kind: ChartKind::Bar,
                x_axis: None,
                y_axis: None,
                colors: None,
                stacked: false,
                show_legend: true,
            },
            datasource: Some(DatasourceConfig {
                workflow_id: "wf-1".into(),
                output_key: "output.data".into(),
                post_process: None,
                capture_traces: false,
                datasource_definition_id: Some("ds-explicit".into()),
                binding_source: Some(DatasourceBindingSource::BuildChat),
                bound_at: Some(now),
                tail_pipeline: vec![],
                model_override: None,
            }),
            refresh_interval: None,
        };
        let dashboard = Dashboard {
            id: "d1".into(),
            name: "d".into(),
            description: None,
            layout: vec![widget],
            workflows: vec![],
            is_default: false,
            created_at: now,
            updated_at: now,
            parameters: vec![],
            model_policy: None,
            language_policy: None,
        };
        storage.create_dashboard(&dashboard).await?;
        let fetched = storage
            .get_dashboard("d1")
            .await?
            .expect("dashboard stored");
        let Widget::Chart { datasource, .. } = &fetched.layout[0] else {
            panic!("expected chart");
        };
        let cfg = datasource.as_ref().expect("datasource present");
        assert_eq!(cfg.datasource_definition_id.as_deref(), Some("ds-explicit"));
        assert_eq!(cfg.binding_source, Some(DatasourceBindingSource::BuildChat));
        assert_eq!(cfg.bound_at, Some(now));

        // Legacy DatasourceConfig (W30 shape, no W31 fields) must still
        // deserialize cleanly into the extended struct.
        let legacy_json = serde_json::json!({
            "workflow_id": "wf-legacy",
            "output_key": "output.data",
        });
        let legacy: DatasourceConfig = serde_json::from_value(legacy_json)?;
        assert!(legacy.datasource_definition_id.is_none());
        assert!(legacy.binding_source.is_none());
        assert!(legacy.bound_at.is_none());
        Ok(())
    }

    /// W31: signature comparison must reject mismatched arguments and
    /// accept identical kind+tool+args+pipeline+prompt. Drives
    /// Build-apply reuse so that fresh proposals targeting an existing
    /// catalog entry bind to it instead of minting a duplicate.
    #[test]
    fn shared_matches_definition_signature() {
        use crate::commands::dashboard::shared_matches_definition;
        use crate::models::dashboard::{BuildDatasourcePlanKind, SharedDatasource};
        use crate::models::datasource::DatasourceDefinition;
        use crate::models::pipeline::PipelineStep;

        let shared = SharedDatasource {
            key: "issues".into(),
            kind: BuildDatasourcePlanKind::McpTool,
            tool_name: Some("list_issues".into()),
            server_id: Some("gh".into()),
            arguments: Some(serde_json::json!({"repo": "foo/bar"})),
            prompt: None,
            pipeline: vec![PipelineStep::Limit { count: 5 }],
            refresh_cron: Some("0 */5 * * * *".into()),
            label: Some("issues feed".into()),
        };
        let def = DatasourceDefinition {
            id: "ds".into(),
            name: "open_issues".into(),
            description: None,
            kind: BuildDatasourcePlanKind::McpTool,
            tool_name: Some("list_issues".into()),
            server_id: Some("gh".into()),
            arguments: Some(serde_json::json!({"repo": "foo/bar"})),
            prompt: None,
            pipeline: vec![PipelineStep::Limit { count: 5 }],
            refresh_cron: None,
            workflow_id: "wf".into(),
            created_at: 0,
            updated_at: 0,
            health: None,
            originated_external_source_id: None,
        };
        assert!(shared_matches_definition(&shared, &def));

        let mut other = shared.clone();
        other.arguments = Some(serde_json::json!({"repo": "different"}));
        assert!(
            !shared_matches_definition(&other, &def),
            "different arguments should not match"
        );

        let mut other = shared.clone();
        other.pipeline = vec![PipelineStep::Limit { count: 10 }];
        assert!(
            !shared_matches_definition(&other, &def),
            "different pipeline should not match"
        );
    }

    /// W31.1: tail_pipeline must round-trip in DatasourceConfig — empty
    /// vec (default) AND non-empty steps both deserialize and survive
    /// dashboard JSON storage.
    #[tokio::test]
    async fn datasource_config_w31_tail_pipeline_round_trip() -> Result<()> {
        use crate::models::dashboard::Dashboard;
        use crate::models::pipeline::PipelineStep;
        use crate::models::widget::{
            ChartConfig, ChartKind, DatasourceBindingSource, DatasourceConfig, Widget,
        };

        let storage = Storage::new_for_tests().await?;
        let now = chrono::Utc::now().timestamp_millis();
        let widget = Widget::Chart {
            id: "w1".into(),
            title: "Issues".into(),
            x: 0,
            y: 0,
            w: 6,
            h: 4,
            config: ChartConfig {
                kind: ChartKind::Bar,
                x_axis: None,
                y_axis: None,
                colors: None,
                stacked: false,
                show_legend: true,
            },
            datasource: Some(DatasourceConfig {
                workflow_id: "wf-1".into(),
                output_key: "output.data".into(),
                post_process: None,
                capture_traces: false,
                datasource_definition_id: Some("ds-1".into()),
                binding_source: Some(DatasourceBindingSource::BuildChat),
                bound_at: Some(now),
                tail_pipeline: vec![
                    PipelineStep::Pick {
                        path: "items".into(),
                    },
                    PipelineStep::Limit { count: 5 },
                ],
                model_override: None,
            }),
            refresh_interval: None,
        };
        let dashboard = Dashboard {
            id: "d1".into(),
            name: "d".into(),
            description: None,
            layout: vec![widget],
            workflows: vec![],
            is_default: false,
            created_at: now,
            updated_at: now,
            parameters: vec![],
            model_policy: None,
            language_policy: None,
        };
        storage.create_dashboard(&dashboard).await?;
        let fetched = storage
            .get_dashboard("d1")
            .await?
            .expect("dashboard stored");
        let Widget::Chart { datasource, .. } = &fetched.layout[0] else {
            panic!("expected chart");
        };
        let cfg = datasource.as_ref().expect("datasource present");
        assert_eq!(cfg.tail_pipeline.len(), 2);
        match &cfg.tail_pipeline[0] {
            PipelineStep::Pick { path } => assert_eq!(path, "items"),
            _ => panic!("expected Pick step"),
        }

        // Backward compat: legacy JSON without tail_pipeline still
        // deserializes with an empty tail.
        let legacy_json = serde_json::json!({
            "workflow_id": "wf-legacy",
            "output_key": "output.data",
        });
        let legacy: DatasourceConfig = serde_json::from_value(legacy_json)?;
        assert!(legacy.tail_pipeline.is_empty());
        Ok(())
    }

    /// W35: AlertEvent persists `workflow_run_id` and surfaces it on
    /// read. `None` round-trips as `None`; existing tooling that builds
    /// events with no run id keeps working.
    #[tokio::test]
    async fn alert_event_workflow_run_id_round_trip() -> Result<()> {
        use crate::models::alert::{AlertEvent, AlertSeverity};

        let storage = Storage::new_for_tests().await?;
        let now = chrono::Utc::now().timestamp_millis();
        let with_run = AlertEvent {
            id: "evt-1".into(),
            widget_id: "w-1".into(),
            dashboard_id: "d-1".into(),
            alert_id: "a-1".into(),
            fired_at: now,
            severity: AlertSeverity::Warning,
            message: "tripped".into(),
            context: serde_json::json!({ "value": 42 }),
            acknowledged_at: None,
            triggered_session_id: None,
            workflow_run_id: Some("run-xyz".into()),
        };
        let without_run = AlertEvent {
            id: "evt-2".into(),
            workflow_run_id: None,
            ..with_run.clone()
        };
        storage.insert_alert_event(&with_run).await?;
        storage.insert_alert_event(&without_run).await?;
        let events = storage.list_alert_events(false, 10).await?;
        let by_id: std::collections::HashMap<_, _> =
            events.into_iter().map(|e| (e.id.clone(), e)).collect();
        assert_eq!(
            by_id.get("evt-1").unwrap().workflow_run_id.as_deref(),
            Some("run-xyz")
        );
        assert!(by_id.get("evt-2").unwrap().workflow_run_id.is_none());
        Ok(())
    }

    /// W35: run summaries skip node_results, return rows ordered by
    /// most recent first, and respect the `workflow_id` filter. Detail
    /// lookup returns the originating workflow id alongside the full
    /// run row.
    #[tokio::test]
    async fn workflow_run_summaries_and_detail_round_trip() -> Result<()> {
        let storage = Storage::new_for_tests().await?;
        let now = chrono::Utc::now().timestamp_millis();

        let workflow_a = Workflow {
            id: "wf-a".into(),
            name: "Workflow A".into(),
            description: None,
            nodes: vec![],
            edges: vec![],
            trigger: WorkflowTrigger {
                kind: TriggerKind::Manual,
                config: None,
            },
            is_enabled: true,
            pause_state: Default::default(),
            last_paused_at: None,
            last_pause_reason: None,
            last_run: None,
            created_at: now,
            updated_at: now,
        };
        let workflow_b = Workflow {
            id: "wf-b".into(),
            name: "Workflow B".into(),
            ..workflow_a.clone()
        };
        storage.create_workflow(&workflow_a).await?;
        storage.create_workflow(&workflow_b).await?;

        for (workflow_id, run_id, started, status, error) in [
            ("wf-a", "run-a-1", now, RunStatus::Success, None::<String>),
            (
                "wf-a",
                "run-a-2",
                now + 1_000,
                RunStatus::Error,
                Some("boom".into()),
            ),
            ("wf-b", "run-b-1", now + 2_000, RunStatus::Success, None),
        ] {
            let run = WorkflowRun {
                id: run_id.into(),
                started_at: started,
                finished_at: Some(started + 50),
                status,
                node_results: Some(serde_json::json!({ "k": "v" })),
                error,
            };
            storage.save_workflow_run(workflow_id, &run).await?;
        }

        let summaries_a = storage
            .list_workflow_run_summaries(Some("wf-a"), 10)
            .await?;
        assert_eq!(summaries_a.len(), 2);
        assert_eq!(summaries_a[0].id, "run-a-2");
        assert!(summaries_a[0].duration_ms.unwrap_or_default() >= 0);
        assert!(summaries_a[0].has_node_results);
        assert!(matches!(summaries_a[0].status, RunStatus::Error));
        assert_eq!(summaries_a[0].error.as_deref(), Some("boom"));

        let summaries_all = storage.list_workflow_run_summaries(None, 10).await?;
        assert_eq!(summaries_all.len(), 3);
        assert_eq!(summaries_all[0].id, "run-b-1");

        let (workflow_id, run) = storage
            .get_workflow_run("run-a-1")
            .await?
            .expect("run stored");
        assert_eq!(workflow_id, "wf-a");
        assert!(matches!(run.status, RunStatus::Success));
        assert!(run.node_results.is_some());

        assert!(storage.get_workflow_run("missing").await?.is_none());
        Ok(())
    }

    /// W36: snapshots round-trip through upsert/list, the conflict
    /// resolution updates the existing row in place (no duplicates),
    /// and deleting the owning dashboard wipes the snapshots so the
    /// table stays bounded by the user's actual dashboard set.
    #[tokio::test]
    async fn widget_runtime_snapshot_round_trip_and_dashboard_delete() -> Result<()> {
        let storage = Storage::new_for_tests().await?;
        let now = chrono::Utc::now().timestamp_millis();
        let dashboard = Dashboard {
            id: "dash-snap".into(),
            name: "Snap".into(),
            description: None,
            layout: vec![],
            workflows: vec![],
            is_default: false,
            created_at: now,
            updated_at: now,
            parameters: Vec::new(),
            model_policy: None,
            language_policy: None,
        };
        storage.create_dashboard(&dashboard).await?;

        let snap = WidgetRuntimeSnapshot {
            dashboard_id: "dash-snap".into(),
            widget_id: "w1".into(),
            widget_kind: "gauge".into(),
            runtime_data: serde_json::json!({"kind": "gauge", "value": 42.0}),
            captured_at: now,
            workflow_id: Some("wf-1".into()),
            workflow_run_id: Some("run-1".into()),
            datasource_definition_id: None,
            config_fingerprint: "cfg-fp-original".into(),
            parameter_fingerprint: "param-fp-original".into(),
        };
        storage.upsert_widget_snapshot(&snap).await?;

        let listed = storage.list_widget_snapshots("dash-snap").await?;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].widget_id, "w1");
        assert_eq!(
            listed[0].runtime_data["value"].as_f64(),
            Some(42.0),
            "runtime_data must survive the JSON round-trip"
        );

        // Upserting the same (dashboard, widget) replaces in place —
        // no second row, and the new fingerprint/data win.
        let snap_v2 = WidgetRuntimeSnapshot {
            runtime_data: serde_json::json!({"kind": "gauge", "value": 77.0}),
            captured_at: now + 1_000,
            config_fingerprint: "cfg-fp-v2".into(),
            ..snap.clone()
        };
        storage.upsert_widget_snapshot(&snap_v2).await?;
        let listed = storage.list_widget_snapshots("dash-snap").await?;
        assert_eq!(listed.len(), 1, "upsert must not insert a second row");
        assert_eq!(listed[0].runtime_data["value"].as_f64(), Some(77.0));
        assert_eq!(listed[0].config_fingerprint, "cfg-fp-v2");
        assert_eq!(listed[0].captured_at, now + 1_000);

        storage.delete_widget_snapshot("dash-snap", "w1").await?;
        assert!(storage.list_widget_snapshots("dash-snap").await?.is_empty());

        // Cascade: deleting the dashboard wipes any remaining
        // snapshots so the orphan-free invariant holds even when the
        // user deletes a dashboard with cached widgets.
        storage.upsert_widget_snapshot(&snap).await?;
        assert_eq!(storage.list_widget_snapshots("dash-snap").await?.len(), 1);
        storage.delete_dashboard("dash-snap").await?;
        assert!(storage.list_widget_snapshots("dash-snap").await?.is_empty());
        Ok(())
    }
}

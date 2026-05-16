use anyhow::Result;
use std::str::FromStr;

use sqlx::{sqlite::SqliteConnectOptions, sqlite::SqlitePoolOptions, Pool, Row, Sqlite};
use tauri::{App, Manager};
use tracing::info;

use crate::models::{
    alert::{AlertEvent, AlertSeverity, WidgetAlert},
    chat::ChatSession,
    dashboard::{Dashboard, DashboardVersion, DashboardVersionSummary, VersionSource},
    mcp::MCPServer,
    memory::{MemoryKind, MemoryRecord, Scope, ToolShape},
    playground::{PlaygroundPreset, PlaygroundToolKind},
    provider::LLMProvider,
    workflow::Workflow,
};

pub struct Storage {
    pool: Pool<Sqlite>,
    app_data_dir: std::path::PathBuf,
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
    async fn new_for_tests() -> Result<Self> {
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
        for stmt in [
            "ALTER TABLE chat_sessions ADD COLUMN total_input_tokens INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE chat_sessions ADD COLUMN total_output_tokens INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE chat_sessions ADD COLUMN total_reasoning_tokens INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE chat_sessions ADD COLUMN total_cost_usd REAL NOT NULL DEFAULT 0.0",
            "ALTER TABLE chat_sessions ADD COLUMN max_cost_usd REAL",
        ] {
            let _ = sqlx::query(stmt).execute(&self.pool).await;
        }

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
                    triggered_session_id TEXT
                )
            "#,
        )
        .execute(&self.pool)
        .await?;

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

        info!("✅ Database migrations complete");
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
        sqlx::query(r#"
            INSERT INTO dashboards (id, name, description, layout, workflows, is_default, created_at, updated_at, parameters)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
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
        .execute(&self.pool).await?;

        Ok(())
    }

    pub async fn update_dashboard(&self, dashboard: &Dashboard) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE dashboards SET name = ?, description = ?, layout = ?, workflows = ?,
            is_default = ?, updated_at = ?, parameters = ? WHERE id = ?
        "#,
        )
        .bind(&dashboard.name)
        .bind(&dashboard.description)
        .bind(serde_json::to_string(&dashboard.layout)?)
        .bind(serde_json::to_string(&dashboard.workflows)?)
        .bind(if dashboard.is_default { 1i64 } else { 0i64 })
        .bind(dashboard.updated_at)
        .bind(serde_json::to_string(&dashboard.parameters)?)
        .bind(&dashboard.id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn delete_dashboard(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM dashboards WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
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
                total_cost_usd, max_cost_usd,
                created_at, updated_at
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
                total_cost_usd = ?, max_cost_usd = ?,
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
        .bind(session.updated_at)
        .bind(&session.id)
        .execute(&self.pool)
        .await?;

        Ok(())
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
            INSERT INTO workflows (id, name, description, nodes, edges, trigger, is_enabled, last_run, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
                    created_at, updated_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    fn row_to_workflow(row: &sqlx::sqlite::SqliteRow) -> Result<Workflow> {
        Ok(Workflow {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            nodes: serde_json::from_str(row.try_get::<String, _>("nodes")?.as_str())?,
            edges: serde_json::from_str(row.try_get::<String, _>("edges")?.as_str())?,
            trigger: serde_json::from_str(row.try_get::<String, _>("trigger")?.as_str())?,
            is_enabled: row.try_get::<i64, _>("is_enabled")? == 1,
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
        Ok(LLMProvider {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            kind: serde_json::from_str(row.try_get::<String, _>("kind")?.as_str())?,
            base_url: row.try_get("base_url")?,
            api_key: row.try_get("api_key")?,
            default_model: row.try_get("default_model")?,
            models: serde_json::from_str(row.try_get::<String, _>("models")?.as_str())?,
            is_enabled: row.try_get::<i64, _>("is_enabled")? == 1,
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
                    triggered_session_id
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
             message, context_json, acknowledged_at, triggered_session_id \
             FROM alert_events WHERE acknowledged_at IS NULL \
             ORDER BY fired_at DESC LIMIT ?"
        } else {
            "SELECT id, widget_id, dashboard_id, alert_id, fired_at, severity, \
             message, context_json, acknowledged_at, triggered_session_id \
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
        })
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
            server_id: "yandex".into(),
            tool_name: "get_releases".into(),
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
            .lookup_tool_shape("yandex", "get_releases", "fp1")
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
}

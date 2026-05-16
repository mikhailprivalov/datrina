use anyhow::Result;
use std::str::FromStr;

use sqlx::{sqlite::SqliteConnectOptions, sqlite::SqlitePoolOptions, Pool, Row, Sqlite};
use tauri::{App, Manager};
use tracing::info;

use crate::models::{
    chat::ChatSession,
    dashboard::Dashboard,
    mcp::MCPServer,
    memory::{MemoryKind, MemoryRecord, Scope, ToolShape},
    provider::LLMProvider,
    workflow::Workflow,
};

pub struct Storage {
    pool: Pool<Sqlite>,
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

        let storage = Self { pool };
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

        Ok(Self { pool })
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
            INSERT INTO dashboards (id, name, description, layout, workflows, is_default, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#)
        .bind(&dashboard.id)
        .bind(&dashboard.name)
        .bind(&dashboard.description)
        .bind(serde_json::to_string(&dashboard.layout)?)
        .bind(serde_json::to_string(&dashboard.workflows)?)
        .bind(if dashboard.is_default { 1i64 } else { 0i64 })
        .bind(dashboard.created_at)
        .bind(dashboard.updated_at)
        .execute(&self.pool).await?;

        Ok(())
    }

    pub async fn update_dashboard(&self, dashboard: &Dashboard) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE dashboards SET name = ?, description = ?, layout = ?, workflows = ?,
            is_default = ?, updated_at = ? WHERE id = ?
        "#,
        )
        .bind(&dashboard.name)
        .bind(&dashboard.description)
        .bind(serde_json::to_string(&dashboard.layout)?)
        .bind(serde_json::to_string(&dashboard.workflows)?)
        .bind(if dashboard.is_default { 1i64 } else { 0i64 })
        .bind(dashboard.updated_at)
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

    fn row_to_dashboard(row: &sqlx::sqlite::SqliteRow) -> Result<Dashboard> {
        let layout_json: String = row.try_get("layout")?;
        let workflows_json: String = row.try_get("workflows")?;

        Ok(Dashboard {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            layout: serde_json::from_str(&layout_json)?,
            workflows: serde_json::from_str(&workflows_json)?,
            is_default: row.try_get::<i64, _>("is_default")? == 1,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
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
        sqlx::query(r#"
            INSERT INTO chat_sessions (id, mode, dashboard_id, widget_id, title, messages, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#)
        .bind(&session.id)
        .bind(match session.mode {
            crate::models::chat::ChatMode::Build => "build",
            crate::models::chat::ChatMode::Context => "context",
        })
        .bind(&session.dashboard_id)
        .bind(&session.widget_id)
        .bind(&session.title)
        .bind(serde_json::to_string(&session.messages)?)
        .bind(session.created_at)
        .bind(session.updated_at)
        .execute(&self.pool).await?;

        Ok(())
    }

    pub async fn update_chat_session(&self, session: &ChatSession) -> Result<()> {
        sqlx::query(r#"
            UPDATE chat_sessions SET mode = ?, dashboard_id = ?, widget_id = ?, title = ?, messages = ?, updated_at = ?
            WHERE id = ?
        "#)
        .bind(match session.mode {
            crate::models::chat::ChatMode::Build => "build",
            crate::models::chat::ChatMode::Context => "context",
        })
        .bind(&session.dashboard_id)
        .bind(&session.widget_id)
        .bind(&session.title)
        .bind(serde_json::to_string(&session.messages)?)
        .bind(session.updated_at)
        .bind(&session.id)
        .execute(&self.pool).await?;

        Ok(())
    }

    pub async fn delete_chat_session(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM chat_sessions WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    fn row_to_chat_session(row: &sqlx::sqlite::SqliteRow) -> Result<ChatSession> {
        let messages_json: String = row.try_get("messages")?;

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
}

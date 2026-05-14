use anyhow::Result;
use std::str::FromStr;

use sqlx::{sqlite::SqliteConnectOptions, sqlite::SqlitePoolOptions, Pool, Row, Sqlite};
use tauri::{App, Manager};
use tracing::info;

use crate::models::{
    chat::ChatSession, dashboard::Dashboard, mcp::MCPServer, provider::LLMProvider,
    workflow::Workflow,
};

pub struct Storage {
    pool: Pool<Sqlite>,
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
        let app_dir = app.path().app_data_dir()?;
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

        info!("✅ Database migrations complete");
        Ok(())
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
    use crate::models::provider::{LLMProvider, ProviderKind};
    use crate::models::workflow::{RunStatus, TriggerKind, Workflow, WorkflowRun, WorkflowTrigger};
    use std::collections::HashMap;

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

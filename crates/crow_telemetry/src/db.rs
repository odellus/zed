//! Crow Telemetry Database
//!
//! SQLite-based storage for agent traces.

use crate::{AgentRole, Trace};
use anyhow::{Result, anyhow};
use gpui::{BackgroundExecutor, Task};
use indoc::indoc;
use parking_lot::Mutex;
use sqlez::connection::Connection;
use std::sync::Arc;
use zed_env_vars::ZED_STATELESS;

/// Database path for crow telemetry
fn crow_telemetry_db_path() -> Option<std::path::PathBuf> {
    if *ZED_STATELESS {
        return None;
    }

    dirs::data_dir().map(|data_dir| data_dir.join("crow").join("telemetry.db"))
}

/// Database for storing agent traces
#[derive(Clone)]
pub struct CrowTelemetryDb {
    executor: BackgroundExecutor,
    connection: Arc<Mutex<Connection>>,
}

impl CrowTelemetryDb {
    /// Connect to the crow telemetry database
    pub fn connect(cx: &gpui::App) -> Task<Result<Self>> {
        let executor = cx.background_executor().clone();

        executor.clone().spawn(async move {
            let db_path = crow_telemetry_db_path()
                .ok_or_else(|| anyhow!("No data directory available for crow telemetry"))?;

            // Ensure directory exists
            if let Some(parent) = db_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let connection = Connection::open_file(&db_path.to_string_lossy());
            Self::initialize(executor, connection)
        })
    }

    fn initialize(executor: BackgroundExecutor, connection: Connection) -> Result<Self> {
        // Create traces table
        connection.exec(indoc! {"
            CREATE TABLE IF NOT EXISTS traces (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                agent_role TEXT NOT NULL,
                started_at TEXT NOT NULL,
                data TEXT NOT NULL
            )
        "})?()
        .map_err(|e| anyhow!("Failed to create traces table: {}", e))?;

        // Indexes for common queries
        connection.exec(indoc! {"
            CREATE INDEX IF NOT EXISTS idx_traces_session ON traces(session_id)
        "})?()
        .map_err(|e| anyhow!("Failed to create traces session index: {}", e))?;

        connection.exec(indoc! {"
            CREATE INDEX IF NOT EXISTS idx_traces_agent_role ON traces(agent_role)
        "})?()
        .map_err(|e| anyhow!("Failed to create traces agent_role index: {}", e))?;

        connection.exec(indoc! {"
            CREATE INDEX IF NOT EXISTS idx_traces_started_at ON traces(started_at)
        "})?()
        .map_err(|e| anyhow!("Failed to create traces started_at index: {}", e))?;

        Ok(Self {
            executor,
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    /// Save a trace to the database
    pub fn save_trace(&self, trace: Trace) -> Task<Result<()>> {
        let connection = self.connection.clone();

        self.executor.spawn(async move {
            let data = serde_json::to_string(&trace)?;
            let connection = connection.lock();

            let mut insert = connection.exec_bound::<(String, String, String, String, String)>(indoc! {"
                INSERT OR REPLACE INTO traces (id, session_id, agent_role, started_at, data)
                VALUES (?, ?, ?, ?, ?)
            "})?;

            insert((
                trace.id,
                trace.session_id,
                trace.agent_role.as_str().to_string(),
                trace.started_at.to_rfc3339(),
                data,
            ))?;

            Ok(())
        })
    }

    /// Get a trace by ID
    pub fn get_trace(&self, id: String) -> Task<Result<Option<Trace>>> {
        let connection = self.connection.clone();

        self.executor.spawn(async move {
            let connection = connection.lock();

            let mut select = connection.select_bound::<String, String>(indoc! {"
                SELECT data FROM traces WHERE id = ? LIMIT 1
            "})?;

            let rows = select(id)?;
            if let Some(data) = rows.into_iter().next() {
                let trace: Trace = serde_json::from_str(&data)?;
                Ok(Some(trace))
            } else {
                Ok(None)
            }
        })
    }

    /// List traces for a session
    pub fn list_traces_for_session(&self, session_id: String) -> Task<Result<Vec<Trace>>> {
        let connection = self.connection.clone();

        self.executor.spawn(async move {
            let connection = connection.lock();

            let mut select = connection.select_bound::<String, String>(indoc! {"
                SELECT data FROM traces WHERE session_id = ? ORDER BY started_at ASC
            "})?;

            let rows = select(session_id)?;
            let mut traces = Vec::new();

            for data in rows {
                if let Ok(trace) = serde_json::from_str::<Trace>(&data) {
                    traces.push(trace);
                }
            }

            Ok(traces)
        })
    }

    /// List recent traces (across all sessions)
    pub fn list_recent_traces(&self, limit: usize) -> Task<Result<Vec<Trace>>> {
        let connection = self.connection.clone();

        self.executor.spawn(async move {
            let connection = connection.lock();

            let query = format!(
                "SELECT data FROM traces ORDER BY started_at DESC LIMIT {}",
                limit
            );

            let mut select = connection.select_bound::<(), String>(&query)?;

            let rows = select(())?;
            let mut traces = Vec::new();

            for data in rows {
                if let Ok(trace) = serde_json::from_str::<Trace>(&data) {
                    traces.push(trace);
                }
            }

            Ok(traces)
        })
    }

    /// List recent traces filtered by agent role
    pub fn list_traces_by_role(&self, role: AgentRole, limit: usize) -> Task<Result<Vec<Trace>>> {
        let connection = self.connection.clone();
        let role_str = role.as_str().to_string();

        self.executor.spawn(async move {
            let connection = connection.lock();

            let query = format!(
                "SELECT data FROM traces WHERE agent_role = ? ORDER BY started_at DESC LIMIT {}",
                limit
            );

            let mut select = connection.select_bound::<String, String>(&query)?;

            let rows = select(role_str)?;
            let mut traces = Vec::new();

            for data in rows {
                if let Ok(trace) = serde_json::from_str::<Trace>(&data) {
                    traces.push(trace);
                }
            }

            Ok(traces)
        })
    }

    /// List recent traces from external agents (Claude Code, Gemini, etc.)
    pub fn list_external_traces(&self, limit: usize, session_id: Option<&str>) -> Task<Result<Vec<Trace>>> {
        let connection = self.connection.clone();
        let session_id = session_id.map(|s| s.to_string());

        self.executor.spawn(async move {
            let connection = connection.lock();

            // Query for all external agent roles, optionally filtered by session
            let query = match &session_id {
                Some(sid) => format!(
                    "SELECT data FROM traces WHERE agent_role IN ('external_claude_code', 'external_gemini', 'external_custom') AND session_id = '{}' ORDER BY started_at DESC LIMIT {}",
                    sid, limit
                ),
                None => format!(
                    "SELECT data FROM traces WHERE agent_role IN ('external_claude_code', 'external_gemini', 'external_custom') ORDER BY started_at DESC LIMIT {}",
                    limit
                ),
            };

            let mut select = connection.select_bound::<(), String>(&query)?;

            let rows = select(())?;
            let mut traces = Vec::new();

            for data in rows {
                if let Ok(trace) = serde_json::from_str::<Trace>(&data) {
                    traces.push(trace);
                }
            }

            Ok(traces)
        })
    }
}

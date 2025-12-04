use crate::{AgentMessage, AgentMessageContent, UserMessage, UserMessageContent};
use acp_thread::UserMessageId;
use agent_client_protocol as acp;
use agent_settings::{AgentProfileId, CompletionMode};
use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use collections::{HashMap, IndexMap};
use futures::{FutureExt, future::Shared};
use gpui::{BackgroundExecutor, Global, Task};
use indoc::indoc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sqlez::{
    bindable::{Bind, Column},
    connection::Connection,
    statement::Statement,
};
use std::sync::Arc;
use ui::{App, SharedString};
use zed_env_vars::ZED_STATELESS;

pub type DbMessage = crate::Message;
pub type DbSummary = crate::legacy_thread::DetailedSummaryState;
pub type DbLanguageModel = crate::legacy_thread::SerializedLanguageModel;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbThreadMetadata {
    pub id: acp::SessionId,
    #[serde(alias = "summary")]
    pub title: SharedString,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DbThread {
    pub title: SharedString,
    pub messages: Vec<DbMessage>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub detailed_summary: Option<SharedString>,
    #[serde(default)]
    pub initial_project_snapshot: Option<Arc<crate::ProjectSnapshot>>,
    #[serde(default)]
    pub cumulative_token_usage: language_model::TokenUsage,
    #[serde(default)]
    pub request_token_usage: HashMap<acp_thread::UserMessageId, language_model::TokenUsage>,
    #[serde(default)]
    pub model: Option<DbLanguageModel>,
    #[serde(default)]
    pub completion_mode: Option<CompletionMode>,
    #[serde(default)]
    pub profile: Option<AgentProfileId>,
}

impl DbThread {
    pub const VERSION: &'static str = "0.3.0";

    pub fn from_json(json: &[u8]) -> Result<Self> {
        let saved_thread_json = serde_json::from_slice::<serde_json::Value>(json)?;
        match saved_thread_json.get("version") {
            Some(serde_json::Value::String(version)) => match version.as_str() {
                Self::VERSION => Ok(serde_json::from_value(saved_thread_json)?),
                _ => Self::upgrade_from_agent_1(crate::legacy_thread::SerializedThread::from_json(
                    json,
                )?),
            },
            _ => {
                Self::upgrade_from_agent_1(crate::legacy_thread::SerializedThread::from_json(json)?)
            }
        }
    }

    fn upgrade_from_agent_1(thread: crate::legacy_thread::SerializedThread) -> Result<Self> {
        let mut messages = Vec::new();
        let mut request_token_usage = HashMap::default();

        let mut last_user_message_id = None;
        for (ix, msg) in thread.messages.into_iter().enumerate() {
            let message = match msg.role {
                language_model::Role::User => {
                    let mut content = Vec::new();

                    // Convert segments to content
                    for segment in msg.segments {
                        match segment {
                            crate::legacy_thread::SerializedMessageSegment::Text { text } => {
                                content.push(UserMessageContent::Text(text));
                            }
                            crate::legacy_thread::SerializedMessageSegment::Thinking {
                                text,
                                ..
                            } => {
                                // User messages don't have thinking segments, but handle gracefully
                                content.push(UserMessageContent::Text(text));
                            }
                            crate::legacy_thread::SerializedMessageSegment::RedactedThinking {
                                ..
                            } => {
                                // User messages don't have redacted thinking, skip.
                            }
                        }
                    }

                    // If no content was added, add context as text if available
                    if content.is_empty() && !msg.context.is_empty() {
                        content.push(UserMessageContent::Text(msg.context));
                    }

                    let id = UserMessageId::new();
                    last_user_message_id = Some(id.clone());

                    crate::Message::User(UserMessage {
                        // MessageId from old format can't be meaningfully converted, so generate a new one
                        id,
                        content,
                    })
                }
                language_model::Role::Assistant => {
                    let mut content = Vec::new();

                    // Convert segments to content
                    for segment in msg.segments {
                        match segment {
                            crate::legacy_thread::SerializedMessageSegment::Text { text } => {
                                content.push(AgentMessageContent::Text(text));
                            }
                            crate::legacy_thread::SerializedMessageSegment::Thinking {
                                text,
                                signature,
                            } => {
                                content.push(AgentMessageContent::Thinking { text, signature });
                            }
                            crate::legacy_thread::SerializedMessageSegment::RedactedThinking {
                                data,
                            } => {
                                content.push(AgentMessageContent::RedactedThinking(data));
                            }
                        }
                    }

                    // Convert tool uses
                    let mut tool_names_by_id = HashMap::default();
                    for tool_use in msg.tool_uses {
                        tool_names_by_id.insert(tool_use.id.clone(), tool_use.name.clone());
                        content.push(AgentMessageContent::ToolUse(
                            language_model::LanguageModelToolUse {
                                id: tool_use.id,
                                name: tool_use.name.into(),
                                raw_input: serde_json::to_string(&tool_use.input)
                                    .unwrap_or_default(),
                                input: tool_use.input,
                                is_input_complete: true,
                            },
                        ));
                    }

                    // Convert tool results
                    let mut tool_results = IndexMap::default();
                    for tool_result in msg.tool_results {
                        let name = tool_names_by_id
                            .remove(&tool_result.tool_use_id)
                            .unwrap_or_else(|| SharedString::from("unknown"));
                        tool_results.insert(
                            tool_result.tool_use_id.clone(),
                            language_model::LanguageModelToolResult {
                                tool_use_id: tool_result.tool_use_id,
                                tool_name: name.into(),
                                is_error: tool_result.is_error,
                                content: tool_result.content,
                                output: tool_result.output,
                            },
                        );
                    }

                    if let Some(last_user_message_id) = &last_user_message_id
                        && let Some(token_usage) = thread.request_token_usage.get(ix).copied()
                    {
                        request_token_usage.insert(last_user_message_id.clone(), token_usage);
                    }

                    crate::Message::Agent(AgentMessage {
                        content,
                        tool_results,
                    })
                }
                language_model::Role::System => {
                    // Skip system messages as they're not supported in the new format
                    continue;
                }
            };

            messages.push(message);
        }

        Ok(Self {
            title: thread.summary,
            messages,
            updated_at: thread.updated_at,
            detailed_summary: match thread.detailed_summary_state {
                crate::legacy_thread::DetailedSummaryState::NotGenerated
                | crate::legacy_thread::DetailedSummaryState::Generating => None,
                crate::legacy_thread::DetailedSummaryState::Generated { text, .. } => Some(text),
            },
            initial_project_snapshot: thread.initial_project_snapshot,
            cumulative_token_usage: thread.cumulative_token_usage,
            request_token_usage,
            model: thread.model,
            completion_mode: thread.completion_mode,
            profile: thread.profile,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataType {
    #[serde(rename = "json")]
    Json,
    #[serde(rename = "zstd")]
    Zstd,
}

impl Bind for DataType {
    fn bind(&self, statement: &Statement, start_index: i32) -> Result<i32> {
        let value = match self {
            DataType::Json => "json",
            DataType::Zstd => "zstd",
        };
        value.bind(statement, start_index)
    }
}

impl Column for DataType {
    fn column(statement: &mut Statement, start_index: i32) -> Result<(Self, i32)> {
        let (value, next_index) = String::column(statement, start_index)?;
        let data_type = match value.as_str() {
            "json" => DataType::Json,
            "zstd" => DataType::Zstd,
            _ => anyhow::bail!("Unknown data type: {}", value),
        };
        Ok((data_type, next_index))
    }
}

pub struct ThreadsDatabase {
    executor: BackgroundExecutor,
    connection: Arc<Mutex<Connection>>,
}

struct GlobalThreadsDatabase(Shared<Task<Result<Arc<ThreadsDatabase>, Arc<anyhow::Error>>>>);

impl Global for GlobalThreadsDatabase {}

impl ThreadsDatabase {
    pub fn connect(cx: &mut App) -> Shared<Task<Result<Arc<ThreadsDatabase>, Arc<anyhow::Error>>>> {
        if cx.has_global::<GlobalThreadsDatabase>() {
            return cx.global::<GlobalThreadsDatabase>().0.clone();
        }
        let executor = cx.background_executor().clone();
        let task = executor
            .spawn({
                let executor = executor.clone();
                async move {
                    match ThreadsDatabase::new(executor) {
                        Ok(db) => Ok(Arc::new(db)),
                        Err(err) => Err(Arc::new(err)),
                    }
                }
            })
            .shared();

        cx.set_global(GlobalThreadsDatabase(task.clone()));
        task
    }

    pub fn new(executor: BackgroundExecutor) -> Result<Self> {
        let connection = if *ZED_STATELESS {
            Connection::open_memory(Some("THREAD_FALLBACK_DB"))
        } else if cfg!(any(feature = "test-support", test)) {
            // rust stores the name of the test on the current thread.
            // We use this to automatically create a database that will
            // be shared within the test (for the test_retrieve_old_thread)
            // but not with concurrent tests.
            let thread = std::thread::current();
            let test_name = thread.name();
            Connection::open_memory(Some(&format!(
                "THREAD_FALLBACK_{}",
                test_name.unwrap_or_default()
            )))
        } else {
            let threads_dir = paths::data_dir().join("threads");
            std::fs::create_dir_all(&threads_dir)?;
            let sqlite_path = threads_dir.join("threads.db");
            Connection::open_file(&sqlite_path.to_string_lossy())
        };

        connection.exec(indoc! {"
            CREATE TABLE IF NOT EXISTS threads (
                id TEXT PRIMARY KEY,
                summary TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                data_type TEXT NOT NULL,
                data BLOB NOT NULL
            )
        "})?()
        .map_err(|e| anyhow!("Failed to create threads table: {}", e))?;

        // Session pairs table for dual-agent (auto) mode telemetry
        // Links executor sessions to their discriminator sessions
        connection.exec(indoc! {"
            CREATE TABLE IF NOT EXISTS session_pairs (
                executor_session_id TEXT PRIMARY KEY,
                discriminator_session_id TEXT NOT NULL,
                created_at TEXT NOT NULL
            )
        "})?()
        .map_err(|e| anyhow!("Failed to create session_pairs table: {}", e))?;

        // Prompts table - version control for prompt templates
        // Each unique (name, template_hash) pair is a distinct version
        connection.exec(indoc! {"
            CREATE TABLE IF NOT EXISTS prompts (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                template_hash TEXT NOT NULL,
                template_content TEXT NOT NULL,
                input_schema TEXT,
                created_at TEXT NOT NULL,
                UNIQUE(name, template_hash)
            )
        "})?()
        .map_err(|e| anyhow!("Failed to create prompts table: {}", e))?;

        // Index for looking up prompts by name (to find all versions)
        connection.exec(indoc! {"
            CREATE INDEX IF NOT EXISTS idx_prompts_name ON prompts(name)
        "})?()
        .map_err(|e| anyhow!("Failed to create prompts name index: {}", e))?;

        // Traces table - full telemetry for every LLM call
        // We store most data as a JSON blob for flexibility and to avoid tuple size limits
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

        // Indexes for common trace queries
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

        let db = Self {
            executor,
            connection: Arc::new(Mutex::new(connection)),
        };

        Ok(db)
    }

    fn save_thread_sync(
        connection: &Arc<Mutex<Connection>>,
        id: acp::SessionId,
        thread: DbThread,
    ) -> Result<()> {
        const COMPRESSION_LEVEL: i32 = 3;

        #[derive(Serialize)]
        struct SerializedThread {
            #[serde(flatten)]
            thread: DbThread,
            version: &'static str,
        }

        let title = thread.title.to_string();
        let updated_at = thread.updated_at.to_rfc3339();
        let json_data = serde_json::to_string(&SerializedThread {
            thread,
            version: DbThread::VERSION,
        })?;

        let connection = connection.lock();

        let compressed = zstd::encode_all(json_data.as_bytes(), COMPRESSION_LEVEL)?;
        let data_type = DataType::Zstd;
        let data = compressed;

        let mut insert = connection.exec_bound::<(Arc<str>, String, String, DataType, Vec<u8>)>(indoc! {"
            INSERT OR REPLACE INTO threads (id, summary, updated_at, data_type, data) VALUES (?, ?, ?, ?, ?)
        "})?;

        insert((id.0, title, updated_at, data_type, data))?;

        Ok(())
    }

    pub fn list_threads(&self) -> Task<Result<Vec<DbThreadMetadata>>> {
        let connection = self.connection.clone();

        self.executor.spawn(async move {
            let connection = connection.lock();

            let mut select =
                connection.select_bound::<(), (Arc<str>, String, String)>(indoc! {"
                SELECT id, summary, updated_at FROM threads ORDER BY updated_at DESC
            "})?;

            let rows = select(())?;
            let mut threads = Vec::new();

            for (id, summary, updated_at) in rows {
                threads.push(DbThreadMetadata {
                    id: acp::SessionId(id),
                    title: summary.into(),
                    updated_at: DateTime::parse_from_rfc3339(&updated_at)?.with_timezone(&Utc),
                });
            }

            Ok(threads)
        })
    }

    pub fn load_thread(&self, id: acp::SessionId) -> Task<Result<Option<DbThread>>> {
        let connection = self.connection.clone();

        self.executor.spawn(async move {
            let connection = connection.lock();
            let mut select = connection.select_bound::<Arc<str>, (DataType, Vec<u8>)>(indoc! {"
                SELECT data_type, data FROM threads WHERE id = ? LIMIT 1
            "})?;

            let rows = select(id.0)?;
            if let Some((data_type, data)) = rows.into_iter().next() {
                let json_data = match data_type {
                    DataType::Zstd => {
                        let decompressed = zstd::decode_all(&data[..])?;
                        String::from_utf8(decompressed)?
                    }
                    DataType::Json => String::from_utf8(data)?,
                };
                let thread = DbThread::from_json(json_data.as_bytes())?;
                Ok(Some(thread))
            } else {
                Ok(None)
            }
        })
    }

    pub fn save_thread(&self, id: acp::SessionId, thread: DbThread) -> Task<Result<()>> {
        let connection = self.connection.clone();

        self.executor
            .spawn(async move { Self::save_thread_sync(&connection, id, thread) })
    }

    pub fn delete_thread(&self, id: acp::SessionId) -> Task<Result<()>> {
        let connection = self.connection.clone();

        self.executor.spawn(async move {
            let connection = connection.lock();

            let mut delete = connection.exec_bound::<Arc<str>>(indoc! {"
                DELETE FROM threads WHERE id = ?
            "})?;

            delete(id.0)?;

            Ok(())
        })
    }

    /// Save a session pair linking executor to discriminator (for dual-agent/auto mode)
    pub fn save_session_pair(
        &self,
        executor_session_id: acp::SessionId,
        discriminator_session_id: acp::SessionId,
    ) -> Task<Result<()>> {
        let connection = self.connection.clone();

        self.executor.spawn(async move {
            let connection = connection.lock();
            let created_at = Utc::now().to_rfc3339();

            let mut insert = connection.exec_bound::<(Arc<str>, Arc<str>, String)>(indoc! {"
                INSERT OR REPLACE INTO session_pairs (executor_session_id, discriminator_session_id, created_at)
                VALUES (?, ?, ?)
            "})?;

            insert((executor_session_id.0, discriminator_session_id.0, created_at))?;

            Ok(())
        })
    }

    /// Get the paired session for a given session ID (works for either executor or discriminator)
    pub fn get_session_pair(&self, session_id: acp::SessionId) -> Task<Result<Option<SessionPair>>> {
        let connection = self.connection.clone();

        self.executor.spawn(async move {
            let connection = connection.lock();

            // Try to find as executor first
            let mut select = connection.select_bound::<Arc<str>, (Arc<str>, String)>(indoc! {"
                SELECT discriminator_session_id, created_at FROM session_pairs WHERE executor_session_id = ? LIMIT 1
            "})?;

            if let Some((discriminator_id, created_at)) = select(session_id.0.clone())?.into_iter().next() {
                return Ok(Some(SessionPair {
                    executor_session_id: session_id,
                    discriminator_session_id: acp::SessionId(discriminator_id),
                    created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
                }));
            }

            // Try to find as discriminator
            let mut select = connection.select_bound::<Arc<str>, (Arc<str>, String)>(indoc! {"
                SELECT executor_session_id, created_at FROM session_pairs WHERE discriminator_session_id = ? LIMIT 1
            "})?;

            let session_id_for_query = session_id.0.clone();
            if let Some((executor_id, created_at)) = select(session_id_for_query)?.into_iter().next() {
                return Ok(Some(SessionPair {
                    executor_session_id: acp::SessionId(executor_id),
                    discriminator_session_id: session_id,
                    created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
                }));
            }

            Ok(None)
        })
    }

    /// List all session pairs
    pub fn list_session_pairs(&self) -> Task<Result<Vec<SessionPair>>> {
        let connection = self.connection.clone();

        self.executor.spawn(async move {
            let connection = connection.lock();

            let mut select = connection.select_bound::<(), (Arc<str>, Arc<str>, String)>(indoc! {"
                SELECT executor_session_id, discriminator_session_id, created_at FROM session_pairs ORDER BY created_at DESC
            "})?;

            let rows = select(())?;
            let mut pairs = Vec::new();

            for (executor_id, discriminator_id, created_at) in rows {
                pairs.push(SessionPair {
                    executor_session_id: acp::SessionId(executor_id),
                    discriminator_session_id: acp::SessionId(discriminator_id),
                    created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
                });
            }

            Ok(pairs)
        })
    }

    // ==================== Prompt Management ====================

    /// Register a prompt template, returning its ID.
    /// If this exact (name, hash) combo exists, returns existing ID.
    /// If it's a new version, inserts and returns new ID.
    pub fn register_prompt(
        &self,
        name: String,
        template_content: String,
        input_schema: Option<String>,
    ) -> Task<Result<String>> {
        let connection = self.connection.clone();

        self.executor.spawn(async move {
            let connection = connection.lock();

            // Hash the template content
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            template_content.hash(&mut hasher);
            let template_hash = format!("{:016x}", hasher.finish());

            // Check if this version already exists
            let mut select = connection.select_bound::<(String, String), String>(indoc! {"
                SELECT id FROM prompts WHERE name = ? AND template_hash = ? LIMIT 1
            "})?;

            if let Some(existing_id) = select((name.clone(), template_hash.clone()))?.into_iter().next() {
                return Ok(existing_id);
            }

            // Insert new version
            let id = uuid::Uuid::new_v4().to_string();
            let created_at = Utc::now().to_rfc3339();

            let mut insert = connection.exec_bound::<(String, String, String, String, Option<String>, String)>(indoc! {"
                INSERT INTO prompts (id, name, template_hash, template_content, input_schema, created_at)
                VALUES (?, ?, ?, ?, ?, ?)
            "})?;

            insert((id.clone(), name, template_hash, template_content, input_schema, created_at))?;

            Ok(id)
        })
    }

    /// Get a prompt by ID
    pub fn get_prompt(&self, id: String) -> Task<Result<Option<Prompt>>> {
        let connection = self.connection.clone();

        self.executor.spawn(async move {
            let connection = connection.lock();

            let mut select = connection.select_bound::<String, (String, String, String, String, Option<String>, String)>(indoc! {"
                SELECT id, name, template_hash, template_content, input_schema, created_at
                FROM prompts WHERE id = ? LIMIT 1
            "})?;

            let row = select(id)?.into_iter().next();

            match row {
                Some((id, name, template_hash, template_content, input_schema, created_at)) => {
                    Ok(Some(Prompt {
                        id,
                        name,
                        template_hash,
                        template_content,
                        input_schema,
                        created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
                    }))
                }
                None => Ok(None),
            }
        })
    }

    /// List all prompts (all versions)
    pub fn list_prompts(&self) -> Task<Result<Vec<Prompt>>> {
        let connection = self.connection.clone();

        self.executor.spawn(async move {
            let connection = connection.lock();

            let mut select = connection.select_bound::<(), (String, String, String, String, Option<String>, String)>(indoc! {"
                SELECT id, name, template_hash, template_content, input_schema, created_at
                FROM prompts ORDER BY name, created_at DESC
            "})?;

            let rows = select(())?;
            let mut prompts = Vec::new();

            for (id, name, template_hash, template_content, input_schema, created_at) in rows {
                prompts.push(Prompt {
                    id,
                    name,
                    template_hash,
                    template_content,
                    input_schema,
                    created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
                });
            }

            Ok(prompts)
        })
    }

    /// List all versions of a specific prompt by name
    pub fn list_prompt_versions(&self, name: String) -> Task<Result<Vec<Prompt>>> {
        let connection = self.connection.clone();

        self.executor.spawn(async move {
            let connection = connection.lock();

            let mut select = connection.select_bound::<String, (String, String, String, String, Option<String>, String)>(indoc! {"
                SELECT id, name, template_hash, template_content, input_schema, created_at
                FROM prompts WHERE name = ? ORDER BY created_at DESC
            "})?;

            let rows = select(name)?;
            let mut prompts = Vec::new();

            for (id, name, template_hash, template_content, input_schema, created_at) in rows {
                prompts.push(Prompt {
                    id,
                    name,
                    template_hash,
                    template_content,
                    input_schema,
                    created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
                });
            }

            Ok(prompts)
        })
    }

    // ==================== Trace Management ====================

    /// Save a completed trace
    pub fn save_trace(&self, trace: Trace) -> Task<Result<()>> {
        let connection = self.connection.clone();

        self.executor.spawn(async move {
            let connection = connection.lock();

            let data = serde_json::to_string(&trace)?;

            let mut insert = connection.exec_bound::<(String, String, String, String, String)>(indoc! {"
                INSERT INTO traces (id, session_id, agent_role, started_at, data)
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

            let row = select(id)?.into_iter().next();

            match row {
                Some(data) => {
                    let trace: Trace = serde_json::from_str(&data)?;
                    Ok(Some(trace))
                }
                None => Ok(None),
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

            // We need to filter by role which is stored in the JSON data
            // For efficiency, we fetch more and filter, or use JSON extraction if supported
            let query = format!(
                "SELECT data FROM traces ORDER BY started_at DESC LIMIT {}",
                limit * 10 // Fetch more to account for filtering
            );

            let mut select = connection.select_bound::<(), String>(&query)?;

            let rows = select(())?;
            let mut traces = Vec::new();

            for data in rows {
                if let Ok(trace) = serde_json::from_str::<Trace>(&data) {
                    if trace.agent_role.as_str() == role_str {
                        traces.push(trace);
                        if traces.len() >= limit {
                            break;
                        }
                    }
                }
            }

            Ok(traces)
        })
    }

    /// List recent traces from external agents (Claude Code, Gemini, etc.)
    pub fn list_external_traces(&self, limit: usize) -> Task<Result<Vec<Trace>>> {
        let connection = self.connection.clone();

        self.executor.spawn(async move {
            let connection = connection.lock();

            let query = format!(
                "SELECT data FROM traces ORDER BY started_at DESC LIMIT {}",
                limit * 10
            );

            let mut select = connection.select_bound::<(), String>(&query)?;

            let rows = select(())?;
            let mut traces = Vec::new();

            for data in rows {
                if let Ok(trace) = serde_json::from_str::<Trace>(&data) {
                    if trace.agent_role.is_external() {
                        traces.push(trace);
                        if traces.len() >= limit {
                            break;
                        }
                    }
                }
            }

            Ok(traces)
        })
    }
}

/// Represents a paired executor/discriminator session for dual-agent mode
#[derive(Debug, Clone)]
pub struct SessionPair {
    pub executor_session_id: acp::SessionId,
    pub discriminator_session_id: acp::SessionId,
    pub created_at: DateTime<Utc>,
}

/// Represents a versioned prompt template
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prompt {
    pub id: String,
    pub name: String,
    pub template_hash: String,
    pub template_content: String,
    pub input_schema: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Role of the agent making an LLM call
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentRole {
    Executor,
    Discriminator,
    EditAgent,
    DiffJudge,
    /// External agent: Claude Code via ACP
    ExternalClaudeCode,
    /// External agent: Gemini via ACP
    ExternalGemini,
    /// External agent: Custom/other via ACP
    ExternalCustom,
}

impl AgentRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentRole::Executor => "executor",
            AgentRole::Discriminator => "discriminator",
            AgentRole::EditAgent => "edit_agent",
            AgentRole::DiffJudge => "diff_judge",
            AgentRole::ExternalClaudeCode => "external_claude_code",
            AgentRole::ExternalGemini => "external_gemini",
            AgentRole::ExternalCustom => "external_custom",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "executor" => Some(AgentRole::Executor),
            "discriminator" => Some(AgentRole::Discriminator),
            "edit_agent" => Some(AgentRole::EditAgent),
            "diff_judge" => Some(AgentRole::DiffJudge),
            "external_claude_code" => Some(AgentRole::ExternalClaudeCode),
            "external_gemini" => Some(AgentRole::ExternalGemini),
            "external_custom" => Some(AgentRole::ExternalCustom),
            _ => None,
        }
    }

    /// Returns true if this is an external agent (ACP-based)
    pub fn is_external(&self) -> bool {
        matches!(
            self,
            AgentRole::ExternalClaudeCode | AgentRole::ExternalGemini | AgentRole::ExternalCustom
        )
    }
}

/// Full telemetry record for an LLM call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trace {
    pub id: String,
    pub session_id: String,
    pub thread_id: Option<String>,
    pub prompt_id: Option<String>,
    pub agent_role: AgentRole,
    pub model_provider: String,
    pub model_id: String,
    pub template_inputs: Option<String>,
    pub rendered_prompt: Option<String>,
    pub request_messages: String,
    pub request_tools: Option<String>,
    pub response_content: Option<String>,
    pub response_tool_calls: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub latency_ms: Option<i64>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

/// Builder for creating a trace - start before the LLM call, complete after
#[derive(Debug, Clone)]
pub struct TraceBuilder {
    pub id: String,
    pub session_id: String,
    pub thread_id: Option<String>,
    pub prompt_id: Option<String>,
    pub agent_role: AgentRole,
    pub model_provider: String,
    pub model_id: String,
    pub template_inputs: Option<String>,
    pub rendered_prompt: Option<String>,
    pub request_messages: String,
    pub request_tools: Option<String>,
    pub started_at: DateTime<Utc>,
}

impl TraceBuilder {
    pub fn new(
        session_id: String,
        agent_role: AgentRole,
        model_provider: String,
        model_id: String,
        request_messages: String,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            session_id,
            thread_id: None,
            prompt_id: None,
            agent_role,
            model_provider,
            model_id,
            template_inputs: None,
            rendered_prompt: None,
            request_messages,
            request_tools: None,
            started_at: Utc::now(),
        }
    }

    pub fn with_thread_id(mut self, thread_id: String) -> Self {
        self.thread_id = Some(thread_id);
        self
    }

    pub fn with_prompt(mut self, prompt_id: String, template_inputs: String, rendered: String) -> Self {
        self.prompt_id = Some(prompt_id);
        self.template_inputs = Some(template_inputs);
        self.rendered_prompt = Some(rendered);
        self
    }

    pub fn with_tools(mut self, tools: String) -> Self {
        self.request_tools = Some(tools);
        self
    }

    pub fn complete(
        self,
        response_content: Option<String>,
        response_tool_calls: Option<String>,
        input_tokens: Option<i64>,
        output_tokens: Option<i64>,
        total_tokens: Option<i64>,
    ) -> Trace {
        let completed_at = Utc::now();
        let latency_ms = (completed_at - self.started_at).num_milliseconds();

        Trace {
            id: self.id,
            session_id: self.session_id,
            thread_id: self.thread_id,
            prompt_id: self.prompt_id,
            agent_role: self.agent_role,
            model_provider: self.model_provider,
            model_id: self.model_id,
            template_inputs: self.template_inputs,
            rendered_prompt: self.rendered_prompt,
            request_messages: self.request_messages,
            request_tools: self.request_tools,
            response_content,
            response_tool_calls,
            input_tokens,
            output_tokens,
            total_tokens,
            latency_ms: Some(latency_ms),
            started_at: self.started_at,
            completed_at: Some(completed_at),
            error: None,
        }
    }

    pub fn fail(self, error: String) -> Trace {
        let completed_at = Utc::now();
        let latency_ms = (completed_at - self.started_at).num_milliseconds();

        Trace {
            id: self.id,
            session_id: self.session_id,
            thread_id: self.thread_id,
            prompt_id: self.prompt_id,
            agent_role: self.agent_role,
            model_provider: self.model_provider,
            model_id: self.model_id,
            template_inputs: self.template_inputs,
            rendered_prompt: self.rendered_prompt,
            request_messages: self.request_messages,
            request_tools: self.request_tools,
            response_content: None,
            response_tool_calls: None,
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            latency_ms: Some(latency_ms),
            started_at: self.started_at,
            completed_at: Some(completed_at),
            error: Some(error),
        }
    }
}

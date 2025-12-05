use acp_thread::AgentConnection;
use anyhow::{Context as _, Result};
use client::Client;
use fs::RealFs;
use gpui::{App, AppContext as _, AsyncApp, Entity, SemanticVersion, UpdateGlobal as _};
use language::LanguageRegistry;
use language_model::LanguageModelRegistry;
use node_runtime::NodeRuntime;
use project::Project;
use prompt_store::PromptBuilder;
use release_channel::ReleaseChannel;
use reqwest_client::ReqwestClient;
use serde::Deserialize;
use settings::{Settings as _, SettingsStore};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use paths;

/// Auth entry from crow's auth.json file
#[derive(Debug, Deserialize)]
struct AuthEntry {
    #[serde(rename = "type")]
    auth_type: String,
    key: String,
}

/// Mapping from auth.json provider names to environment variable names
fn provider_to_env_var(provider: &str) -> Option<&'static str> {
    match provider {
        "moonshotai" | "moonshot" => Some("MOONSHOT_API_KEY"),
        "anthropic" => Some("ANTHROPIC_API_KEY"),
        "openai" => Some("OPENAI_API_KEY"),
        "google" | "google-ai" => Some("GOOGLE_AI_API_KEY"),
        "deepseek" => Some("DEEPSEEK_API_KEY"),
        "mistral" => Some("MISTRAL_API_KEY"),
        "groq" => Some("GROQ_API_KEY"),
        "xai" | "x-ai" => Some("XAI_API_KEY"),
        "lm-studio2" => Some("LM_STUDIO_2_API_KEY"),
        _ => None,
    }
}

/// Load API keys from ~/.local/share/crow/auth.json and set as environment variables.
/// This allows crow-cli to use credentials stored by the crow auth system.
fn load_crow_auth() {
    let auth_path = dirs::data_dir()
        .map(|d| d.join("crow").join("auth.json"))
        .unwrap_or_else(|| PathBuf::from("~/.local/share/crow/auth.json"));

    let content = match std::fs::read_to_string(&auth_path) {
        Ok(c) => c,
        Err(_) => {
            log::debug!("No crow auth.json found at {:?}", auth_path);
            return;
        }
    };

    let auth_data: HashMap<String, AuthEntry> = match serde_json::from_str(&content) {
        Ok(d) => d,
        Err(e) => {
            log::warn!("Failed to parse crow auth.json: {}", e);
            return;
        }
    };

    for (provider, entry) in auth_data {
        if entry.auth_type != "api" {
            continue;
        }
        if let Some(env_var) = provider_to_env_var(&provider) {
            // Only set if not already set (env var takes precedence)
            if std::env::var(env_var).is_err() {
                // SAFETY: We're setting env vars before any threads are spawned,
                // and only once during initialization.
                unsafe { std::env::set_var(env_var, &entry.key) };
                log::info!("Loaded {} from crow auth.json", env_var);
            }
        } else {
            log::debug!("Unknown provider in auth.json: {}", provider);
        }
    }
}

use agent::{HistoryStore, NativeAgent, NativeAgentConnection, Templates};
use agent_settings::AgentSettings;
use assistant_text_thread::TextThreadStore;

/// All the initialized components needed to run the agent
pub struct CrowContext {
    pub agent: Entity<NativeAgent>,
    pub connection: Rc<NativeAgentConnection>,
    pub project: Entity<Project>,
    pub fs: Arc<dyn fs::Fs>,
    pub cwd: PathBuf,
}

impl CrowContext {
    /// Create a new thread for this session
    pub fn new_thread(&self, cx: &mut App) -> gpui::Task<Result<Entity<acp_thread::AcpThread>>> {
        let connection = self.connection.clone();
        let project = self.project.clone();
        let cwd = self.cwd.clone();
        AgentConnection::new_thread(connection, project, &cwd, cx)
    }
}

/// Minimal initialization for database-only operations (listing sessions, etc.)
/// This skips provider authentication and project setup which can be slow or hang.
pub fn initialize_minimal(cx: &mut AsyncApp) -> Result<()> {
    // Load user settings from ~/.config/zed/settings.json
    let user_settings_content = std::fs::read_to_string(paths::settings_file())
        .ok()
        .unwrap_or_default();

    cx.update(|cx| {
        // Initialize tokio runtime for async operations
        gpui_tokio::init(cx);

        // Set release channel to Dev to use file-based credentials instead of keyring
        // This avoids blocking keyring prompts in headless/CLI mode
        release_channel::init_test(SemanticVersion::default(), ReleaseChannel::Dev, cx);

        let settings_store = SettingsStore::new(cx, &settings::default_settings());
        cx.set_global(settings_store);

        // Register agent settings (needed for paths)
        AgentSettings::register(cx);

        // Load user settings from config file
        if !user_settings_content.is_empty() {
            let parse_result = SettingsStore::update_global(cx, |store, cx| {
                store.set_user_settings(&user_settings_content, cx)
            });
            if parse_result.requires_user_action() {
                log::warn!("Settings parse issues: {:?}", parse_result);
            }
        }
    })?;

    Ok(())
}

/// Initialize all the components needed to run the agent in headless mode
pub async fn initialize(cx: &mut AsyncApp) -> Result<CrowContext> {
    let cwd = std::env::current_dir().context("Failed to get current directory")?;

    // Load API keys from crow's auth.json before provider initialization
    load_crow_auth();

    // Load user settings from ~/.config/zed/settings.json
    let user_settings_content = std::fs::read_to_string(paths::settings_file())
        .ok()
        .unwrap_or_default();

    // Initialize settings store and basic infrastructure
    let (fs, client, user_store, languages, authenticate_tasks) = cx.update(|cx| {
        // Initialize tokio runtime for async operations
        gpui_tokio::init(cx);

        // Set release channel to Dev to use file-based credentials instead of keyring
        // This avoids blocking keyring prompts in headless/CLI mode
        release_channel::init_test(SemanticVersion::default(), ReleaseChannel::Dev, cx);

        let settings_store = SettingsStore::new(cx, &settings::default_settings());
        cx.set_global(settings_store);

        // Register agent settings
        AgentSettings::register(cx);

        // Register language model settings before loading user settings
        language_models::AllLanguageModelSettings::register(cx);

        // Load user settings from config file
        if !user_settings_content.is_empty() {
            let parse_result = SettingsStore::update_global(cx, |store, cx| {
                store.set_user_settings(&user_settings_content, cx)
            });
            if parse_result.requires_user_action() {
                log::warn!("Settings parse issues: {:?}", parse_result);
            } else {
                log::info!("Loaded user settings from {:?}", paths::settings_file());
            }
        }

        // Initialize theme (needed for some components)
        theme::init(theme::LoadThemes::JustBase, cx);

        // Create HTTP client and set it globally
        let http_client = Arc::new(ReqwestClient::new());
        cx.set_http_client(http_client);

        // Create the filesystem - REAL filesystem, not fake
        let fs: Arc<dyn fs::Fs> = Arc::new(RealFs::new(None, cx.background_executor().clone()));

        // Create the Zed client using production config
        let client = Client::production(cx);

        // Create user store
        let user_store = cx.new(|cx| client::UserStore::new(client.clone(), cx));

        // Create language registry
        let languages = Arc::new(LanguageRegistry::new(cx.background_executor().clone()));

        // Initialize language model registry and providers
        language_model::init(client.clone(), cx);
        language_models::init(user_store.clone(), client.clone(), cx);

        // Debug: check what OpenAI-compatible providers are configured
        let openai_compatible_keys: Vec<_> = language_models::AllLanguageModelSettings::get_global(cx)
            .openai_compatible
            .keys()
            .cloned()
            .collect();
        log::info!("OpenAI-compatible providers from settings: {:?}", openai_compatible_keys);

        // Trigger authentication for all providers (reads env vars, keychains, etc.)
        let all_providers: Vec<_> = LanguageModelRegistry::global(cx)
            .read(cx)
            .providers()
            .iter()
            .map(|p| p.id().0.to_string())
            .collect();
        log::info!("All registered providers: {:?}", all_providers);

        let authenticate_tasks = LanguageModelRegistry::global(cx)
            .read(cx)
            .providers()
            .iter()
            .map(|provider| (provider.id(), provider.name(), provider.authenticate(cx)))
            .collect::<Vec<_>>();

        (fs, client, user_store, languages, authenticate_tasks)
    })?;

    // Wait for all providers to authenticate
    for (provider_id, provider_name, authenticate_task) in authenticate_tasks {
        match authenticate_task.await {
            Ok(()) => {
                log::info!("Successfully authenticated provider {} ({})", provider_name.0, provider_id.0);
            }
            Err(err) => {
                log::debug!(
                    "Failed to authenticate provider {} ({}): {:?}",
                    provider_name.0,
                    provider_id.0,
                    err
                );
            }
        }
    }

    // Set up default model from agent settings AFTER authentication completes
    // This is normally done by agent_ui::init_language_model_settings()
    cx.update(|cx| {
        let default_model_selection = {
            let agent_settings = AgentSettings::get_global(cx);
            agent_settings.default_model.as_ref().map(|dm| {
                (
                    language_model::SelectedModel {
                        provider: language_model::LanguageModelProviderId::from(dm.provider.0.clone()),
                        model: language_model::LanguageModelId::from(dm.model.clone()),
                    },
                    dm.provider.0.clone(),
                    dm.model.clone(),
                )
            })
        };
        if let Some((selected, provider, model)) = default_model_selection {
            LanguageModelRegistry::global(cx).update(cx, |registry, cx| {
                registry.select_default_model(Some(&selected), cx);
            });
            log::info!("Set default model to {}/{}", provider, model);
        } else {
            log::warn!("No default model configured in agent settings");
        }

        // Debug: list available models after auth
        let available_models: Vec<_> = LanguageModelRegistry::global(cx)
            .read(cx)
            .available_models(cx)
            .map(|m| format!("{}/{}", m.provider_id().0, m.id().0))
            .collect();
        log::info!("Available models after auth: {:?}", available_models);
    })?;

    // Create the project pointing at cwd
    let project = cx.update(|cx| {
        Project::local(
            client.clone(),
            NodeRuntime::unavailable(),
            user_store.clone(),
            languages,
            fs.clone(),
            None, // env vars
            cx,
        )
    })?;

    // Add the current directory as a worktree
    let (worktree, _) = project
        .update(cx, |project, cx| {
            project.find_or_create_worktree(&cwd, true, cx)
        })?
        .await
        .context("Failed to create worktree for current directory")?;

    // Wait for the filesystem scan to complete
    worktree
        .read_with(cx, |tree, _| {
            tree.as_local()
                .expect("Local worktree expected")
                .scan_complete()
        })?
        .await;

    log::info!("Project initialized with worktree at {:?}", cwd);

    // Create templates for system prompts
    let templates = Templates::new();

    // Register prompt templates with database for telemetry
    let db = cx
        .update(|cx| agent::ThreadsDatabase::connect(cx))?
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    templates.register_with_database(&db).await
        .context("Failed to register prompt templates")?;

    // Create prompt builder and slash command registry (synchronous operations)
    let (prompt_builder, slash_command_registry, text_thread_store_task) = cx.update(|cx| {
        let stdout_is_a_pty = false;
        let prompt_builder = PromptBuilder::load(fs.clone(), stdout_is_a_pty, cx);
        let slash_command_registry =
            Arc::new(assistant_slash_command::SlashCommandWorkingSet::default());

        // Create text thread store (returns a task)
        let text_thread_store_task = TextThreadStore::new(
            project.clone(),
            prompt_builder.clone(),
            slash_command_registry.clone(),
            cx,
        );

        (prompt_builder, slash_command_registry, text_thread_store_task)
    })?;

    // Await the text thread store creation
    let text_thread_store = text_thread_store_task
        .await
        .context("Failed to create TextThreadStore")?;

    // Create history store
    let history = cx.update(|cx| cx.new(|cx| HistoryStore::new(text_thread_store, cx)))?;

    // Create the agent
    let agent = NativeAgent::new(
        project.clone(),
        history,
        templates,
        None, // prompt_store - we'll skip user rules for now
        fs.clone(),
        cx,
    )
    .await
    .context("Failed to create NativeAgent")?;

    log::info!("NativeAgent initialized");

    // Create the connection wrapper
    let connection = Rc::new(NativeAgentConnection(agent.clone()));

    // Check if we have any authenticated language model providers
    let has_models = cx.update(|cx| {
        let registry = LanguageModelRegistry::global(cx);
        registry.read(cx).available_models(cx).next().is_some()
    })?;

    if !has_models {
        log::warn!(
            "No language models available. Set an API key environment variable (e.g., ANTHROPIC_API_KEY, OPENAI_API_KEY) or configure a local provider."
        );
    }

    // Suppress unused variable warning
    let _ = prompt_builder;
    let _ = slash_command_registry;

    Ok(CrowContext {
        agent,
        connection,
        project,
        fs,
        cwd,
    })
}

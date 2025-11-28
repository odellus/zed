use acp_thread::AgentConnection;
use anyhow::{Context as _, Result};
use client::Client;
use fs::RealFs;
use gpui::{App, AppContext as _, AsyncApp, Entity, UpdateGlobal as _};
use language::LanguageRegistry;
use language_model::LanguageModelRegistry;
use node_runtime::NodeRuntime;
use project::Project;
use prompt_store::PromptBuilder;
use reqwest_client::ReqwestClient;
use settings::{Settings as _, SettingsStore};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use paths;

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

/// Initialize all the components needed to run the agent in headless mode
pub async fn initialize(cx: &mut AsyncApp) -> Result<CrowContext> {
    let cwd = std::env::current_dir().context("Failed to get current directory")?;

    // Load user settings from ~/.config/zed/settings.json
    let user_settings_content = std::fs::read_to_string(paths::settings_file())
        .ok()
        .unwrap_or_default();

    // Initialize settings store and basic infrastructure
    let (fs, client, user_store, languages, authenticate_tasks) = cx.update(|cx| {
        // Initialize tokio runtime for async operations
        gpui_tokio::init(cx);

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

        // Set up default model from agent settings (this is normally done by agent_ui)
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
        }

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

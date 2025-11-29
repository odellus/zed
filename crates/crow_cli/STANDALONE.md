# Crow Standalone: Forking Zed's Agent for Headless Distribution

This document analyzes what's needed to ship crow-cli as a standalone tool, independent of Zed's UI and auth ceremony.

## Goal

```bash
curl -fsSL https://crow-ai.dev/install.sh | bash
# or
cargo install crow-cli
```

Users run `crow-cli auth login` to set API keys, or use environment variables. No Zed config, no keychain UI, no GUI.

## Current State

crow-cli currently lives inside the Zed monorepo and depends on:
- Zed's credential provider (keychain + UI flow)
- Zed's settings system (`~/.config/zed/settings.json`)
- gpui (Zed's UI framework, used headlessly for async runtime)
- ~30 workspace crates

## The Auth Problem

### How Zed Does Auth Now

1. **Storage**: API keys stored in system keychain (macOS Keychain, Windows Credential Manager, Linux Secret Service)
2. **UI Flow**: `ConfigurationView` in each provider shows input field, user pastes key, saved to keychain
3. **Env Var Fallback**: Each provider checks env var first (e.g., `ANTHROPIC_API_KEY`)
4. **Development Mode**: Dev builds use `~/.config/zed/development_credentials` JSON file

### What We Want: The Opencode Pattern

Opencode uses a simple, elegant auth system we should copy:

**XDG Directory Structure:**
```
~/.local/share/crow/           # XDG_DATA_HOME/crow
├── auth.json                  # API keys and base URLs
├── sessions/                  # Session storage
└── log/                       # Logs

~/.config/crow/                # XDG_CONFIG_HOME/crow  
└── config.json                # User preferences

~/.cache/crow/                 # XDG_CACHE_HOME/crow
└── models.json                # Cached model list
```

**auth.json Format:**
```json
{
  "anthropic": {
    "type": "api",
    "key": "sk-ant-api03-...",
    "api": "https://api.anthropic.com"
  },
  "openai": {
    "type": "api",
    "key": "sk-...",
    "api": "https://api.openai.com/v1"
  },
  "ollama": {
    "type": "api",
    "key": "",
    "api": "http://localhost:11434"
  }
}
```

**Auth Types (from opencode):**
```rust
enum AuthInfo {
    // Simple API key auth (most providers)
    Api { key: String, api: Option<String> },
    
    // OAuth flow (GitHub Copilot, etc.)
    OAuth { 
        refresh: String, 
        access: String, 
        expires: u64,
        enterprise_url: Option<String>,
    },
    
    // Well-known auth (custom providers)
    WellKnown { key: String, token: String },
}
```

**Priority Order:**
1. Environment variable (`ANTHROPIC_API_KEY`, etc.)
2. auth.json file
3. Error: "No API key configured"

### CLI Commands

```bash
# List configured providers
crow-cli auth list
# Output:
# Credentials ~/.local/share/crow/auth.json
#   anthropic api
#   openai api
# Environment
#   google GEMINI_API_KEY

# Add a provider
crow-cli auth login
# Interactive prompt to select provider and enter API key

crow-cli auth login anthropic
# Direct login for specific provider

# Remove a provider  
crow-cli auth logout
# Interactive prompt to select provider to remove

crow-cli auth logout anthropic
# Direct logout for specific provider
```

## Reference: Opencode Implementation

### XDG Paths (opencode/src/global/index.ts)
```typescript
import { xdgData, xdgCache, xdgConfig, xdgState } from "xdg-basedir"

const app = "opencode"
const data = path.join(xdgData!, app)      // ~/.local/share/opencode
const cache = path.join(xdgCache!, app)    // ~/.cache/opencode
const config = path.join(xdgConfig!, app)  // ~/.config/opencode
const state = path.join(xdgState!, app)    // ~/.local/state/opencode
```

### Auth Module (opencode/src/auth/index.ts)
```typescript
const filepath = path.join(Global.Path.data, "auth.json")

async function get(providerID: string): Promise<AuthInfo | undefined> {
  const file = Bun.file(filepath)
  return file.json()
    .catch(() => ({}))
    .then((x) => x[providerID])
}

async function all(): Promise<Record<string, AuthInfo>> {
  const file = Bun.file(filepath)
  return file.json().catch(() => ({}))
}

async function set(key: string, info: AuthInfo) {
  const file = Bun.file(filepath)
  const data = await all()
  await Bun.write(file, JSON.stringify({ ...data, [key]: info }, null, 2))
  await fs.chmod(file.name!, 0o600)  // Secure permissions
}

async function remove(key: string) {
  const file = Bun.file(filepath)
  const data = await all()
  delete data[key]
  await Bun.write(file, JSON.stringify(data, null, 2))
}
```

### Provider Loading (opencode/src/provider/provider.ts)
```typescript
// Priority 1: Environment variables
for (const [providerID, provider] of Object.entries(database)) {
  const apiKey = provider.env.map((item) => process.env[item]).at(0)
  if (!apiKey) continue
  mergeProvider(providerID, { apiKey }, "env")
}

// Priority 2: auth.json file
for (const [providerID, provider] of Object.entries(await Auth.all())) {
  if (provider.type === "api") {
    mergeProvider(providerID, { apiKey: provider.key }, "api")
  }
}

// Merge also applies baseURL from provider.api field
function mergeProvider(id, options, source) {
  const info = database[id]
  if (info.api && !options["baseURL"]) {
    options["baseURL"] = info.api
  }
  // ...
}
```

## Rust Implementation Plan

### Phase 1: XDG Paths Module (0.5 days)

Create `crates/crow_cli/src/paths.rs`:

```rust
use std::path::PathBuf;

pub struct CrowPaths {
    pub data: PathBuf,      // ~/.local/share/crow
    pub config: PathBuf,    // ~/.config/crow  
    pub cache: PathBuf,     // ~/.cache/crow
    pub state: PathBuf,     // ~/.local/state/crow
}

impl CrowPaths {
    pub fn new() -> Self {
        let data = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("~/.local/share"))
            .join("crow");
        let config = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("crow");
        let cache = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("~/.cache"))
            .join("crow");
        let state = dirs::state_dir()
            .unwrap_or_else(|| PathBuf::from("~/.local/state"))
            .join("crow");
        
        Self { data, config, cache, state }
    }
    
    pub fn auth_file(&self) -> PathBuf {
        self.data.join("auth.json")
    }
    
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.data)?;
        std::fs::create_dir_all(&self.config)?;
        std::fs::create_dir_all(&self.cache)?;
        std::fs::create_dir_all(&self.state)?;
        Ok(())
    }
}
```

### Phase 2: Auth Module (1 day)

Create `crates/crow_cli/src/auth.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AuthInfo {
    Api {
        key: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        api: Option<String>,
    },
    OAuth {
        refresh: String,
        access: String,
        expires: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        enterprise_url: Option<String>,
    },
    WellKnown {
        key: String,
        token: String,
    },
}

pub struct Auth {
    path: PathBuf,
}

impl Auth {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
    
    pub async fn get(&self, provider_id: &str) -> Option<AuthInfo> {
        let data = self.all().await;
        data.get(provider_id).cloned()
    }
    
    pub async fn all(&self) -> HashMap<String, AuthInfo> {
        let content = tokio::fs::read_to_string(&self.path).await.ok()?;
        serde_json::from_str(&content).unwrap_or_default()
    }
    
    pub async fn set(&self, provider_id: &str, info: AuthInfo) -> anyhow::Result<()> {
        let mut data = self.all().await;
        data.insert(provider_id.to_string(), info);
        
        let json = serde_json::to_string_pretty(&data)?;
        tokio::fs::write(&self.path, &json).await?;
        
        // Set secure permissions (0600)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&self.path, perms)?;
        }
        
        Ok(())
    }
    
    pub async fn remove(&self, provider_id: &str) -> anyhow::Result<()> {
        let mut data = self.all().await;
        data.remove(provider_id);
        
        let json = serde_json::to_string_pretty(&data)?;
        tokio::fs::write(&self.path, &json).await?;
        Ok(())
    }
}
```

### Phase 3: Credentials Provider (1 day)

Create `crates/crow_cli/src/credentials.rs`:

```rust
use crate::auth::{Auth, AuthInfo};
use crate::paths::CrowPaths;

/// Provider info with env var names and default API URL
struct ProviderInfo {
    env_vars: &'static [&'static str],
    default_api: &'static str,
}

const PROVIDERS: &[(&str, ProviderInfo)] = &[
    ("anthropic", ProviderInfo {
        env_vars: &["ANTHROPIC_API_KEY"],
        default_api: "https://api.anthropic.com",
    }),
    ("openai", ProviderInfo {
        env_vars: &["OPENAI_API_KEY"],
        default_api: "https://api.openai.com/v1",
    }),
    ("google", ProviderInfo {
        env_vars: &["GEMINI_API_KEY", "GOOGLE_AI_API_KEY"],
        default_api: "https://generativelanguage.googleapis.com",
    }),
    // ... etc
];

pub struct CrowCredentialsProvider {
    auth: Auth,
}

impl CrowCredentialsProvider {
    pub fn new(paths: &CrowPaths) -> Self {
        Self {
            auth: Auth::new(paths.auth_file()),
        }
    }
    
    /// Get API key for a provider. Priority: env var > auth.json
    pub async fn get_api_key(&self, provider_id: &str) -> Option<String> {
        // Check env vars first
        if let Some(info) = PROVIDERS.iter().find(|(id, _)| *id == provider_id) {
            for env_var in info.1.env_vars {
                if let Ok(key) = std::env::var(env_var) {
                    return Some(key);
                }
            }
        }
        
        // Fall back to auth.json
        match self.auth.get(provider_id).await? {
            AuthInfo::Api { key, .. } => Some(key),
            AuthInfo::OAuth { access, .. } => Some(access),
            AuthInfo::WellKnown { token, .. } => Some(token),
        }
    }
    
    /// Get API URL for a provider. Priority: auth.json > default
    pub async fn get_api_url(&self, provider_id: &str) -> Option<String> {
        // Check auth.json for custom URL
        if let Some(AuthInfo::Api { api: Some(url), .. }) = self.auth.get(provider_id).await {
            return Some(url);
        }
        
        // Fall back to default
        PROVIDERS.iter()
            .find(|(id, _)| *id == provider_id)
            .map(|(_, info)| info.default_api.to_string())
    }
}
```

### Phase 4: Auth CLI Commands (0.5 days)

Add to `crates/crow_cli/src/crow_cli.rs`:

```rust
#[derive(Subcommand)]
enum AuthCommands {
    /// List configured providers
    List,
    
    /// Log in to a provider
    Login {
        /// Provider ID (anthropic, openai, etc.)
        provider: Option<String>,
    },
    
    /// Log out from a provider
    Logout {
        /// Provider ID
        provider: Option<String>,
    },
}
```

Create `crates/crow_cli/src/commands/auth.rs`:

```rust
pub async fn run_auth_list() -> Result<()> {
    let paths = CrowPaths::new();
    let auth = Auth::new(paths.auth_file());
    
    println!("Credentials {}", paths.auth_file().display());
    
    for (provider_id, info) in auth.all().await {
        let type_str = match info {
            AuthInfo::Api { .. } => "api",
            AuthInfo::OAuth { .. } => "oauth", 
            AuthInfo::WellKnown { .. } => "wellknown",
        };
        println!("  {} {}", provider_id, type_str);
    }
    
    // Also show env vars
    println!("\nEnvironment");
    for (provider_id, info) in PROVIDERS {
        for env_var in info.env_vars {
            if std::env::var(env_var).is_ok() {
                println!("  {} {}", provider_id, env_var);
            }
        }
    }
    
    Ok(())
}

pub async fn run_auth_login(provider: Option<String>) -> Result<()> {
    let paths = CrowPaths::new();
    paths.ensure_dirs()?;
    let auth = Auth::new(paths.auth_file());
    
    let provider_id = match provider {
        Some(p) => p,
        None => {
            // Interactive provider selection
            prompt_select_provider()?
        }
    };
    
    // Prompt for API key
    let key = rpassword::prompt_password("Enter your API key: ")?;
    
    // Optionally prompt for custom API URL
    let api_url = prompt_optional("Custom API URL (leave empty for default): ")?;
    
    auth.set(&provider_id, AuthInfo::Api {
        key,
        api: if api_url.is_empty() { None } else { Some(api_url) },
    }).await?;
    
    println!("Logged in to {}", provider_id);
    Ok(())
}
```

### Phase 5: Integration with Zed's Language Models (1 day)

Modify how `language_models` crate loads credentials. Create a custom `CredentialsProvider` that uses our auth system:

```rust
// In crow_cli/src/init.rs
impl CredentialsProvider for CrowCredentialsProvider {
    fn read_credentials<'a>(
        &'a self,
        url: &'a str,
        _cx: &'a AsyncApp,
    ) -> Pin<Box<dyn Future<Output = Result<Option<(String, Vec<u8>)>>> + 'a>> {
        Box::pin(async move {
            // Map URL to provider ID
            let provider_id = url_to_provider_id(url);
            
            if let Some(key) = self.get_api_key(&provider_id).await {
                Ok(Some(("Bearer".to_string(), key.into_bytes())))
            } else {
                Ok(None)
            }
        })
    }
    
    fn write_credentials<'a>(&'a self, ...) -> ... {
        // Delegate to auth.set()
    }
    
    fn delete_credentials<'a>(&'a self, ...) -> ... {
        // Delegate to auth.remove()
    }
}
```

## The GPUI Problem

### Why GPUI is Used

gpui provides:
1. **`Application::headless()`** - Async runtime wrapper
2. **`Entity<T>`** - ECS-like entity management
3. **`AsyncApp`** - Task spawning context
4. **`AppContext`** - Global state registration

crow-cli uses `Application::headless().run(|cx| { ... })` which does NOT create a window. It's just the async runtime.

### Can We Remove GPUI?

**Short answer: Not easily.**

The dependency chain:
```
crow_cli
├── agent → gpui (Entity<NativeAgent>)
├── acp_thread → gpui (Entity<AcpThread>)
├── language_models → gpui (for settings observation)
├── client → gpui (for AppContext)
└── project → gpui (Entity<Project>)
```

Removing gpui would require:
- Replacing `Entity<T>` with `Arc<Mutex<T>>` everywhere
- Replacing `AsyncApp` with raw tokio
- Rewriting global state management
- Touching 10+ crates

**Not worth it.** gpui compiles once and caches. The headless runtime is lightweight.

### What We CAN Remove

| Crate | Status | Notes |
|-------|--------|-------|
| `ui` | Removable | Just re-exports `gpui::App` |
| `theme` | Mostly removable | Replace `theme::init()` with no-op |
| `editor` | Keep | Used by project/language for buffer types |
| `terminal` | Keep | Used by project for terminal context |
| `agent_ui` | Not in crow_cli | Already excluded |

## Provider Environment Variables

Supported (matching opencode):

| Provider | Env Var(s) |
|----------|------------|
| Anthropic | `ANTHROPIC_API_KEY` |
| OpenAI | `OPENAI_API_KEY` |
| Google | `GEMINI_API_KEY`, `GOOGLE_AI_API_KEY` |
| Mistral | `MISTRAL_API_KEY` |
| DeepSeek | `DEEPSEEK_API_KEY` |
| X.AI | `XAI_API_KEY` |
| OpenRouter | `OPENROUTER_API_KEY` |
| Ollama | (no key needed, just set `api` in auth.json) |
| Azure | `AZURE_OPENAI_API_KEY` |
| AWS Bedrock | `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_BEARER_TOKEN_BEDROCK` |

## Implementation Summary

| Phase | Task | Effort | Files |
|-------|------|--------|-------|
| 1 | XDG paths module | 0.5 days | `src/paths.rs` |
| 2 | Auth module (auth.json R/W) | 1 day | `src/auth.rs` |
| 3 | Credentials provider | 1 day | `src/credentials.rs` |
| 4 | Auth CLI commands | 0.5 days | `src/commands/auth.rs` |
| 5 | Integration with language_models | 1 day | `src/init.rs` |
| 6 | Remove UI deps | 0.5 days | `Cargo.toml`, `src/init.rs` |
| 7 | Distribution | 0.5 days | CI, install script |

**Total: ~5 days to a shippable standalone crow-cli.**

## Risks & Mitigations

### Risk: Breaking Zed Integration

**Mitigation:** Use feature flags. Default to standalone mode for `crow-cli` binary, Zed integration for full Zed build.

### Risk: GPUI Compile Times

**Mitigation:** GPUI caches well. First build is slow (~2 min), subsequent builds are fast. Consider providing prebuilt binaries.

### Risk: Missing Zed Features

**Mitigation:** Document what works standalone vs integrated:
- Standalone: Chat, sessions, traces, prompts, auto mode
- Zed-only: Git integration, LSP, file previews (need Zed UI)

## File Structure After Implementation

```
crates/crow_cli/
├── src/
│   ├── crow_cli.rs      # CLI entry point
│   ├── init.rs          # Initialization
│   ├── paths.rs         # NEW: XDG directory paths
│   ├── auth.rs          # NEW: auth.json management
│   ├── credentials.rs   # NEW: CredentialsProvider impl
│   ├── commands/
│   │   ├── mod.rs
│   │   ├── auth.rs      # NEW: auth login/logout/list
│   │   ├── chat.rs
│   │   ├── repl.rs
│   │   ├── sessions.rs
│   │   └── telemetry.rs
│   └── render/
├── Cargo.toml
├── README.md
├── AGENTS.md
├── STANDALONE.md        # This file
└── TEST_AGENTS.md
```

//! Login command - fetches credentials from system keyring and stores them
//! in the file-based development credentials store for subsequent use.

use anyhow::{Context, Result};
use colored::Colorize;
use gpui::AsyncApp;
use std::collections::HashMap;
use std::path::PathBuf;

/// Known provider URLs that store API keys in the keyring
const PROVIDER_URLS: &[(&str, &str)] = &[
    ("Anthropic", "https://api.anthropic.com"),
    ("OpenAI", "https://api.openai.com"),
    ("Google AI", "https://generativelanguage.googleapis.com"),
    ("OpenRouter", "https://openrouter.ai/api"),
    ("Mistral", "https://api.mistral.ai"),
    ("DeepSeek", "https://api.deepseek.com"),
    ("X.AI", "https://api.x.ai"),
    ("Zed", "https://zed.dev"),
];

fn dev_credentials_path() -> PathBuf {
    paths::config_dir().join("development_credentials")
}

fn load_dev_credentials() -> HashMap<String, (String, Vec<u8>)> {
    let path = dev_credentials_path();
    if let Ok(json) = std::fs::read(&path) {
        serde_json::from_slice(&json).unwrap_or_default()
    } else {
        HashMap::new()
    }
}

fn save_dev_credentials(credentials: &HashMap<String, (String, Vec<u8>)>) -> Result<()> {
    let path = dev_credentials_path();
    let json = serde_json::to_string_pretty(credentials)?;
    std::fs::write(&path, json)?;
    Ok(())
}

/// Run the login command - fetches credentials from keyring and saves to file store
pub async fn run_login_command(cx: &mut AsyncApp) -> Result<()> {
    println!(
        "{}",
        "Fetching API keys from system keyring...".cyan().bold()
    );
    println!(
        "{}",
        "You may be prompted for your system password.".dimmed()
    );
    println!();

    // We need to use the real keyring, not the dev credentials provider
    // So we'll read directly using gpui's credential APIs
    let mut found_any = false;
    let mut credentials = load_dev_credentials();

    for (provider_name, url) in PROVIDER_URLS {
        print!("  {} {}... ", "Checking".dimmed(), provider_name);
        std::io::Write::flush(&mut std::io::stdout())?;

        let result = cx
            .update(|cx| cx.read_credentials(url))?
            .await;

        match result {
            Ok(Some((username, key_bytes))) => {
                credentials.insert(url.to_string(), (username, key_bytes));
                found_any = true;
                println!("{}", "found".green());
            }
            Ok(None) => {
                println!("{}", "not set".dimmed());
            }
            Err(e) => {
                println!("{} ({})", "error".red(), e);
            }
        }
    }

    if found_any {
        save_dev_credentials(&credentials)?;
        println!();
        println!(
            "{} {}",
            "Saved to:".green(),
            dev_credentials_path().display()
        );
        println!(
            "{}",
            "Subsequent crow-cli commands will use these credentials.".dimmed()
        );
    } else {
        println!();
        println!(
            "{}",
            "No API keys found in keyring. Set them in Zed's settings or via environment variables."
                .yellow()
        );
    }

    Ok(())
}

/// Show current credential status
pub async fn run_status_command(cx: &mut AsyncApp) -> Result<()> {
    println!("{}", "Credential Status".cyan().bold());
    println!();

    // Check file-based credentials
    let dev_creds = load_dev_credentials();
    println!("{}", "File-based credentials (used by crow-cli):".white());
    if dev_creds.is_empty() {
        println!("  {}", "None stored. Run 'crow-cli login' to import from keyring.".dimmed());
    } else {
        for (url, _) in &dev_creds {
            let provider_name = PROVIDER_URLS
                .iter()
                .find(|(_, u)| *u == url)
                .map(|(n, _)| *n)
                .unwrap_or("Unknown");
            println!("  {} {}", "●".green(), provider_name);
        }
    }

    println!();

    // Check environment variables
    println!("{}", "Environment variables:".white());
    let env_vars = [
        ("ANTHROPIC_API_KEY", "Anthropic"),
        ("OPENAI_API_KEY", "OpenAI"),
        ("GOOGLE_AI_API_KEY", "Google AI"),
        ("OPENROUTER_API_KEY", "OpenRouter"),
        ("MISTRAL_API_KEY", "Mistral"),
        ("DEEPSEEK_API_KEY", "DeepSeek"),
        ("XAI_API_KEY", "X.AI"),
    ];

    let mut found_env = false;
    for (var, provider) in env_vars {
        if std::env::var(var).is_ok() {
            println!("  {} {} ({})", "●".green(), provider, var);
            found_env = true;
        }
    }
    if !found_env {
        println!("  {}", "None set".dimmed());
    }

    println!();
    println!(
        "{}",
        "Note: Environment variables take precedence over stored credentials.".dimmed()
    );

    Ok(())
}

/// Clear stored credentials
pub async fn run_logout_command(_cx: &mut AsyncApp) -> Result<()> {
    let path = dev_credentials_path();
    if path.exists() {
        std::fs::remove_file(&path)?;
        println!("{}", "Cleared stored credentials.".green());
    } else {
        println!("{}", "No stored credentials to clear.".dimmed());
    }
    Ok(())
}

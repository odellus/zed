use anyhow::Result;
use colored::Colorize;
use gpui::AsyncApp;

use crate::init;

/// List all saved sessions
pub async fn run_list_sessions_command(cx: &mut AsyncApp) -> Result<()> {
    log::info!("Listing sessions");

    // Initialize (needed to access the database)
    let _crow = init::initialize(cx).await?;

    // TODO: Access to history store requires adding a public API to NativeAgent
    // For now, just show a placeholder message
    println!("{}", "Session listing not yet implemented.".yellow());
    println!(
        "{}",
        "Use the Zed editor to view and manage sessions.".dimmed()
    );

    Ok(())
}

/// Create a new session
pub async fn run_new_session_command(title: Option<String>, cx: &mut AsyncApp) -> Result<()> {
    log::info!("Creating new session");

    // Initialize
    let crow = init::initialize(cx).await?;

    // Create a new thread
    let acp_thread = cx.update(|cx| crow.new_thread(cx))?.await?;

    // Set title if provided
    if let Some(title) = title {
        acp_thread
            .update(cx, |thread, cx| thread.set_title(title.into(), cx))?
            .await?;
    }

    let session_id = acp_thread.read_with(cx, |thread, _| thread.session_id().clone())?;

    println!("{} {}", "Created session:".green(), session_id.0);

    Ok(())
}

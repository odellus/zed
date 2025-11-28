mod terminal;

pub use terminal::*;

/// Output mode for CLI commands
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// Full streaming output with decorations
    Verbose,
    /// Only final response, no streaming decorations
    Quiet,
    /// JSON output
    Json,
}

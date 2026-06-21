/// Terminal pager integration.
///
/// Detects whether output will overflow the terminal and, if so, pipes
/// through `$PAGER` (default: `less -RFX`).
use std::io::{self, Write};
use std::process::{Child, Command, Stdio};

use crossterm::terminal;
use tracing::debug;

/// Default pager command used when `$PAGER` is not set.
///
/// Windows ships `more` as a built-in; Unix systems have `less`.
#[cfg(target_os = "windows")]
const DEFAULT_PAGER: &str = "more";
#[cfg(not(target_os = "windows"))]
const DEFAULT_PAGER: &str = "less -RFX";

/// Controls whether and how output is paged.
#[derive(Debug, Clone)]
pub struct Pager {
    /// The pager command to invoke (e.g. `"less -RFX"`).
    pub command: String,
    /// When `true`, the pager is completely bypassed.
    pub disabled: bool,
}

impl Default for Pager {
    fn default() -> Self {
        let command = std::env::var("PAGER").unwrap_or_else(|_| DEFAULT_PAGER.to_string());
        Self {
            command,
            disabled: false,
        }
    }
}

impl Pager {
    /// Create a `Pager` that is entirely disabled (output goes straight to stdout).
    pub fn disabled() -> Self {
        Self {
            command: String::new(),
            disabled: true,
        }
    }

    /// Create a `Pager` with an explicit command override.
    pub fn with_command(cmd: &str) -> Self {
        Self {
            command: cmd.to_string(),
            disabled: false,
        }
    }

    /// Write `content` to the pager if it would overflow the terminal height,
    /// or directly to stdout otherwise.
    ///
    /// When paging is disabled, always writes to stdout directly.
    ///
    /// # Errors
    ///
    /// Returns `std::io::Error` if writing fails or the pager cannot be spawned.
    pub fn print(&self, content: &str) -> io::Result<()> {
        if self.disabled || !should_page(content) {
            print!("{content}");
            io::stdout().flush()?;
            return Ok(());
        }

        match self.spawn_pager() {
            Ok(mut child) => {
                if let Some(stdin) = child.stdin.as_mut() {
                    stdin.write_all(content.as_bytes())?;
                }
                let _ = child.wait();
                Ok(())
            }
            Err(e) => {
                // Fall back to stdout if pager fails to spawn.
                debug!("Pager spawn failed ({e}); falling back to stdout");
                print!("{content}");
                io::stdout().flush()
            }
        }
    }

    /// Spawn the pager process with a piped stdin.
    fn spawn_pager(&self) -> io::Result<Child> {
        let parts: Vec<&str> = self.command.split_whitespace().collect();
        let (cmd, args) = parts.split_first().unwrap_or((&"less", &[]));
        Command::new(cmd).args(args).stdin(Stdio::piped()).spawn()
    }
}

/// Returns `true` if `content` has more lines than the current terminal height.
fn should_page(content: &str) -> bool {
    let line_count = content.lines().count();
    match terminal::size() {
        Ok((_, rows)) => line_count > rows as usize,
        Err(_) => false, // not a TTY-never page
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pager_disabled_flag() {
        let p = Pager::disabled();
        assert!(p.disabled);
    }

    #[test]
    fn pager_default_has_command() {
        let p = Pager::default();
        assert!(!p.command.is_empty());
    }

    #[test]
    fn pager_with_command() {
        let p = Pager::with_command("more");
        assert_eq!(p.command, "more");
        assert!(!p.disabled);
    }
}

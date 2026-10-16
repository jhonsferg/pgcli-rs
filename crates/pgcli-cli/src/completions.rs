/// Shell completion script generation for pgcli-rs.
///
/// Generates completion scripts for bash, zsh, fish, and PowerShell
/// using clap's built-in completion infrastructure.
use clap::CommandFactory;
use clap_complete::{generate, Shell};
use std::io;

use crate::args::CliArgs;

/// Generate a shell completion script and write it to `writer`.
///
/// # Errors
///
/// Returns an I/O error if writing to `writer` fails.
pub fn generate_completions(shell: Shell, writer: &mut dyn io::Write) -> io::Result<()> {
    let mut cmd = CliArgs::command();
    generate(shell, &mut cmd, "pgcli", writer);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap_complete::Shell;

    #[test]
    fn generate_bash_completions_does_not_panic() {
        let mut buf = Vec::new();
        generate_completions(Shell::Bash, &mut buf).expect("generation failed");
        assert!(!buf.is_empty());
    }

    #[test]
    fn completions_target_the_pgcli_binary() {
        let mut buf = Vec::new();
        generate_completions(Shell::Bash, &mut buf).expect("generation failed");
        let script = String::from_utf8(buf).expect("utf-8");
        assert!(script.contains("complete -F _pgcli"), "{script}");
        assert!(!script.contains("pgcli-rs"), "must not target pgcli-rs");
    }

    #[test]
    fn generate_zsh_completions_does_not_panic() {
        let mut buf = Vec::new();
        generate_completions(Shell::Zsh, &mut buf).expect("generation failed");
        assert!(!buf.is_empty());
    }
}

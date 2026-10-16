//! `\!`: runs a shell command and captures its output.

/// Execute a shell command and return its combined stdout+stderr as a string.
pub fn run_shell(cmd_str: &str) -> String {
    if cmd_str.trim().is_empty() {
        return String::new();
    }
    #[cfg(unix)]
    let result = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd_str)
        .output();
    #[cfg(windows)]
    let result = std::process::Command::new("cmd")
        .args(["/C", cmd_str])
        .output();
    match result {
        Ok(out) => {
            let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
            let err = String::from_utf8_lossy(&out.stderr);
            if !err.is_empty() {
                s.push_str(&err);
            }
            s
        }
        Err(e) => format!("ERROR: shell exec failed: {e}\n"),
    }
}

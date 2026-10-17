//! `--upgrade`: replaces the running pgcli binary with the latest GitHub release.
//!
//! Flow: query the latest release, compare versions (semver), download
//! `checksums.txt` and the archive for this platform, verify the archive's
//! SHA-256, and only then extract the binary and swap it in place. With
//! `--check` it only queries the latest release. A missing or mismatching
//! checksum leaves the installed binary untouched.
//!
//! Environment variables: `PGCLI_REPO` (default `jhonsferg/pgcli-rs`), and
//! `PGCLI_TEST_API_BASE` / `PGCLI_TEST_DL_BASE` to point at a local server in
//! tests. Plain `http://` is only accepted for loopback hosts.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use pgcli_core::error::{PgCliError, Result};
use semver::Version;
use sha2::{Digest, Sha256};

/// Repository the releases are published from.
const DEFAULT_REPO: &str = "jhonsferg/pgcli-rs";
const DEFAULT_API_BASE: &str = "https://api.github.com";
const DEFAULT_DL_BASE: &str = "https://github.com";
const CHECKSUMS_FILE: &str = "checksums.txt";
/// Largest download accepted (the archives are a few MiB).
const MAX_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;
/// Network attempts per request, with 1 s, 2 s back-off between them.
const ATTEMPTS: u32 = 3;
/// Maximum depth when searching for the binary inside the extracted archive.
const MAX_SEARCH_DEPTH: u8 = 3;

fn err(message: impl Into<String>) -> PgCliError {
    PgCliError::Upgrade(message.into())
}

/// Runs `--upgrade`. With `check` it only reports whether a newer release
/// exists. Blocking: call it outside the async runtime's worker threads.
///
/// # Errors
///
/// Returns `PgCliError::Upgrade` when the release cannot be found or
/// downloaded, the platform has no published binary, the checksum is missing
/// or does not match, or the binary cannot be replaced. In every case the
/// installed binary is left as it was.
pub fn run(check: bool) -> Result<()> {
    let current = current_version()?;
    let repo = repo()?;
    let api_base = base_url("PGCLI_TEST_API_BASE", DEFAULT_API_BASE)?;
    let dl_base = base_url("PGCLI_TEST_DL_BASE", DEFAULT_DL_BASE)?;
    let agent = agent();

    eprintln!("Checking for updates...");
    let (tag, latest) = latest_release(&agent, &api_base, &repo)?;

    if latest <= current {
        println!("pgcli {current} is already up to date.");
        return Ok(());
    }
    if check {
        println!("pgcli {current} -> {latest} is available. Run `pgcli --upgrade` to install it.");
        return Ok(());
    }

    let archive = artifact_name(
        std::env::consts::OS,
        std::env::consts::ARCH,
        cfg!(target_endian = "little"),
    )?;
    let exe =
        std::env::current_exe().map_err(|e| err(format!("cannot locate the pgcli binary: {e}")))?;

    let base = format!("{dl_base}/{repo}/releases/download/{tag}");
    let sums = get_bytes(&agent, &format!("{base}/{CHECKSUMS_FILE}")).map_err(|e| {
        err(format!(
            "{e}; the release must publish {CHECKSUMS_FILE}, nothing was changed"
        ))
    })?;
    let sums = String::from_utf8_lossy(&sums);
    let expected = parse_checksums(&sums, archive).ok_or_else(|| {
        err(format!(
            "{CHECKSUMS_FILE} has no SHA-256 for {archive}; refusing to upgrade, nothing was changed"
        ))
    })?;

    eprintln!("Downloading {archive}...");
    let data = get_bytes(&agent, &format!("{base}/{archive}"))?;
    let actual = sha256_hex(&data);
    if actual != expected {
        return Err(err(format!(
            "checksum mismatch for {archive}; nothing was changed\n  expected: {expected}\n  got:      {actual}"
        )));
    }
    eprintln!("Checksum verified.");

    let work = tempfile::tempdir()
        .map_err(|e| err(format!("cannot create a temporary directory: {e}")))?;
    let archive_path = work.path().join(archive);
    std::fs::write(&archive_path, &data)
        .map_err(|e| err(format!("cannot write {}: {e}", archive_path.display())))?;
    let extract_dir = work.path().join("extract");
    unpack(&archive_path, &extract_dir)?;

    let binary_name = if cfg!(windows) { "pgcli.exe" } else { "pgcli" };
    let new_binary = find_binary(&extract_dir, binary_name, MAX_SEARCH_DEPTH)
        .ok_or_else(|| err(format!("{archive} does not contain {binary_name}")))?;

    replace_binary_at(&new_binary, &exe)?;
    println!(
        "pgcli upgraded from {current} to {latest} at {}.",
        exe.display()
    );
    Ok(())
}

/// Removes the `pgcli.exe.old` a previous upgrade left on Windows, where the
/// running executable cannot be deleted. Does nothing on other platforms.
pub fn remove_stale_old() {
    #[cfg(windows)]
    {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::fs::remove_file(old_path(&exe));
        }
    }
}

/// Path the previous binary is moved to when the running one is replaced.
fn old_path(exe: &Path) -> PathBuf {
    let name = exe
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("pgcli.exe");
    exe.with_file_name(format!("{name}.old"))
}

/// Release archive for an OS and architecture (`std::env::consts` names),
/// following the release matrix in `release.yml`.
fn artifact_name(os: &str, arch: &str, little_endian: bool) -> Result<&'static str> {
    let name = match (os, arch) {
        ("linux", "x86_64") => "pgcli-rs_linux_x86_64.tar.gz",
        ("linux", "aarch64") => "pgcli-rs_linux_aarch64.tar.gz",
        ("linux", "arm") => "pgcli-rs_linux_armv7.tar.gz",
        ("linux", "x86") => "pgcli-rs_linux_386.tar.gz",
        ("linux", "riscv64") => "pgcli-rs_linux_riscv64.tar.gz",
        ("linux", "s390x") => "pgcli-rs_linux_s390x.tar.gz",
        ("linux", "powerpc64") if little_endian => "pgcli-rs_linux_ppc64le.tar.gz",
        ("android", "aarch64") => "pgcli-rs_android_aarch64.tar.gz",
        ("macos", "x86_64") => "pgcli-rs_darwin_x86_64.tar.gz",
        ("macos", "aarch64") => "pgcli-rs_darwin_aarch64.tar.gz",
        ("windows", "x86_64") => "pgcli-rs_windows_x86_64.zip",
        ("windows", "aarch64") => "pgcli-rs_windows_arm64.zip",
        _ => {
            return Err(err(format!(
                "no pgcli binary is published for {os}-{arch}; build it from source with cargo"
            )))
        }
    };
    Ok(name)
}

/// Version of the running binary.
fn current_version() -> Result<Version> {
    Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|e| err(format!("invalid package version: {e}")))
}

/// Repository `owner/name` from `PGCLI_REPO`, or the official one.
fn repo() -> Result<String> {
    let repo = std::env::var("PGCLI_REPO")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_REPO.to_string());
    validate_repo(&repo)?;
    Ok(repo)
}

/// Checks that `repo` has the form `owner/name` with URL-safe characters.
fn validate_repo(repo: &str) -> Result<()> {
    let parts: Vec<&str> = repo.split('/').collect();
    let safe = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if parts.len() == 2 && parts.iter().all(|p| safe(p)) {
        Ok(())
    } else {
        Err(err(format!(
            "PGCLI_REPO must look like owner/name, got '{repo}'"
        )))
    }
}

/// Base URL from `var`, or `default`, without a trailing slash.
fn base_url(var: &str, default: &str) -> Result<String> {
    let url = std::env::var(var)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim_end_matches('/').to_string())
        .unwrap_or_else(|| default.to_string());
    if is_allowed_url(&url) {
        Ok(url)
    } else {
        Err(err(format!(
            "{var} must use https:// (plain http:// is only accepted for localhost)"
        )))
    }
}

/// Only https, or plain http to a loopback host for local test servers.
fn is_allowed_url(url: &str) -> bool {
    if url.starts_with("https://") {
        return true;
    }
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let host = rest.split(['/', ':']).next().unwrap_or("");
    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(300)))
        .user_agent(concat!("pgcli/", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent()
}

/// Downloads `url` into memory, retrying transient failures.
fn get_bytes(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>> {
    let mut last = None;
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(1 << (attempt - 1)));
        }
        match agent.get(url).call() {
            Ok(mut response) => {
                let mut data = Vec::new();
                response
                    .body_mut()
                    .as_reader()
                    .take(MAX_DOWNLOAD_BYTES)
                    .read_to_end(&mut data)
                    .map_err(|e| err(format!("download of {url} interrupted: {e}")))?;
                return Ok(data);
            }
            // 4xx answers (missing release or asset) will not change on retry.
            Err(ureq::Error::StatusCode(code)) if (400..500).contains(&code) => {
                return Err(err(format!("{url} returned HTTP {code}")));
            }
            Err(e) => last = Some(e),
        }
    }
    Err(err(format!(
        "cannot download {url}: {}",
        last.map(|e| e.to_string()).unwrap_or_default()
    )))
}

/// Queries the latest release and returns its tag and version.
fn latest_release(agent: &ureq::Agent, api_base: &str, repo: &str) -> Result<(String, Version)> {
    let url = format!("{api_base}/repos/{repo}/releases/latest");
    let body = get_bytes(agent, &url).map_err(|e| {
        err(format!(
            "{e}; check your connection and that {repo} has a release"
        ))
    })?;
    let json: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| err(format!("invalid release index from GitHub: {e}")))?;
    let tag = json
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| err("the release index has no tag_name"))?;
    let version = Version::parse(tag.trim_start_matches('v'))
        .map_err(|e| err(format!("cannot parse release tag '{tag}': {e}")))?;
    Ok((tag.to_string(), version))
}

/// The SHA-256 listed for `file` in a `sha256sum`-style checksums file.
/// Requires exactly one well-formed entry.
fn parse_checksums(sums: &str, file: &str) -> Option<String> {
    let mut found = sums.lines().filter_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == file).then(|| hash.to_ascii_lowercase())
    });
    let hash = found.next()?;
    let well_formed = hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit());
    (found.next().is_none() && well_formed).then_some(hash)
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Extracts a `.tar.gz` or `.zip` archive into `dest`.
fn unpack(archive: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)
        .map_err(|e| err(format!("cannot create {}: {e}", dest.display())))?;
    let file = std::fs::File::open(archive)
        .map_err(|e| err(format!("cannot open {}: {e}", archive.display())))?;
    let name = archive.to_string_lossy();
    if name.ends_with(".zip") {
        zip::ZipArchive::new(file)
            .and_then(|mut zip| zip.extract(dest))
            .map_err(|e| err(format!("cannot extract {name}: {e}")))
    } else {
        tar::Archive::new(flate2::read::GzDecoder::new(file))
            .unpack(dest)
            .map_err(|e| err(format!("cannot extract {name}: {e}")))
    }
}

/// Replaces `exe` with `new_binary` without leaving it half-done.
///
/// The new binary is first copied next to the current one (same volume). On
/// Unix a `rename` puts it in place atomically. On Windows the running
/// executable cannot be overwritten: it is renamed to `<name>.old`, the new one
/// takes its place, and the old one is deleted now or on the next run.
fn replace_binary_at(new_binary: &Path, exe: &Path) -> Result<()> {
    let file_name = exe
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| err(format!("invalid binary path {}", exe.display())))?;
    let staged = exe.with_file_name(format!(".{file_name}.new-{}", std::process::id()));
    let hint = "check write permissions for the install directory";

    std::fs::copy(new_binary, &staged).map_err(|e| {
        err(format!(
            "cannot stage the new binary next to {}: {e} ({hint})",
            exe.display()
        ))
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if let Err(e) = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)) {
            let _ = std::fs::remove_file(&staged);
            return Err(err(format!("cannot make the new binary executable: {e}")));
        }
        if let Err(e) = std::fs::rename(&staged, exe) {
            let _ = std::fs::remove_file(&staged);
            return Err(err(format!(
                "cannot replace {}: {e} ({hint})",
                exe.display()
            )));
        }
    }

    #[cfg(windows)]
    {
        let old = old_path(exe);
        let _ = std::fs::remove_file(&old);
        if let Err(e) = std::fs::rename(exe, &old) {
            let _ = std::fs::remove_file(&staged);
            return Err(err(format!(
                "cannot move {} out of the way: {e} ({hint})",
                exe.display()
            )));
        }
        if let Err(e) = std::fs::rename(&staged, exe) {
            let _ = std::fs::rename(&old, exe);
            let _ = std::fs::remove_file(&staged);
            return Err(err(format!(
                "cannot place the new binary, previous version restored: {e}"
            )));
        }
        // The running process still holds the old file; if it cannot be
        // deleted now, the next run deletes it (see `remove_stale_old`).
        let _ = std::fs::remove_file(&old);
    }

    Ok(())
}

/// Searches for a regular file named `name` under `dir`, up to `depth` levels.
fn find_binary(dir: &Path, name: &str, depth: u8) -> Option<PathBuf> {
    let mut subdirs = Vec::new();
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_file() && entry.file_name().to_str() == Some(name) {
            return Some(entry.path());
        }
        if kind.is_dir() {
            subdirs.push(entry.path());
        }
    }
    if depth == 0 {
        return None;
    }
    subdirs
        .into_iter()
        .find_map(|sub| find_binary(&sub, name, depth - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_names_follow_the_release_matrix() {
        let cases = [
            ("linux", "x86_64", "pgcli-rs_linux_x86_64.tar.gz"),
            ("linux", "aarch64", "pgcli-rs_linux_aarch64.tar.gz"),
            ("linux", "arm", "pgcli-rs_linux_armv7.tar.gz"),
            ("linux", "x86", "pgcli-rs_linux_386.tar.gz"),
            ("linux", "riscv64", "pgcli-rs_linux_riscv64.tar.gz"),
            ("linux", "s390x", "pgcli-rs_linux_s390x.tar.gz"),
            ("linux", "powerpc64", "pgcli-rs_linux_ppc64le.tar.gz"),
            ("android", "aarch64", "pgcli-rs_android_aarch64.tar.gz"),
            ("macos", "x86_64", "pgcli-rs_darwin_x86_64.tar.gz"),
            ("macos", "aarch64", "pgcli-rs_darwin_aarch64.tar.gz"),
            ("windows", "x86_64", "pgcli-rs_windows_x86_64.zip"),
            ("windows", "aarch64", "pgcli-rs_windows_arm64.zip"),
        ];
        for (os, arch, expected) in cases {
            assert_eq!(
                artifact_name(os, arch, true).ok(),
                Some(expected),
                "{os}-{arch}"
            );
        }
    }

    #[test]
    fn unpublished_platforms_have_no_artifact() {
        assert!(artifact_name("linux", "powerpc64", false).is_err());
        assert!(artifact_name("windows", "x86", true).is_err());
        assert!(artifact_name("freebsd", "x86_64", true).is_err());
    }

    #[test]
    fn checksums_need_exactly_one_well_formed_entry() {
        let hash = "a".repeat(64);
        let sums = format!(
            "{hash}  pgcli-rs_linux_x86_64.tar.gz\n{}  other.zip\n",
            "b".repeat(64)
        );
        assert_eq!(
            parse_checksums(&sums, "pgcli-rs_linux_x86_64.tar.gz"),
            Some(hash.clone())
        );
        assert_eq!(parse_checksums(&sums, "missing.tar.gz"), None);

        let duplicated = format!("{hash}  x.zip\n{hash}  x.zip\n");
        assert_eq!(parse_checksums(&duplicated, "x.zip"), None);
        assert_eq!(parse_checksums("abc  x.zip\n", "x.zip"), None);
        // `sha256sum -b` marks binary mode with a leading `*`.
        assert_eq!(
            parse_checksums(&format!("{hash} *x.zip\n"), "x.zip"),
            Some(hash)
        );
    }

    #[test]
    fn sha256_matches_a_known_digest() {
        assert_eq!(
            sha256_hex(b"hello world"),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn only_https_or_loopback_http_is_allowed() {
        assert!(is_allowed_url("https://api.github.com"));
        assert!(is_allowed_url("http://127.0.0.1:8080"));
        assert!(is_allowed_url("http://localhost/x"));
        assert!(!is_allowed_url("http://example.com"));
        assert!(!is_allowed_url("http://127.0.0.1.evil.com"));
        assert!(!is_allowed_url("ftp://example.com"));
    }

    #[test]
    fn repo_must_be_owner_and_name() {
        assert!(validate_repo("jhonsferg/pgcli-rs").is_ok());
        assert!(validate_repo("a/b/c").is_err());
        assert!(validate_repo("../etc").is_err());
        assert!(validate_repo("owner/na me").is_err());
        assert!(validate_repo("owner/..").is_err());
    }

    #[test]
    fn current_version_is_valid_semver() {
        assert!(current_version().is_ok());
    }

    #[test]
    fn old_path_appends_the_old_suffix() {
        assert_eq!(
            old_path(Path::new("/opt/bin/pgcli.exe")),
            Path::new("/opt/bin/pgcli.exe.old")
        );
    }

    #[test]
    fn replace_swaps_contents_and_leaves_no_staged_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let exe = dir.path().join("pgcli-target");
        std::fs::write(&exe, b"old contents").expect("write old");
        let new_binary = dir.path().join("downloaded");
        std::fs::write(&new_binary, b"new contents").expect("write new");

        replace_binary_at(&new_binary, &exe).expect("replace");

        assert_eq!(std::fs::read(&exe).expect("read"), b"new contents");
        let leftovers = std::fs::read_dir(dir.path())
            .expect("list")
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".new-"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn find_binary_searches_nested_directories() {
        let dir = tempfile::tempdir().expect("temp dir");
        let nested = dir.path().join("release").join("bin");
        std::fs::create_dir_all(&nested).expect("mkdir");
        std::fs::write(nested.join("pgcli"), b"x").expect("write");

        assert_eq!(
            find_binary(dir.path(), "pgcli", 3),
            Some(nested.join("pgcli"))
        );
        assert_eq!(find_binary(dir.path(), "pgcli", 1), None);
        assert_eq!(find_binary(dir.path(), "pgcli.exe", 3), None);
    }
}

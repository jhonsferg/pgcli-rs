//! Tests for `pgcli upgrade` against a local HTTP server. No real network.
//!
//! Each test copies the pgcli test binary into a temporary directory and
//! upgrades that copy, never the cargo build output.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

use sha2::{Digest, Sha256};

const REPO: &str = "jhonsferg/pgcli-rs";

/// Release archive name for the host, or `None` where the release matrix has
/// no binary (the tests then skip).
fn host_archive() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("pgcli-rs_linux_x86_64.tar.gz"),
        ("linux", "aarch64") => Some("pgcli-rs_linux_aarch64.tar.gz"),
        ("macos", "x86_64") => Some("pgcli-rs_darwin_x86_64.tar.gz"),
        ("macos", "aarch64") => Some("pgcli-rs_darwin_aarch64.tar.gz"),
        ("windows", "x86_64") => Some("pgcli-rs_windows_x86_64.zip"),
        ("windows", "aarch64") => Some("pgcli-rs_windows_arm64.zip"),
        _ => None,
    }
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "pgcli.exe"
    } else {
        "pgcli"
    }
}

/// A release archive holding a "binary" with `content`.
fn release_archive(name: &str, content: &[u8]) -> Vec<u8> {
    if name.ends_with(".zip") {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            zip.start_file(binary_name(), zip::write::SimpleFileOptions::default())
                .expect("zip entry");
            zip.write_all(content).expect("zip write");
            zip.finish().expect("zip finish");
        }
        buf.into_inner()
    } else {
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, binary_name(), content)
            .expect("tar entry");
        tar.into_inner()
            .expect("tar finish")
            .finish()
            .expect("gzip finish")
    }
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Serves fixed responses by path on 127.0.0.1 until the test process ends.
fn serve(routes: HashMap<String, Vec<u8>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let routes = Arc::new(routes);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let routes = Arc::clone(&routes);
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                let mut request = String::new();
                if reader.read_line(&mut request).is_err() {
                    return;
                }
                // Drain the headers.
                let mut line = String::new();
                while reader.read_line(&mut line).map(|n| n > 2).unwrap_or(false) {
                    line.clear();
                }
                let path = request.split_whitespace().nth(1).unwrap_or("/");
                let mut stream = stream;
                match routes.get(path) {
                    Some(body) => {
                        let head = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(head.as_bytes());
                        let _ = stream.write_all(body);
                    }
                    None => {
                        let _ = stream.write_all(
                            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                    }
                }
            });
        }
    });
    base
}

/// Routes of a release `tag` whose archive is `archive` and whose
/// checksums file is `sums`.
fn release_routes(
    tag: &str,
    archive_name: &str,
    archive: Vec<u8>,
    sums: String,
) -> HashMap<String, Vec<u8>> {
    let dl = format!("/{REPO}/releases/download/{tag}");
    HashMap::from([
        (
            format!("/repos/{REPO}/releases/latest"),
            format!("{{\"tag_name\": \"{tag}\"}}").into_bytes(),
        ),
        (format!("{dl}/checksums.txt"), sums.into_bytes()),
        (format!("{dl}/{archive_name}"), archive),
    ])
}

/// Copies the pgcli test binary into `dir` and returns the copy.
fn copy_binary(dir: &Path) -> PathBuf {
    let dest = dir.join(binary_name());
    std::fs::copy(env!("CARGO_BIN_EXE_pgcli"), &dest).expect("copy binary");
    dest
}

fn run(bin: &Path, base: &str, args: &[&str]) -> Output {
    Command::new(bin)
        .args(args)
        .env("PGCLI_TEST_API_BASE", base)
        .env("PGCLI_TEST_DL_BASE", base)
        .env_remove("PGCLI_REPO")
        .output()
        .expect("run pgcli")
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn check_reports_a_newer_release_and_changes_nothing() {
    let Some(name) = host_archive() else { return };
    let archive = release_archive(name, b"new pgcli");
    let sums = format!("{}  {name}\n", sha256_hex(&archive));
    let base = serve(release_routes("v999.0.0", name, archive, sums));
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = copy_binary(dir.path());
    let before = std::fs::read(&bin).expect("read");

    let out = run(&bin, &base, &["upgrade", "--check"]);

    assert!(out.status.success(), "{out:?}");
    assert!(
        text(&out).contains("999.0.0 is available"),
        "{}",
        text(&out)
    );
    assert_eq!(std::fs::read(&bin).expect("read"), before);
}

#[test]
fn verified_release_replaces_the_binary() {
    let Some(name) = host_archive() else { return };
    let archive = release_archive(name, b"new pgcli build");
    let sums = format!("{}  {name}\n", sha256_hex(&archive));
    let base = serve(release_routes("v999.0.0", name, archive, sums));
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = copy_binary(dir.path());

    let out = run(&bin, &base, &["upgrade"]);

    assert!(out.status.success(), "{out:?}");
    assert!(text(&out).contains("upgraded"), "{}", text(&out));
    assert_eq!(std::fs::read(&bin).expect("read"), b"new pgcli build");
}

#[test]
fn checksum_mismatch_keeps_the_installed_binary() {
    let Some(name) = host_archive() else { return };
    let archive = release_archive(name, b"tampered build");
    let sums = format!("{}  {name}\n", "0".repeat(64));
    let base = serve(release_routes("v999.0.0", name, archive, sums));
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = copy_binary(dir.path());
    let before = std::fs::read(&bin).expect("read");

    let out = run(&bin, &base, &["upgrade"]);

    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(text(&out).contains("checksum mismatch"), "{}", text(&out));
    assert_eq!(std::fs::read(&bin).expect("read"), before);
}

#[test]
fn missing_checksum_entry_refuses_the_upgrade() {
    let Some(name) = host_archive() else { return };
    let archive = release_archive(name, b"unlisted build");
    let sums = format!("{}  some-other-file.tar.gz\n", sha256_hex(&archive));
    let base = serve(release_routes("v999.0.0", name, archive, sums));
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = copy_binary(dir.path());
    let before = std::fs::read(&bin).expect("read");

    let out = run(&bin, &base, &["upgrade"]);

    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(text(&out).contains("has no SHA-256"), "{}", text(&out));
    assert_eq!(std::fs::read(&bin).expect("read"), before);
}

#[test]
fn an_older_release_means_up_to_date() {
    let Some(name) = host_archive() else { return };
    let archive = release_archive(name, b"old build");
    let sums = format!("{}  {name}\n", sha256_hex(&archive));
    let base = serve(release_routes("v0.0.1", name, archive, sums));
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = copy_binary(dir.path());
    let before = std::fs::read(&bin).expect("read");

    let out = run(&bin, &base, &["upgrade"]);

    assert!(out.status.success(), "{out:?}");
    assert!(text(&out).contains("already up to date"), "{}", text(&out));
    assert_eq!(std::fs::read(&bin).expect("read"), before);
}

#[test]
fn upgrade_flags_do_not_exist_outside_the_subcommand() {
    let out = Command::new(env!("CARGO_BIN_EXE_pgcli"))
        .arg("--check")
        .output()
        .expect("run pgcli");
    assert_eq!(out.status.code(), Some(2), "{out:?}");
}

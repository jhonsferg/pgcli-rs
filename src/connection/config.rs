/// Connection configuration built from CLI args, environment variables,
/// and the `~/.pgpass` file.
use std::path::PathBuf;

use crate::cli::CliArgs;
use crate::connection::tls::TlsMode;
use crate::error::{PgCliError, Result};

/// All parameters needed to establish a PostgreSQL connection.
#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    /// Server hostname or IP address.
    pub host: String,
    /// Server port.
    pub port: u16,
    /// Database user.
    pub user: String,
    /// Database name.
    pub database: String,
    /// Password, if known. May be `None` if using trust auth or pgpass lookup.
    pub password: Option<String>,
    /// TLS connection mode.
    pub tls_mode: TlsMode,
    /// Path to client TLS certificate (mutual TLS).
    pub tls_cert: Option<PathBuf>,
    /// Path to client TLS private key (mutual TLS).
    pub tls_key: Option<PathBuf>,
    /// Path to CA certificate for server verification.
    pub tls_ca: Option<PathBuf>,
    /// Connection timeout in seconds.
    pub timeout_secs: u64,
    /// Application name sent to the server.
    pub application_name: String,
}

impl ConnectionConfig {
    /// Build a `ConnectionConfig` from parsed CLI arguments and environment variables.
    ///
    /// Precedence (highest to lowest):
    /// 1. Explicit CLI flags
    /// 2. `PGHOST`, `PGPORT`, `PGUSER`, `PGPASSWORD`, `PGDATABASE`, `PGSSLMODE`
    /// 3. Positional connection URI (if provided)
    /// 4. Defaults
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Config` if a URI positional argument fails to parse.
    pub fn from_cli_args(args: &CliArgs) -> Result<Self> {
        // Start with defaults, then merge URI if present, then explicit flags.
        let mut cfg = Self::defaults();

        // Track whether a URI supplied user/password so env vars don't clobber them.
        let mut uri_supplied_user = false;
        let mut uri_supplied_password = false;

        // If the positional arg looks like a URI, parse it first.
        if let Some(pos) = &args.positional_dbname {
            if pos.starts_with("postgresql://") || pos.starts_with("postgres://") {
                let uri_cfg = Self::from_uri(pos)?;
                uri_supplied_user = !uri_cfg.user.is_empty();
                uri_supplied_password = uri_cfg.password.is_some();
                cfg = cfg.merge(uri_cfg);
            } else {
                cfg.database = pos.clone();
            }
        }

        // Apply explicit CLI flags - they always win over URI and env vars.
        if let Some(h) = &args.host {
            cfg.host = h.clone();
        }
        if let Some(p) = args.port {
            cfg.port = p;
        }
        if let Some(u) = &args.username {
            cfg.user = u.clone();
        }
        if let Some(d) = &args.dbname {
            cfg.database = d.clone();
        }

        // Apply environment variables only when not already set by CLI flag or URI.
        if args.host.is_none() {
            if let Ok(v) = std::env::var("PGHOST") {
                cfg.host = v;
            }
        }
        if args.port.is_none() {
            if let Ok(v) = std::env::var("PGPORT") {
                cfg.port = v.parse::<u16>().map_err(|_| {
                    PgCliError::Config("PGPORT is not a valid port number".to_string())
                })?;
            }
        }
        if args.username.is_none() && !uri_supplied_user {
            if let Ok(v) = std::env::var("PGUSER") {
                cfg.user = v;
            } else if let Ok(v) = std::env::var("USER").or_else(|_| std::env::var("USERNAME")) {
                cfg.user = v;
            }
        }
        if args.dbname.is_none() && args.positional_dbname.is_none() {
            if let Ok(v) = std::env::var("PGDATABASE") {
                cfg.database = v;
            } else {
                cfg.database = cfg.user.clone();
            }
        }

        // Password resolution priority:
        // 1. PGPASSWORD env var (explicit override, even over URI)
        // 2. URI-supplied password (already in cfg.password from merge)
        // 3. ~/.pgpass file lookup
        if let Ok(p) = std::env::var("PGPASSWORD") {
            cfg.password = Some(p);
        } else if !uri_supplied_password && cfg.password.is_none() {
            cfg.password = read_pgpass(&cfg.host, cfg.port, &cfg.database, &cfg.user);
        }

        // TLS mode from PGSSLMODE or flags.
        cfg.tls_mode = resolve_tls_mode(args)?;
        cfg.tls_cert = args.tls_cert.clone();
        cfg.tls_key = args.tls_key.clone();
        cfg.tls_ca = args.tls_ca.clone();
        cfg.timeout_secs = args.timeout;

        Ok(cfg)
    }

    /// Parse a `postgresql://` or `postgres://` URI into a `ConnectionConfig`.
    ///
    /// Supports the standard URI format:
    /// `postgresql://[user[:password]@][host][:port][/dbname][?param=value]`
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Config` if the URI is malformed.
    pub fn from_uri(uri: &str) -> Result<Self> {
        let mut cfg = Self::defaults();

        let stripped = uri
            .strip_prefix("postgresql://")
            .or_else(|| uri.strip_prefix("postgres://"))
            .ok_or_else(|| PgCliError::Config(format!("invalid URI scheme: {uri}")))?;

        // Split query string.
        let (main, _query) = stripped.split_once('?').unwrap_or((stripped, ""));

        // Split userinfo from host/db.
        let (userinfo, hostdb) = if let Some(at) = main.rfind('@') {
            (&main[..at], &main[at + 1..])
        } else {
            ("", main)
        };

        // Parse user:password.
        if !userinfo.is_empty() {
            if let Some((u, p)) = userinfo.split_once(':') {
                cfg.user = percent_decode(u);
                cfg.password = Some(percent_decode(p));
            } else {
                cfg.user = percent_decode(userinfo);
            }
        }

        // Parse host:port/dbname.
        let (hostport, dbname) = if let Some(slash) = hostdb.find('/') {
            (&hostdb[..slash], &hostdb[slash + 1..])
        } else {
            (hostdb, "")
        };

        // Handle IPv6 bracket notation.
        if hostport.starts_with('[') {
            let end = hostport
                .find(']')
                .ok_or_else(|| PgCliError::Config("unclosed '[' in URI host".to_string()))?;
            cfg.host = hostport[1..end].to_string();
            let after = &hostport[end + 1..];
            if let Some(p) = after.strip_prefix(':') {
                cfg.port = p
                    .parse::<u16>()
                    .map_err(|_| PgCliError::Config(format!("invalid port in URI: {p}")))?;
            }
        } else if let Some((h, p)) = hostport.split_once(':') {
            cfg.host = h.to_string();
            cfg.port = p
                .parse::<u16>()
                .map_err(|_| PgCliError::Config(format!("invalid port in URI: {p}")))?;
        } else if !hostport.is_empty() {
            cfg.host = hostport.to_string();
        }

        if !dbname.is_empty() {
            cfg.database = percent_decode(dbname);
        }

        Ok(cfg)
    }

    /// Merge `other` into `self`, with `other` values taking precedence where non-default.
    ///
    /// This is used so that explicit CLI flags (parsed into the base config) always
    /// override values derived from a URI.
    pub fn merge(mut self, other: ConnectionConfig) -> ConnectionConfig {
        if other.host != "localhost" {
            self.host = other.host;
        }
        if other.port != 5432 {
            self.port = other.port;
        }
        if !other.user.is_empty() {
            self.user = other.user;
        }
        if !other.database.is_empty() {
            self.database = other.database;
        }
        if other.password.is_some() {
            self.password = other.password;
        }
        self
    }

    /// Build a default `ConnectionConfig`.
    fn defaults() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 5432,
            user: String::new(),
            database: String::new(),
            password: None,
            tls_mode: TlsMode::Prefer,
            tls_cert: None,
            tls_key: None,
            tls_ca: None,
            timeout_secs: 30,
            application_name: "pgcli-rs".to_string(),
        }
    }
}

/// Determine the TLS mode from CLI flags and `PGSSLMODE` env var.
fn resolve_tls_mode(args: &CliArgs) -> Result<TlsMode> {
    if args.no_tls {
        return Ok(TlsMode::Disabled);
    }
    if args.require_tls {
        return Ok(TlsMode::Require);
    }
    if args.tls_ca.is_some() {
        return Ok(TlsMode::VerifyFull);
    }
    if let Ok(mode) = std::env::var("PGSSLMODE") {
        return match mode.to_lowercase().as_str() {
            "disable" => Ok(TlsMode::Disabled),
            "allow" | "prefer" => Ok(TlsMode::Prefer),
            "require" => Ok(TlsMode::Require),
            "verify-ca" | "verify-full" => Ok(TlsMode::VerifyFull),
            other => Err(PgCliError::Config(format!("unknown PGSSLMODE: {other}"))),
        };
    }
    Ok(TlsMode::Prefer)
}

/// Decode `%XX` percent-encoded characters in a URI component.
fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let bytes: Vec<u8> = s.bytes().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b as char);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    // Drop the peekable iterator that borrows chars-we built `out` from bytes.
    let _ = chars.next();
    out
}

/// Look up the password in the pgpass file for the given connection parameters.
///
/// The pgpass file format uses lines of: `hostname:port:database:username:password`
/// with `*` as a wildcard in any field.
///
/// File location precedence:
/// 1. `PGPASSFILE` environment variable (all platforms)
/// 2. `%APPDATA%\postgresql\pgpass.conf` (Windows)
/// 3. `~/.pgpass` (Unix/macOS)
fn read_pgpass(host: &str, port: u16, database: &str, user: &str) -> Option<String> {
    let path = if let Ok(p) = std::env::var("PGPASSFILE") {
        PathBuf::from(p)
    } else {
        #[cfg(target_os = "windows")]
        {
            dirs::data_dir()?.join("postgresql").join("pgpass.conf")
        }
        #[cfg(not(target_os = "windows"))]
        {
            dirs::home_dir()?.join(".pgpass")
        }
    };
    read_pgpass_file(&path, host, port, database, user)
}

/// Parse a pgpass file at `path` and return the password for the given connection.
fn read_pgpass_file(
    path: &PathBuf,
    host: &str,
    port: u16,
    database: &str,
    user: &str,
) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.splitn(5, ':').collect();
        if parts.len() < 5 {
            continue;
        }
        let (ph, pp, pd, pu, pw) = (parts[0], parts[1], parts[2], parts[3], parts[4]);
        let port_str = port.to_string();
        let matches = (ph == "*" || ph == host)
            && (pp == "*" || pp == port_str)
            && (pd == "*" || pd == database)
            && (pu == "*" || pu == user);
        if matches {
            return Some(pw.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_uri() {
        let cfg = ConnectionConfig::from_uri("postgresql://alice:secret@db.local:5433/myapp")
            .expect("parse failed");
        assert_eq!(cfg.host, "db.local");
        assert_eq!(cfg.port, 5433);
        assert_eq!(cfg.user, "alice");
        assert_eq!(cfg.password.as_deref(), Some("secret"));
        assert_eq!(cfg.database, "myapp");
    }

    #[test]
    fn parse_uri_no_credentials() {
        let cfg =
            ConnectionConfig::from_uri("postgresql://localhost/testdb").expect("parse failed");
        assert_eq!(cfg.host, "localhost");
        assert_eq!(cfg.database, "testdb");
        assert!(cfg.password.is_none());
    }

    #[test]
    fn parse_postgres_scheme() {
        let cfg = ConnectionConfig::from_uri("postgres://u@host/db").expect("parse failed");
        assert_eq!(cfg.user, "u");
        assert_eq!(cfg.database, "db");
    }

    #[test]
    fn percent_decode_password() {
        assert_eq!(percent_decode("p%40ss"), "p@ss");
        assert_eq!(percent_decode("plain"), "plain");
    }

    #[test]
    fn merge_prefers_non_default() {
        let base = ConnectionConfig {
            host: "host-a".to_string(),
            port: 5432,
            ..ConnectionConfig::from_uri("postgresql://localhost/base").unwrap()
        };
        let other = ConnectionConfig::from_uri("postgresql://user:pw@host-b:5433/other").unwrap();
        let merged = base.merge(other);
        assert_eq!(merged.host, "host-b");
        assert_eq!(merged.port, 5433);
    }

    fn write_pgpass(content: &str) -> (tempfile::TempDir, PathBuf) {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.pgpass");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            write!(f, "{content}").unwrap();
        }
        (dir, path)
    }

    #[test]
    fn pgpass_exact_match() {
        let (_dir, path) = write_pgpass("localhost:5432:mydb:alice:secret\n");
        let pw = read_pgpass_file(&path, "localhost", 5432, "mydb", "alice");
        assert_eq!(pw.as_deref(), Some("secret"));
    }

    #[test]
    fn pgpass_wildcard_host_matches() {
        let (_dir, path) = write_pgpass("*:*:*:bob:bobpass\n");
        let pw = read_pgpass_file(&path, "anyhost", 9999, "anydb", "bob");
        assert_eq!(pw.as_deref(), Some("bobpass"));
    }

    #[test]
    fn pgpass_no_match_returns_none() {
        let (_dir, path) = write_pgpass("otherhost:5432:db:user:pw\n");
        let pw = read_pgpass_file(&path, "localhost", 5432, "db", "user");
        assert!(pw.is_none());
    }

    #[test]
    fn pgpass_skips_comments_and_blank_lines() {
        let (_dir, path) = write_pgpass("# comment\n\nlocalhost:5432:db:user:pass\n");
        let pw = read_pgpass_file(&path, "localhost", 5432, "db", "user");
        assert_eq!(pw.as_deref(), Some("pass"));
    }
}

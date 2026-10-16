/// Minimal connection pool for CLI use.
///
/// Holds a single active `tokio_postgres::Client` and automatically
/// reconnects on connection loss with exponential backoff.
use std::sync::Arc;
use std::time::Duration;

use tokio_postgres::config::SslMode;
use tokio_postgres::error::SqlState;
use tokio_postgres::tls::MakeTlsConnect;
use tokio_postgres::{Client, NoTls, Socket};
use tracing::{info, warn};

use crate::config::ConnectionConfig;
#[cfg(any(feature = "native-tls-backend", feature = "rustls-backend"))]
use crate::tls::TlsConfig;
use crate::tls::TlsMode;
use pgcli_core::error::{PgCliError, Result};

#[cfg(feature = "native-tls-backend")]
use crate::tls::build_native_tls as build_tls_connector;
#[cfg(all(feature = "rustls-backend", not(feature = "native-tls-backend")))]
use crate::tls::build_rustls as build_tls_connector;

/// Maximum number of reconnect attempts before giving up.
const MAX_RETRIES: u32 = 3;
/// Base delay for exponential backoff in milliseconds.
const BASE_BACKOFF_MS: u64 = 200;

/// A single-connection pool with automatic reconnect.
///
/// For CLI use a single connection is sufficient. The pool abstraction
/// allows callers to be reconnect-aware without changing their API surface.
pub struct ConnectionPool {
    client: Arc<Client>,
    config: ConnectionConfig,
}

impl ConnectionPool {
    /// Establish a connection using the given `ConnectionConfig`.
    ///
    /// Network failures are retried up to `MAX_RETRIES` times with
    /// exponential backoff. Errors the server reports, such as a wrong
    /// password or an unknown database, fail at once: every retry would get
    /// the same answer.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Connection` on a definitive error, or after all
    /// retries are exhausted.
    pub async fn connect(config: &ConnectionConfig) -> Result<Self> {
        let mut last_err = None;
        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let delay = Duration::from_millis(BASE_BACKOFF_MS * (1 << (attempt - 1)));
                warn!("Connection attempt {attempt} failed, retrying in {delay:?}");
                tokio::time::sleep(delay).await;
            }
            match try_connect(config).await {
                Err(AttemptError {
                    error,
                    retryable: false,
                }) => return Err(error),
                Ok(client) => {
                    info!(
                        host = %config.host,
                        port = config.port,
                        database = %config.database,
                        user = %config.user,
                        "Connected to PostgreSQL"
                    );
                    return Ok(Self {
                        client: Arc::new(client),
                        config: config.clone(),
                    });
                }
                Err(AttemptError { error, .. }) => {
                    last_err = Some(error);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| PgCliError::Connection("connection failed".to_string())))
    }

    /// Return a reference to the active `tokio_postgres::Client`.
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Return a shared handle to the active client, for background tasks
    /// (such as the schema cache refresh) that outlive a borrow of the pool.
    /// After a reconnect the pool hands out the new client; tasks that still
    /// hold the old one finish on it.
    pub fn shared_client(&self) -> Arc<Client> {
        Arc::clone(&self.client)
    }

    /// Check whether the connection is still alive.
    ///
    /// Sends a trivial `SELECT 1` to verify liveness.
    pub async fn is_alive(&self) -> bool {
        self.client.simple_query("").await.is_ok()
    }

    /// Reconnect using the stored configuration, replacing the current client.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Connection` if all reconnect attempts fail.
    pub async fn reconnect(&mut self) -> Result<()> {
        let config = self.config.clone();
        let new_pool = Self::connect(&config).await?;
        self.client = new_pool.client;
        Ok(())
    }

    /// Connect to a different database/user/host, replacing the current client.
    ///
    /// Fields in `patch` that are `Some(...)` override the stored config.
    /// The updated config is stored so future `reconnect()` calls also use it.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Connection` if the new connection cannot be established.
    pub async fn reconnect_to(
        &mut self,
        dbname: Option<String>,
        user: Option<String>,
        host: Option<String>,
        port: Option<u16>,
    ) -> Result<()> {
        let mut new_cfg = self.config.clone();
        if let Some(d) = dbname {
            new_cfg.database = d;
        }
        if let Some(u) = user {
            new_cfg.user = u;
        }
        if let Some(h) = host {
            new_cfg.host = h;
        }
        if let Some(p) = port {
            new_cfg.port = p;
        }
        let new_pool = Self::connect(&new_cfg).await?;
        self.client = new_pool.client;
        self.config = new_cfg;
        Ok(())
    }

    /// Return the active connection configuration.
    pub fn config(&self) -> &ConnectionConfig {
        &self.config
    }
}

/// A failed connection attempt, and whether trying again can help.
struct AttemptError {
    error: PgCliError,
    retryable: bool,
}

impl AttemptError {
    fn from_pg(e: tokio_postgres::Error, user: &str) -> Self {
        let retryable = is_retryable(&e);
        Self {
            error: classify_pg_error(e, user),
            retryable,
        }
    }

    fn definitive(error: PgCliError) -> Self {
        Self {
            error,
            retryable: false,
        }
    }
}

/// Whether a connection error is worth retrying.
///
/// Errors reported by the server (wrong password, unknown role or database,
/// `pg_hba.conf` rejection) come back the same on every try, except a server
/// that is still starting up or has no free connection slots. Network
/// failures (refused, reset, timed out) are transient. Everything else, such
/// as a TLS certificate rejection, is definitive.
fn is_retryable(e: &tokio_postgres::Error) -> bool {
    if let Some(db) = e.as_db_error() {
        return db.code() == &SqlState::CANNOT_CONNECT_NOW
            || db.code() == &SqlState::TOO_MANY_CONNECTIONS;
    }
    let mut source = std::error::Error::source(e);
    while let Some(err) = source {
        if let Some(io) = err.downcast_ref::<std::io::Error>() {
            use std::io::ErrorKind::*;
            return matches!(
                io.kind(),
                ConnectionRefused
                    | ConnectionReset
                    | ConnectionAborted
                    | NotConnected
                    | BrokenPipe
                    | TimedOut
                    | UnexpectedEof
                    | Interrupted
                    | HostUnreachable
                    | NetworkUnreachable
                    | AddrNotAvailable
            );
        }
        source = err.source();
    }
    false
}

/// Connects with `tls` and drives the connection in a background task.
async fn connect_with<T>(
    pg_cfg: &tokio_postgres::Config,
    tls: T,
) -> std::result::Result<Client, tokio_postgres::Error>
where
    T: MakeTlsConnect<Socket>,
    T::Stream: Send + 'static,
{
    let (client, connection) = pg_cfg.connect(tls).await?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            tracing::error!("PostgreSQL connection error: {e}");
        }
    });
    Ok(client)
}

/// Attempt a single connection using the given config.
async fn try_connect(config: &ConnectionConfig) -> std::result::Result<Client, AttemptError> {
    let mut pg_cfg = tokio_postgres::Config::new();
    pg_cfg
        .host(&config.host)
        .port(config.port)
        .user(&config.user)
        .dbname(&config.database)
        .application_name(&config.application_name)
        .connect_timeout(Duration::from_secs(config.timeout_secs));

    if let Some(pw) = &config.password {
        pg_cfg.password(pw);
    }

    let fail = |e| AttemptError::from_pg(e, &config.user);

    match config.tls_mode {
        TlsMode::Disabled => {
            pg_cfg.ssl_mode(SslMode::Disable);
            connect_with(&pg_cfg, NoTls).await.map_err(fail)
        }

        TlsMode::Prefer => {
            // With `SslMode::Prefer`, tokio-postgres already continues in plain
            // text on the same connection when the server declines TLS. A second,
            // plain connection is only needed when the TLS handshake itself fails.
            pg_cfg.ssl_mode(SslMode::Prefer);
            #[cfg(any(feature = "native-tls-backend", feature = "rustls-backend"))]
            {
                let tls_cfg = tls_config(config);
                match build_tls_connector(&tls_cfg) {
                    Ok(connector) => match connect_with(&pg_cfg, connector).await {
                        Ok(client) => return Ok(client),
                        // The server answered (wrong password, unknown database...):
                        // a plain retry would get the same answer one round trip later.
                        Err(e) if e.as_db_error().is_some() => return Err(fail(e)),
                        Err(e) => warn!("TLS connection failed, retrying without TLS: {e}"),
                    },
                    Err(e) => warn!("TLS setup failed, connecting without TLS: {e}"),
                }
            }
            pg_cfg.ssl_mode(SslMode::Disable);
            connect_with(&pg_cfg, NoTls).await.map_err(fail)
        }

        TlsMode::Require | TlsMode::VerifyFull => {
            // `SslMode::Require` makes tokio-postgres reject a server that does
            // not offer TLS instead of silently continuing in plain text.
            pg_cfg.ssl_mode(SslMode::Require);
            #[cfg(any(feature = "native-tls-backend", feature = "rustls-backend"))]
            {
                let connector =
                    build_tls_connector(&tls_config(config)).map_err(AttemptError::definitive)?;
                connect_with(&pg_cfg, connector).await.map_err(fail)
            }
            #[cfg(not(any(feature = "native-tls-backend", feature = "rustls-backend")))]
            {
                Err(AttemptError::definitive(PgCliError::Connection(
                    "TLS required but no TLS backend feature is enabled \
                     (build with --features native-tls-backend or rustls-backend)"
                        .to_string(),
                )))
            }
        }
    }
}

/// The TLS settings of `config`.
#[cfg(any(feature = "native-tls-backend", feature = "rustls-backend"))]
fn tls_config(config: &ConnectionConfig) -> TlsConfig {
    TlsConfig {
        mode: config.tls_mode.clone(),
        cert_path: config.tls_cert.clone(),
        key_path: config.tls_key.clone(),
        ca_path: config.tls_ca.clone(),
    }
}

/// Convert a `tokio_postgres::Error` into a human-readable `PgCliError::Connection`.
///
/// Provides more specific messages for common failures (auth, timeout, TLS)
/// so that users get actionable guidance instead of raw internal error text.
fn classify_pg_error(e: tokio_postgres::Error, user: &str) -> PgCliError {
    // If the server sent a structured error, use its message directly.
    if let Some(db_err) = e.as_db_error() {
        return PgCliError::Connection(format!("{}: {}", db_err.severity(), db_err.message()));
    }

    let msg = e.to_string();
    let msg_upper = msg.to_ascii_uppercase();
    if msg_upper.contains("GSSAPI") || msg_upper.contains("SSPI") {
        PgCliError::Connection(
            "server requested GSSAPI/SSPI (Kerberos) authentication which is not supported. \
             Configure pg_hba.conf to use scram-sha-256 or md5 for this connection."
                .to_string(),
        )
    } else if msg.contains("invalid configuration")
        || msg.contains("SASL")
        || msg.contains("password")
    {
        PgCliError::Connection(format!(
            "authentication failed for user \"{user}\" - password required. \
             Use -W to prompt, set PGPASSWORD, or add an entry to ~/.pgpass"
        ))
    } else if msg.contains("timed out") || msg.contains("timeout") {
        PgCliError::Connection(format!("connection timed out: {msg}"))
    } else if msg.contains("refused") || msg.contains("No route") || msg.contains("Network") {
        PgCliError::Connection(format!("could not connect to server: {msg}"))
    } else {
        PgCliError::Connection(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_are_sane() {
        const { assert!(MAX_RETRIES > 0) };
        const { assert!(BASE_BACKOFF_MS > 0) };
    }
}

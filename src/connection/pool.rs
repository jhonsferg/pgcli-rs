/// Minimal connection pool for CLI use.
///
/// Holds a single active `tokio_postgres::Client` and automatically
/// reconnects on connection loss with exponential backoff.
use std::time::Duration;

use tokio_postgres::Client;
use tracing::{info, warn};

use crate::connection::config::ConnectionConfig;
use crate::connection::tls::{build_no_tls, TlsConfig, TlsMode};
use crate::error::{PgCliError, Result};

#[cfg(feature = "native-tls-backend")]
use crate::connection::tls::build_native_tls;
#[cfg(all(feature = "rustls-backend", not(feature = "native-tls-backend")))]
use crate::connection::tls::build_rustls;

/// Maximum number of reconnect attempts before giving up.
const MAX_RETRIES: u32 = 3;
/// Base delay for exponential backoff in milliseconds.
const BASE_BACKOFF_MS: u64 = 200;

/// A single-connection pool with automatic reconnect.
///
/// For CLI use a single connection is sufficient. The pool abstraction
/// allows callers to be reconnect-aware without changing their API surface.
pub struct ConnectionPool {
    client: Client,
    config: ConnectionConfig,
}

impl ConnectionPool {
    /// Establish a connection using the given `ConnectionConfig`.
    ///
    /// Tries up to `MAX_RETRIES` times with exponential backoff.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Connection` after all retries are exhausted.
    pub async fn connect(config: &ConnectionConfig) -> Result<Self> {
        let mut last_err = None;
        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let delay = Duration::from_millis(BASE_BACKOFF_MS * (1 << (attempt - 1)));
                warn!("Connection attempt {attempt} failed, retrying in {delay:?}");
                tokio::time::sleep(delay).await;
            }
            match try_connect(config).await {
                Ok(client) => {
                    info!(
                        host = %config.host,
                        port = config.port,
                        database = %config.database,
                        user = %config.user,
                        "Connected to PostgreSQL"
                    );
                    return Ok(Self {
                        client,
                        config: config.clone(),
                    });
                }
                Err(e) => {
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| PgCliError::Connection("connection failed".to_string())))
    }

    /// Return a reference to the active `tokio_postgres::Client`.
    pub fn client(&self) -> &Client {
        &self.client
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

/// Attempt a single connection using the given config.
async fn try_connect(config: &ConnectionConfig) -> Result<Client> {
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

    let tls_cfg = TlsConfig {
        mode: config.tls_mode.clone(),
        cert_path: config.tls_cert.clone(),
        key_path: config.tls_key.clone(),
        ca_path: config.tls_ca.clone(),
    };

    let (client, connection) = match config.tls_mode {
        TlsMode::Disabled => pg_cfg
            .connect(build_no_tls()?)
            .await
            .map_err(|e| classify_pg_error(e, &config.user))?,

        TlsMode::Prefer => {
            // Try TLS first; fall back to plain TCP if the server refuses or the
            // handshake fails.  Each branch returns early because the two connection
            // stream types are different and cannot be unified in a single binding.
            #[cfg(feature = "native-tls-backend")]
            {
                if let Ok(connector) = build_native_tls(&tls_cfg) {
                    if let Ok((tls_client, tls_conn)) = pg_cfg.connect(connector).await {
                        tokio::spawn(async move {
                            if let Err(e) = tls_conn.await {
                                tracing::error!("PostgreSQL TLS connection error: {e}");
                            }
                        });
                        return Ok(tls_client);
                    }
                }
                let (client, connection) = pg_cfg
                    .connect(build_no_tls()?)
                    .await
                    .map_err(|e| classify_pg_error(e, &config.user))?;
                tokio::spawn(async move {
                    if let Err(e) = connection.await {
                        tracing::error!("PostgreSQL connection error: {e}");
                    }
                });
                return Ok(client);
            }
            // rustls-backend: pure-Rust TLS, no OS libraries required.
            // Note: rustls enforces certificate validation in all modes; a failed
            // handshake (e.g. self-signed cert) falls back to plain TCP.
            #[cfg(all(feature = "rustls-backend", not(feature = "native-tls-backend")))]
            {
                if let Ok(connector) = build_rustls(&tls_cfg) {
                    if let Ok((tls_client, tls_conn)) = pg_cfg.connect(connector).await {
                        tokio::spawn(async move {
                            if let Err(e) = tls_conn.await {
                                tracing::error!("PostgreSQL TLS connection error: {e}");
                            }
                        });
                        return Ok(tls_client);
                    }
                }
                let (client, connection) = pg_cfg
                    .connect(build_no_tls()?)
                    .await
                    .map_err(|e| classify_pg_error(e, &config.user))?;
                tokio::spawn(async move {
                    if let Err(e) = connection.await {
                        tracing::error!("PostgreSQL connection error: {e}");
                    }
                });
                return Ok(client);
            }
            #[cfg(not(any(feature = "native-tls-backend", feature = "rustls-backend")))]
            {
                let (client, connection) = pg_cfg
                    .connect(build_no_tls()?)
                    .await
                    .map_err(|e| classify_pg_error(e, &config.user))?;
                tokio::spawn(async move {
                    if let Err(e) = connection.await {
                        tracing::error!("PostgreSQL connection error: {e}");
                    }
                });
                return Ok(client);
            }
        }

        TlsMode::Require | TlsMode::VerifyFull => {
            #[cfg(feature = "native-tls-backend")]
            {
                let connector = build_native_tls(&tls_cfg)?;
                let (client, connection) = pg_cfg
                    .connect(connector)
                    .await
                    .map_err(|e| classify_pg_error(e, &config.user))?;
                tokio::spawn(async move {
                    if let Err(e) = connection.await {
                        tracing::error!("PostgreSQL TLS connection error: {e}");
                    }
                });
                return Ok(client);
            }
            #[cfg(all(feature = "rustls-backend", not(feature = "native-tls-backend")))]
            {
                let connector = build_rustls(&tls_cfg)?;
                let (client, connection) = pg_cfg
                    .connect(connector)
                    .await
                    .map_err(|e| classify_pg_error(e, &config.user))?;
                tokio::spawn(async move {
                    if let Err(e) = connection.await {
                        tracing::error!("PostgreSQL TLS connection error: {e}");
                    }
                });
                return Ok(client);
            }
            #[cfg(not(any(feature = "native-tls-backend", feature = "rustls-backend")))]
            {
                return Err(PgCliError::Connection(
                    "TLS required but no TLS backend feature is enabled \
                     (build with --features native-tls-backend or rustls-backend)"
                        .to_string(),
                ));
            }
        }
    };

    // Drive the plain-TCP connection in the background (Disabled mode).
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            tracing::error!("PostgreSQL connection error: {e}");
        }
    });

    Ok(client)
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
    } else if msg.contains("invalid configuration") || msg.contains("SASL") || msg.contains("password") {
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

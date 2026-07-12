/// TLS configuration and connector for PostgreSQL connections.
///
/// Supports three modes: Disabled (plain TCP), Prefer (try TLS, fall back),
/// Require (fail if no TLS), and VerifyFull (require + validate CA).
use std::path::PathBuf;

use crate::error::{PgCliError, Result};

/// How TLS should be negotiated for a connection.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum TlsMode {
    /// Plain TCP connection-no TLS attempted.
    Disabled,
    /// Attempt TLS; fall back to plain TCP if the server doesn't support it.
    #[default]
    Prefer,
    /// Require TLS; return an error if the server cannot upgrade.
    Require,
    /// Require TLS and validate the server certificate against a CA.
    VerifyFull,
}

/// TLS configuration parameters passed at connection time.
#[derive(Debug, Clone)]
pub struct TlsConfig {
    /// The negotiation mode to use.
    pub mode: TlsMode,
    /// Optional path to client certificate (mutual TLS).
    pub cert_path: Option<PathBuf>,
    /// Optional path to client private key (mutual TLS).
    pub key_path: Option<PathBuf>,
    /// Optional path to CA certificate for server verification.
    pub ca_path: Option<PathBuf>,
}

impl TlsConfig {
    /// Create a new `TlsConfig` with the given mode and no client certificates.
    pub fn new(mode: TlsMode) -> Self {
        Self {
            mode,
            cert_path: None,
            key_path: None,
            ca_path: None,
        }
    }

    /// Returns `true` when the mode permits a plain TCP fallback.
    pub fn allows_plain_fallback(&self) -> bool {
        self.mode == TlsMode::Prefer || self.mode == TlsMode::Disabled
    }

    /// Returns `true` when TLS is required and a plain connection must be rejected.
    pub fn requires_tls(&self) -> bool {
        matches!(self.mode, TlsMode::Require | TlsMode::VerifyFull)
    }
}

/// Build a `tokio_postgres::tls::NoTls` connector for disabled TLS.
///
/// # Errors
///
/// This function currently always succeeds. The `Result` wrapper is for
/// API consistency with the TLS-enabled builder variants.
pub fn build_no_tls() -> Result<tokio_postgres::NoTls> {
    Ok(tokio_postgres::NoTls)
}

#[cfg(feature = "native-tls-backend")]
/// Build a `postgres_native_tls::MakeTlsConnector` from the given `TlsConfig`.
///
/// Uses `danger_accept_invalid_certs` for Prefer/Require modes (no CA provided).
/// When a CA path is set, certificate verification is enabled.
///
/// # Errors
///
/// Returns `PgCliError::Connection` if TLS certificate loading fails.
pub fn build_native_tls(config: &TlsConfig) -> Result<postgres_native_tls::MakeTlsConnector> {
    use native_tls::TlsConnector;

    let mut builder = TlsConnector::builder();

    // Accept self-signed certs in Prefer/Require modes unless a CA is supplied.
    if config.ca_path.is_none() {
        builder.danger_accept_invalid_certs(true);
    }

    if let Some(ca_path) = &config.ca_path {
        let pem = std::fs::read(ca_path)
            .map_err(|e| PgCliError::Connection(format!("failed to read CA cert: {e}")))?;
        let cert = native_tls::Certificate::from_pem(&pem)
            .map_err(|e| PgCliError::Connection(format!("invalid CA cert: {e}")))?;
        builder.add_root_certificate(cert);
        builder.danger_accept_invalid_certs(false);
    }

    if let (Some(cert_path), Some(key_path)) = (&config.cert_path, &config.key_path) {
        let cert_pem = std::fs::read(cert_path)
            .map_err(|e| PgCliError::Connection(format!("failed to read client cert: {e}")))?;
        let key_pem = std::fs::read(key_path)
            .map_err(|e| PgCliError::Connection(format!("failed to read client key: {e}")))?;
        let identity = native_tls::Identity::from_pkcs8(&cert_pem, &key_pem)
            .map_err(|e| PgCliError::Connection(format!("invalid client identity: {e}")))?;
        builder.identity(identity);
    }

    let connector = builder
        .build()
        .map_err(|e| PgCliError::Connection(format!("TLS connector build failed: {e}")))?;
    Ok(postgres_native_tls::MakeTlsConnector::new(connector))
}

// -- rustls backend ------------------------------------------------------------
// Pure-Rust TLS using tokio-rustls / rustls 0.23.  No OS TLS libraries.
// Used for fully static musl builds where native-tls is not available.
//
// tokio-postgres 0.7.x requires:
//   - MakeTlsConnect<S> — factory; returns a TlsConnect per connection
//   - TlsConnect<S>     — performs one TLS handshake
//   - TlsStream (trait) — the resulting encrypted stream
//
// tokio_rustls::client::TlsStream is a foreign type, so we cannot implement
// the foreign tokio_postgres::tls::TlsStream trait on it directly (orphan rule).
// PgRustlsStream<S> is the newtype wrapper that bridges the two crates.
#[cfg(feature = "rustls-backend")]
/// Newtype that wraps `tokio_rustls::client::TlsStream<S>` and satisfies
/// [`tokio_postgres::tls::TlsStream`].
pub struct PgRustlsStream<S>(tokio_rustls::client::TlsStream<S>);

#[cfg(feature = "rustls-backend")]
impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin> tokio::io::AsyncRead
    for PgRustlsStream<S>
{
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

#[cfg(feature = "rustls-backend")]
impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite
    for PgRustlsStream<S>
{
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

#[cfg(feature = "rustls-backend")]
impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + Sync + 'static>
    tokio_postgres::tls::TlsStream for PgRustlsStream<S>
{
    fn channel_binding(&self) -> tokio_postgres::tls::ChannelBinding {
        tokio_postgres::tls::ChannelBinding::none()
    }
}

#[cfg(feature = "rustls-backend")]
/// A `tokio-postgres` TLS connector backed by `rustls` (pure Rust, no OS TLS libs).
///
/// Implements [`tokio_postgres::tls::MakeTlsConnect`] so it can be passed directly
/// to `tokio_postgres::Config::connect`.  Supports:
/// - Custom CA certificate for server verification (`ca_path`)
/// - Mutual TLS via PKCS#8 client cert + key (`cert_path` / `key_path`)
/// - Falls back to the `webpki-roots` trust store when no CA is provided
pub struct RustlsConnector {
    config: std::sync::Arc<tokio_rustls::rustls::ClientConfig>,
}

#[cfg(feature = "rustls-backend")]
/// Per-connection TLS handshake state created by [`RustlsConnector`].
pub struct RustlsConnect {
    connector: tokio_rustls::TlsConnector,
    domain: rustls_pki_types::ServerName<'static>,
}

#[cfg(feature = "rustls-backend")]
impl<S> tokio_postgres::tls::MakeTlsConnect<S> for RustlsConnector
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + Sync + 'static,
{
    type Stream = PgRustlsStream<S>;
    type TlsConnect = RustlsConnect;
    type Error = std::io::Error;

    fn make_tls_connect(&mut self, hostname: &str) -> std::io::Result<RustlsConnect> {
        let domain = rustls_pki_types::ServerName::try_from(hostname.to_string())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        Ok(RustlsConnect {
            connector: tokio_rustls::TlsConnector::from(self.config.clone()),
            domain,
        })
    }
}

#[cfg(feature = "rustls-backend")]
impl<S> tokio_postgres::tls::TlsConnect<S> for RustlsConnect
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + Sync + 'static,
{
    type Stream = PgRustlsStream<S>;
    type Error = std::io::Error;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = std::io::Result<PgRustlsStream<S>>> + Send>,
    >;

    fn connect(self, stream: S) -> Self::Future {
        Box::pin(async move {
            let tls = self.connector.connect(self.domain, stream).await?;
            Ok(PgRustlsStream(tls))
        })
    }
}

#[cfg(feature = "rustls-backend")]
/// Build a [`RustlsConnector`] from the given [`TlsConfig`].
///
/// Uses the `webpki-roots` trust store by default.  When `ca_path` is set,
/// only that CA is trusted (enabling pinned-CA / self-signed setups).
/// When both `cert_path` and `key_path` are set, mutual TLS is enabled.
///
/// The private key must be in PKCS#8 PEM format.  Convert a traditional
/// RSA key with:
///   `openssl pkcs8 -topk8 -nocrypt -in key.pem -out key-pkcs8.pem`
///
/// # Errors
///
/// Returns `PgCliError::Connection` if any certificate or key file cannot be
/// read or parsed.
pub fn build_rustls(config: &TlsConfig) -> Result<RustlsConnector> {
    use std::sync::Arc;
    use tokio_rustls::rustls::{ClientConfig, RootCertStore};

    // rustls 0.23 requires a process-wide default `CryptoProvider`. Installing it
    // is idempotent from our perspective: if another call already installed one
    // (e.g. from a previous connection attempt), we simply keep using it.
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();

    let mut root_store = RootCertStore::empty();

    if let Some(ca_path) = &config.ca_path {
        let ca_data = std::fs::read(ca_path)
            .map_err(|e| PgCliError::Connection(format!("failed to read CA cert: {e}")))?;
        for cert in rustls_pemfile::certs(&mut ca_data.as_slice()) {
            let cert =
                cert.map_err(|e| PgCliError::Connection(format!("invalid CA cert PEM: {e}")))?;
            root_store
                .add(cert)
                .map_err(|e| PgCliError::Connection(format!("invalid CA cert: {e}")))?;
        }
    } else {
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }

    let builder = ClientConfig::builder().with_root_certificates(root_store);

    let tls_config =
        if let (Some(cert_path), Some(key_path)) = (&config.cert_path, &config.key_path) {
            let cert_data = std::fs::read(cert_path)
                .map_err(|e| PgCliError::Connection(format!("failed to read client cert: {e}")))?;
            let certs = rustls_pemfile::certs(&mut cert_data.as_slice())
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| PgCliError::Connection(format!("invalid client cert PEM: {e}")))?;

            let key_data = std::fs::read(key_path)
                .map_err(|e| PgCliError::Connection(format!("failed to read client key: {e}")))?;
            let key = rustls_pemfile::pkcs8_private_keys(&mut key_data.as_slice())
                .next()
                .transpose()
                .map_err(|e| PgCliError::Connection(format!("invalid client key PEM: {e}")))?
                .map(rustls_pki_types::PrivateKeyDer::Pkcs8)
                .ok_or_else(|| {
                    PgCliError::Connection(
                        "no PKCS#8 private key found in key file; convert with: \
                         openssl pkcs8 -topk8 -nocrypt -in key.pem -out key-pkcs8.pem"
                            .to_string(),
                    )
                })?;

            builder
                .with_client_auth_cert(certs, key)
                .map_err(|e| PgCliError::Connection(format!("invalid client cert/key pair: {e}")))?
        } else {
            builder.with_no_client_auth()
        };

    Ok(RustlsConnector {
        config: Arc::new(tls_config),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_allows_plain() {
        let cfg = TlsConfig::new(TlsMode::Disabled);
        assert!(cfg.allows_plain_fallback());
        assert!(!cfg.requires_tls());
    }

    #[test]
    fn prefer_allows_plain() {
        let cfg = TlsConfig::new(TlsMode::Prefer);
        assert!(cfg.allows_plain_fallback());
        assert!(!cfg.requires_tls());
    }

    #[test]
    fn require_rejects_plain() {
        let cfg = TlsConfig::new(TlsMode::Require);
        assert!(!cfg.allows_plain_fallback());
        assert!(cfg.requires_tls());
    }

    #[test]
    fn verify_full_rejects_plain() {
        let cfg = TlsConfig::new(TlsMode::VerifyFull);
        assert!(!cfg.allows_plain_fallback());
        assert!(cfg.requires_tls());
    }

    #[test]
    fn build_no_tls_ok() {
        build_no_tls().expect("NoTls build failed");
    }
}

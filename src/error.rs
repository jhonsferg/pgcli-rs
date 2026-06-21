/// Unified error type for pgcli-rs.
///
/// All modules return `crate::Result<T>` which resolves to `Result<T, PgCliError>`.
#[derive(Debug, thiserror::Error)]
pub enum PgCliError {
    /// TCP or TLS connection failure.
    #[error("connection error: {0}")]
    Connection(String),

    /// PostgreSQL wire protocol violation.
    #[error("protocol error: {0}")]
    Protocol(String),

    /// Authentication failure (wrong password, unsupported method, etc.).
    #[error("authentication error: {0}")]
    Authentication(String),

    /// SQL execution error, including server-returned ErrorResponse.
    #[error("query error: {0}")]
    Query(String),

    /// File and stream I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Invalid or missing configuration.
    #[error("configuration error: {0}")]
    Config(String),

    /// Export or serialization failure.
    #[error("export error: {0}")]
    Export(String),

    /// User pressed Ctrl-C to interrupt an operation.
    #[error("interrupted")]
    Interrupted,
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, PgCliError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_connection() {
        let e = PgCliError::Connection("refused".to_string());
        assert_eq!(e.to_string(), "connection error: refused");
    }

    #[test]
    fn error_display_interrupted() {
        let e = PgCliError::Interrupted;
        assert_eq!(e.to_string(), "interrupted");
    }

    #[test]
    fn error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let e: PgCliError = io_err.into();
        assert!(e.to_string().contains("I/O error"));
    }
}

//! pgcli-core: the error type and the PostgreSQL protocol types shared by
//! every other pgcli crate (query results, cell values, SCRAM authentication).

pub mod error;
pub mod protocol;

pub use error::{PgCliError, Result};

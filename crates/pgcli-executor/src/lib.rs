//! pgcli-executor: runs SQL against an open connection, single statements,
//! batches and pipelines, and executes SQL script files with variables.

pub mod pipeline;
pub mod query;
pub mod runner;

pub use pipeline::ScriptPipeline;
pub use query::QueryExecutor;
pub use runner::ScriptRunner;

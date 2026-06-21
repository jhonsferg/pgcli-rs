/// SQL statement execution and script pipeline.
pub mod pipeline;
pub mod query;

pub use pipeline::ScriptPipeline;
pub use query::QueryExecutor;

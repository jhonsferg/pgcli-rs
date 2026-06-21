/// Query result formatting and pager invocation.
pub mod formats;
pub mod pager;
pub mod spinner;
pub mod stats;
pub mod table;

pub use formats::{FormatOptions, Formatter, LineStyle, OutputFormat};
pub use pager::Pager;
pub use spinner::Spinner;
pub use stats::{estimate_result_bytes, BenchStats};
pub use table::TableFormatter;

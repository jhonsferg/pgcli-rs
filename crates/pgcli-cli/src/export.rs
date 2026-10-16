//! `--export`: writes the result of a query to a file, with the format taken
//! from the file extension.

use std::io::Write as _;

use pgcli_core::error::PgCliError;
use pgcli_core::protocol::messages::QueryResult;
use pgcli_output::formats::{format_result, FormatOptions, OutputFormat};

/// Export `results` to `path`, inferring the format from the file extension.
///
/// The caller passes the results it already has, so the statement runs only once.
pub fn export_results(results: &[QueryResult], path: &std::path::Path) -> pgcli_core::Result<()> {
    use pgcli_output::export::{csv::CsvExporter, json::JsonExporter, sql::SqlExporter};

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let mut file = std::fs::File::create(path).map_err(PgCliError::Io)?;

    for result in results {
        match ext.as_str() {
            "csv" => CsvExporter::export(result, &mut file)?,
            "json" => JsonExporter::export_array(result, &mut file)?,
            "jsonl" | "ndjson" => JsonExporter::export_lines(result, &mut file)?,
            "tsv" => {
                let opts = FormatOptions::default();
                let out = format_result(result, &OutputFormat::Tsv, &opts)?;
                file.write_all(out.as_bytes()).map_err(PgCliError::Io)?;
            }
            "sql" => SqlExporter::export_insert(result, "exported_data", &mut file)?,
            _ => {
                // Default to CSV for unknown extensions.
                CsvExporter::export(result, &mut file)?;
            }
        }
    }
    println!("Exported to '{}'.", path.display());
    Ok(())
}

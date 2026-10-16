//! `--export`: writes the result of a query to a file, with the format taken
//! from the file extension.

use std::io::Write as _;

use pgcli_core::error::PgCliError;
use pgcli_executor::query::QueryExecutor;
use pgcli_output::formats::{format_result, FormatOptions, OutputFormat};

/// Export the result of `sql` to `path`, inferring format from the file extension.
pub async fn export_to_file(
    client: &tokio_postgres::Client,
    sql: &str,
    path: &std::path::Path,
) -> pgcli_core::Result<()> {
    use pgcli_output::export::{csv::CsvExporter, json::JsonExporter, sql::SqlExporter};

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let results = QueryExecutor::execute_batch(client, sql).await?;
    let mut file = std::fs::File::create(path).map_err(PgCliError::Io)?;

    for result in &results {
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

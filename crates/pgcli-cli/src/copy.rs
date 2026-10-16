//! `\copy`: client-side COPY that reads from or writes to a local file.

/// Execute a client-side COPY command, reading from or writing to a local file.
///
/// `spec` is the `\copy` argument string, e.g.:
/// - `"employees TO '/tmp/emp.csv' WITH (FORMAT CSV, HEADER true)"`
/// - `"employees FROM '/tmp/emp.csv' WITH (FORMAT CSV)"`
///
/// Transforms `TO 'file'` → `TO STDOUT` and `FROM 'file'` → `FROM STDIN`.
pub async fn client_copy(
    client: &tokio_postgres::Client,
    spec: &str,
) -> pgcli_core::Result<String> {
    use futures_util::{SinkExt, TryStreamExt};
    use tokio::io::AsyncWriteExt;

    let spec_upper = spec.to_uppercase();

    if let Some(to_pos) = find_keyword_pos(&spec_upper, " TO ") {
        // Extract file path from  TO 'file' or TO file
        let after_to = spec[to_pos + 4..].trim_start();
        let (file_path, rest) = extract_path(after_to)?;

        let copy_sql = format!(
            "COPY {} TO STDOUT{}",
            &spec[..to_pos],
            if rest.is_empty() {
                String::new()
            } else {
                format!(" {rest}")
            }
        );

        let stream = client
            .copy_out(&copy_sql)
            .await
            .map_err(|e| pgcli_core::error::PgCliError::Query(e.to_string()))?;

        let chunks: Vec<bytes::Bytes> = stream
            .try_collect()
            .await
            .map_err(|e| pgcli_core::error::PgCliError::Query(e.to_string()))?;

        let mut f = tokio::fs::File::create(&file_path)
            .await
            .map_err(pgcli_core::error::PgCliError::Io)?;
        let mut total = 0usize;
        for chunk in &chunks {
            f.write_all(chunk.as_ref())
                .await
                .map_err(pgcli_core::error::PgCliError::Io)?;
            total += chunk.len();
        }
        Ok(format!("COPY: wrote {total} bytes to '{file_path}'"))
    } else if let Some(from_pos) = find_keyword_pos(&spec_upper, " FROM ") {
        let after_from = spec[from_pos + 6..].trim_start();
        let (file_path, rest) = extract_path(after_from)?;

        let copy_sql = format!(
            "COPY {} FROM STDIN{}",
            &spec[..from_pos],
            if rest.is_empty() {
                String::new()
            } else {
                format!(" {rest}")
            }
        );

        let data = std::fs::read(&file_path).map_err(pgcli_core::error::PgCliError::Io)?;
        let row_count = data.iter().filter(|&&b| b == b'\n').count();

        let sink = client
            .copy_in(&copy_sql)
            .await
            .map_err(|e| pgcli_core::error::PgCliError::Query(e.to_string()))?;
        futures_util::pin_mut!(sink);
        sink.send(bytes::Bytes::from(data))
            .await
            .map_err(|e| pgcli_core::error::PgCliError::Query(e.to_string()))?;
        sink.close()
            .await
            .map_err(|e| pgcli_core::error::PgCliError::Query(e.to_string()))?;

        Ok(format!("COPY: loaded ~{row_count} rows from '{file_path}'"))
    } else {
        Err(pgcli_core::error::PgCliError::Config(
            "\\copy: expected TO or FROM in spec (e.g. \\copy TABLE TO 'file')".to_string(),
        ))
    }
}

/// Find a keyword (case-sensitive on pre-uppercased string) at a word boundary.
fn find_keyword_pos(hay_upper: &str, needle: &str) -> Option<usize> {
    hay_upper.find(needle)
}

/// Extract a file path (possibly quoted with `'` or `"`) from the start of `s`.
/// Returns `(path, remainder)`.
fn extract_path(s: &str) -> pgcli_core::Result<(String, String)> {
    let s = s.trim_start();
    if s.starts_with('\'') || s.starts_with('"') {
        let q = s.chars().next().unwrap();
        let end = s[1..].find(q).ok_or_else(|| {
            pgcli_core::error::PgCliError::Config(format!("unclosed quote in \\copy path: {s}"))
        })?;
        let path = s[1..end + 1].to_string();
        let rest = s[end + 2..].trim().to_string();
        Ok((path, rest))
    } else {
        // Unquoted path: take until whitespace
        let end = s.find(char::is_whitespace).unwrap_or(s.len());
        Ok((s[..end].to_string(), s[end..].trim().to_string()))
    }
}

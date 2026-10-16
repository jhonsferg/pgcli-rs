//! Settings derived from the CLI arguments (format options, pager, `-v`
//! variables), output helpers and the auto-expanded display rule.

use pgcli_cli::CliArgs;
use pgcli_core::error::PgCliError;
use pgcli_executor::runner::ScriptRunner;
use pgcli_meta::commands::MetaCommandDispatcher;
use pgcli_output::{formats::FormatOptions, pager::Pager};

/// Seed dispatcher variables from `-v NAME=VALUE` CLI flags.
pub fn seed_variables(dispatcher: &mut MetaCommandDispatcher, args: &CliArgs) {
    for pair in &args.set {
        if let Some((k, v)) = pair.split_once('=') {
            dispatcher.variables.insert(k.to_string(), v.to_string());
        }
    }
}

/// Seed a ScriptRunner's variables from `-v NAME=VALUE` CLI flags.
pub fn seed_variables_into_runner(runner: &mut ScriptRunner, args: &CliArgs) {
    for pair in &args.set {
        if let Some((k, v)) = pair.split_once('=') {
            runner.set_variable(k, v);
        }
    }
}

/// Build `FormatOptions` from CLI args.
pub fn build_format_opts(args: &CliArgs) -> FormatOptions {
    let format = if args.html {
        // -H flag sets HTML output; handled via OutputFormat, not opts.
        "table"
    } else if args.no_align {
        "unaligned"
    } else {
        "table"
    };
    let _ = format; // OutputFormat is set separately; opts only carries per-row options.
    FormatOptions {
        tuples_only: args.tuples_only,
        field_separator: args.field_separator.clone(),
        record_separator: args.record_separator.clone(),
        theme: args.theme.clone(),
        ..FormatOptions::default()
    }
}

/// Build a `Pager` from CLI args.
pub fn build_pager(args: &CliArgs) -> Pager {
    if args.no_pager {
        Pager::disabled()
    } else if let Some(ref cmd) = args.pager {
        Pager::with_command(cmd)
    } else {
        Pager::default()
    }
}

/// Determine the effective expanded mode considering `auto` expansion.
///
/// In auto mode, expands if the estimated result width exceeds the terminal width.
pub fn effective_expanded(
    dispatcher: &MetaCommandDispatcher,
    result: &pgcli_core::protocol::messages::QueryResult,
) -> bool {
    if dispatcher.expanded {
        return true;
    }
    if !dispatcher.expanded_auto {
        return false;
    }
    let term_width = crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(120);
    let estimated: usize = result.columns.iter().map(|c| c.name.len() + 14).sum();
    estimated > term_width
}

/// Write `output` to `--output FILE` if set, otherwise print to stdout.
pub fn write_output(output: &str, args: &CliArgs) -> pgcli_core::Result<()> {
    if let Some(ref path) = args.output {
        std::fs::write(path, output).map_err(PgCliError::Io)?;
    } else {
        println!("{output}");
    }
    Ok(())
}

/// Write to pager, or to `--output FILE` if set.
pub fn write_output_or_pager(
    output: &str,
    args: &CliArgs,
    pager: &Pager,
) -> pgcli_core::Result<()> {
    if let Some(ref path) = args.output {
        std::fs::write(path, output).map_err(PgCliError::Io)?;
    } else {
        pager.print(output).ok();
    }
    Ok(())
}

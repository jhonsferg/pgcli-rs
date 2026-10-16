//! `\pset` options and the `~/.pgclirc` startup file.

use pgcli_meta::commands::{MetaCommand, MetaCommandDispatcher, MetaResult};
use pgcli_output::formats::{FormatOptions, OutputFormat};

/// Load `~/.pgclirc` and apply `\set` and `\pset` directives found there.
///
/// Lines starting with `#` or empty lines are ignored. Only `\set` and `\pset`
/// meta-commands are honoured during startup to avoid side-effects.
pub fn load_startup_file(
    dispatcher: &mut MetaCommandDispatcher,
    format: &mut OutputFormat,
    opts: &mut FormatOptions,
) {
    let Some(home) = dirs::home_dir() else { return };
    let rc_path = home.join(".pgclirc");
    let Ok(content) = std::fs::read_to_string(&rc_path) else {
        return;
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(cmd) = MetaCommand::parse(line) {
            // Only apply safe, side-effect-free directives at startup.
            if cmd.name == "set" || cmd.name == "pset" || cmd.name == "format" {
                if let Ok(MetaResult::SetPrintOption { key, value }) = dispatcher.dispatch(&cmd) {
                    apply_pset(&key, value, format, opts, dispatcher);
                }
            }
        }
    }
}

/// Apply a `\pset KEY [VALUE]` option to the mutable format and options state.
///
/// Also updates `dispatcher` for options that mirror interactive toggles (`\x`, `\timing`).
pub fn apply_pset(
    key: &str,
    value: Option<String>,
    format: &mut OutputFormat,
    opts: &mut pgcli_output::formats::FormatOptions,
    dispatcher: &mut MetaCommandDispatcher,
) {
    use pgcli_output::formats::LineStyle;
    match key {
        "format" => match value {
            Some(ref v) => match v.parse::<OutputFormat>() {
                Ok(f) => {
                    *format = f;
                    dispatcher.format = v.clone();
                    println!("Output format is now '{v}'.");
                }
                Err(()) => eprintln!("\\pset: unknown format '{v}' (use: table csv json jsonl tsv html unaligned markdown latex asciidoc)"),
            },
            None => println!("Output format: {}", format.as_str()),
        },
        "null" => match value {
            Some(v) => {
                println!("Null display is '{v}'.");
                opts.null_display = v;
            }
            None => println!("Null display is '{}'.", opts.null_display),
        },
        "border" => match value.as_deref().and_then(|s| s.parse::<u8>().ok()) {
            Some(n) if n <= 2 => {
                opts.border = n;
                println!("Border style is {n}.");
            }
            _ => eprintln!("\\pset border: expected 0, 1, or 2"),
        },
        "title" => {
            opts.title = value.filter(|s| !s.is_empty());
            match &opts.title {
                Some(t) => println!("Title is '{t}'."),
                None => println!("Title cleared."),
            }
        }
        "footer" => {
            opts.footer = parse_on_off(value.as_deref(), opts.footer);
            println!("Footer is {}.", on_off(opts.footer));
        }
        "columns" | "C" => {
            let n = value.as_deref().and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
            opts.max_column_width = n;
            if n == 0 {
                println!("Column width: unlimited.");
            } else {
                println!("Column width: {n}.");
            }
        }
        "tuples_only" | "t" => {
            opts.tuples_only = parse_on_off(value.as_deref(), opts.tuples_only);
            println!("Tuples-only mode is {}.", on_off(opts.tuples_only));
        }
        "expanded" | "x" => {
            opts.expanded = parse_on_off(value.as_deref(), opts.expanded);
            dispatcher.expanded = opts.expanded;
            println!("Expanded display is {}.", on_off(opts.expanded));
        }
        "timing" => {
            opts.timing = parse_on_off(value.as_deref(), opts.timing);
            dispatcher.timing = opts.timing;
            println!("Timing is {}.", on_off(opts.timing));
        }
        "fieldsep" | "F" => {
            let v = value.unwrap_or_else(|| "|".to_string());
            println!("Field separator is '{v}'.");
            opts.field_separator = v;
        }
        "recordsep" | "R" => {
            opts.record_separator = value.unwrap_or_else(|| "\n".to_string());
        }
        "linestyle" => match value.as_deref() {
            Some("ascii") => { opts.line_style = LineStyle::Ascii; println!("Line style is 'ascii'."); }
            Some("old-ascii") => { opts.line_style = LineStyle::OldAscii; println!("Line style is 'old-ascii'."); }
            Some("unicode") | None => { opts.line_style = LineStyle::Unicode; println!("Line style is 'unicode'."); }
            Some(other) => eprintln!("\\pset linestyle: unknown style '{other}' (use: ascii unicode old-ascii)"),
        },
        "numericlocale" => {
            opts.numeric_locale = parse_on_off(value.as_deref(), opts.numeric_locale);
            println!("Numeric locale is {}.", on_off(opts.numeric_locale));
        }
        "pager" => {
            opts.pager_enabled = parse_on_off(value.as_deref(), opts.pager_enabled);
            println!("Pager usage is {}.", on_off(opts.pager_enabled));
        }
        other => eprintln!("\\pset: unknown option '{other}'. Run \\pset without args for help."),
    }
}

/// Parse `"on"`/`"off"` → bool, defaulting to toggling `current` when value is absent or ambiguous.
fn parse_on_off(value: Option<&str>, current: bool) -> bool {
    match value {
        Some("on") | Some("true") | Some("1") | Some("yes") => true,
        Some("off") | Some("false") | Some("0") | Some("no") => false,
        _ => !current,
    }
}

fn on_off(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

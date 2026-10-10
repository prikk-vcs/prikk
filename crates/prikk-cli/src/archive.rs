//! RFC 155 §9.8: `prikk archive export|verify|import`. Only `export` exists so far (the
//! implementation handoff's own Part E); `verify` and `import` are separate parts.

use std::path::PathBuf;

use crate::arg_scan::{SetOnce, flag_value, mark_seen, unknown_argument};
use crate::commands::CliError;
use crate::output::verification::escape_json_string;
use crate::stdout::{print, println};
use prikk_store::export_archive;

/// Dispatch `prikk archive [export]`.
pub fn run_archive(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut iter = args.into_iter();
    match iter.next().as_deref() {
        Some("export") => run_export(root, iter.collect()),
        Some(other) => Err(CliError::Usage(format!(
            "unknown archive subcommand: {other} (expected export; verify and import are not \
             built yet)"
        ))),
        None => Err(CliError::Usage(
            "archive requires a subcommand: export".to_string(),
        )),
    }
}

struct ExportArgs {
    output: PathBuf,
    format_json: bool,
}

fn parse_export_args(args: Vec<String>) -> std::result::Result<ExportArgs, CliError> {
    let mut output = None;
    let mut format_json = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--format" => {
                let value = flag_value(&mut iter, "archive export --format")?;
                match value.as_str() {
                    "json" => mark_seen(&mut format_json, "--format")?,
                    other => {
                        return Err(CliError::Usage(format!(
                            "archive export --format does not support {other:?}"
                        )));
                    }
                }
            }
            other if other.starts_with('-') => {
                return Err(unknown_argument("archive export", other));
            }
            _ => output.set_once("<file>", PathBuf::from(arg))?,
        }
    }
    let output =
        output.ok_or_else(|| CliError::Usage("archive export requires <file>".to_string()))?;
    Ok(ExportArgs {
        output,
        format_json,
    })
}

fn run_export(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let parsed = parse_export_args(args)?;
    // DC-44's own precedent (`bundle export`'s `--force` check): fail fast on an existing
    // destination before opening the repository or taking any lock.
    if crate::durable_output::destination_exists(&parsed.output) {
        return Err(format!(
            "refusing to overwrite existing file at {} (remove it first, or choose a new path)",
            parsed.output.display()
        )
        .into());
    }
    let layout = crate::open_repository(root)?;
    let report = crate::durable_output::write_new_file_durably_streaming(&parsed.output, |file| {
        let mut writer = std::io::BufWriter::new(file);
        let report = export_archive(&layout, &mut writer).map_err(|err| err.to_string())?;
        std::io::Write::flush(&mut writer).map_err(|err| err.to_string())?;
        Ok(report)
    })?;

    if parsed.format_json {
        print_export_report_json(&report, &parsed.output);
    } else {
        println!("exported {}", parsed.output.display());
        println!("repository format: {}", report.repository_format);
        println!("tool version: {}", report.tool_version);
        println!("sections: {}", report.section_count);
        println!("archive bytes: {}", report.total_bytes);
    }
    Ok(())
}

fn print_export_report_json(report: &prikk_store::ArchiveExportReport, output: &std::path::Path) {
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str("  \"schema_version\": \"prepo-export-report-v1\",\n");
    json.push_str("  \"ok\": true,\n");
    json.push_str(&format!(
        "  \"output\": {},\n",
        escape_json_string(&output.display().to_string())
    ));
    json.push_str(&format!(
        "  \"repository_format\": {},\n",
        report.repository_format
    ));
    json.push_str(&format!(
        "  \"tool_version\": {},\n",
        escape_json_string(&report.tool_version)
    ));
    json.push_str(&format!("  \"section_count\": {},\n", report.section_count));
    json.push_str(&format!("  \"total_bytes\": {}\n", report.total_bytes));
    json.push_str("}\n");
    print!("{json}");
}

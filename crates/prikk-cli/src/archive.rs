//! RFC 155 §9.8: `prikk archive export|verify|import`. `export` and `verify` exist so far (the
//! implementation handoff's own Parts E and V); `import` is a separate, later part.

use std::path::PathBuf;

use crate::arg_scan::{SetOnce, flag_value, mark_seen, unknown_argument};
use crate::commands::CliError;
use crate::output::verification::escape_json_string;
use crate::stdout::{print, println};
use prikk_store::export_archive;

/// Dispatch `prikk archive [export|verify]`.
pub fn run_archive(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut iter = args.into_iter();
    match iter.next().as_deref() {
        Some("export") => run_export(root, iter.collect()),
        Some("verify") => run_verify(iter.collect()),
        Some(other) => Err(CliError::Usage(format!(
            "unknown archive subcommand: {other} (expected export or verify; import is not built \
             yet)"
        ))),
        None => Err(CliError::Usage(
            "archive requires a subcommand: export, verify".to_string(),
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
        for (object_type, count) in &report.object_counts {
            println!("  {object_type}: {count}");
        }
        println!("local branches: {}", report.local_branches);
        println!("tags: {}", report.tags);
        println!("received refs: {}", report.received_refs);
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
    json.push_str("  \"objects\": {\n");
    let object_lines: Vec<String> = report
        .object_counts
        .iter()
        .map(|(object_type, count)| {
            format!(
                "    {}: {count}",
                escape_json_string(&object_type.to_string())
            )
        })
        .collect();
    json.push_str(&object_lines.join(",\n"));
    json.push_str("\n  },\n");
    json.push_str("  \"refs\": {\n");
    json.push_str(&format!(
        "    \"local_branches\": {},\n    \"tags\": {},\n    \"received\": {}\n",
        report.local_branches, report.tags, report.received_refs
    ));
    json.push_str("  },\n");
    json.push_str(&format!("  \"total_bytes\": {}\n", report.total_bytes));
    json.push_str("}\n");
    print!("{json}");
}

struct VerifyArgs {
    input: PathBuf,
    format_json: bool,
}

fn parse_verify_args(args: Vec<String>) -> std::result::Result<VerifyArgs, CliError> {
    let mut input = None;
    let mut format_json = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--format" => {
                let value = flag_value(&mut iter, "archive verify --format")?;
                match value.as_str() {
                    "json" => mark_seen(&mut format_json, "--format")?,
                    other => {
                        return Err(CliError::Usage(format!(
                            "archive verify --format does not support {other:?}"
                        )));
                    }
                }
            }
            other if other.starts_with('-') => {
                return Err(unknown_argument("archive verify", other));
            }
            _ => input.set_once("<file>", PathBuf::from(arg))?,
        }
    }
    let input =
        input.ok_or_else(|| CliError::Usage("archive verify requires <file>".to_string()))?;
    Ok(VerifyArgs { input, format_json })
}

/// Never opens a repository, and never writes anything, on any exit path (§9.4) -- this command
/// only ever reads `parsed.input`.
fn run_verify(args: Vec<String>) -> std::result::Result<(), CliError> {
    let parsed = parse_verify_args(args)?;
    let file = std::fs::File::open(&parsed.input).map_err(|err| {
        format!(
            "failed to open archive at {}: {err}",
            parsed.input.display()
        )
    })?;
    let mut reader = std::io::BufReader::new(file);
    let report = prikk_store::verify_archive(&mut reader).map_err(|err| err.to_string())?;

    if parsed.format_json {
        print_verify_report_json(&report);
    } else {
        print_verify_report(&report);
    }

    if report.is_internally_consistent() {
        Ok(())
    } else {
        Err(
            "archive is not internally consistent -- see the report above"
                .to_string()
                .into(),
        )
    }
}

fn print_verify_report(report: &prikk_store::ArchiveVerifyReport) {
    println!("repository format: {}", report.repository_format);
    println!("tool version: {}", report.tool_version);
    println!("structurally sound: {}", report.structurally_sound);
    println!(
        "reference closure sound: {}",
        report.reference_closure_sound
    );
    println!("signatures checked: {}", report.signatures_checked);
    for key in &report.maintainer_keys {
        println!(
            "maintainer key {} (carried, not adopted): {}",
            key.key_id, key.public_key_hex
        );
    }
    for line in &report.unresolvable_signatures {
        println!("unresolvable signature: {line}");
    }
    for line in &report.damaged {
        println!("damaged: {line}");
    }
    println!("note: {}", report.note);
}

fn print_verify_report_json(report: &prikk_store::ArchiveVerifyReport) {
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str("  \"schema_version\": \"prepo-verify-report-v1\",\n");
    json.push_str(&format!(
        "  \"ok\": {},\n",
        report.is_internally_consistent()
    ));
    json.push_str(&format!(
        "  \"repository_format\": {},\n",
        report.repository_format
    ));
    json.push_str(&format!(
        "  \"tool_version\": {},\n",
        escape_json_string(&report.tool_version)
    ));
    json.push_str(&format!(
        "  \"structure\": {{ \"ok\": {} }},\n",
        report.structurally_sound
    ));
    json.push_str(&format!(
        "  \"closure\": {{ \"ok\": {} }},\n",
        report.reference_closure_sound
    ));
    let unresolvable: Vec<String> = report
        .unresolvable_signatures
        .iter()
        .map(|line| escape_json_string(line))
        .collect();
    json.push_str(&format!(
        "  \"signatures\": {{ \"checked\": {}, \"unresolvable\": [{}] }},\n",
        report.signatures_checked,
        unresolvable.join(", ")
    ));
    let damaged: Vec<String> = report
        .damaged
        .iter()
        .map(|line| escape_json_string(line))
        .collect();
    json.push_str(&format!("  \"damaged\": [{}],\n", damaged.join(", ")));
    let maintainer_keys: Vec<String> = report
        .maintainer_keys
        .iter()
        .map(|key| {
            format!(
                "{{ \"key_id\": {}, \"public_key\": {}, \"status\": \"carried, not adopted\" }}",
                escape_json_string(&key.key_id),
                escape_json_string(&key.public_key_hex)
            )
        })
        .collect();
    json.push_str(&format!(
        "  \"maintainer_keys\": [{}],\n",
        maintainer_keys.join(", ")
    ));
    json.push_str(&format!(
        "  \"note\": {}\n",
        escape_json_string(report.note)
    ));
    json.push_str("}\n");
    print!("{json}");
}

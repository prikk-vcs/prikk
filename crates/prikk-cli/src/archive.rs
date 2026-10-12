//! RFC 155 §9.8: `prikk archive export|verify|import`. All three, plus `doctor --cancel-import`,
//! now exist (the implementation handoff's own Parts E, V, I-A/I-A2, and I-B).

use std::path::PathBuf;

use crate::arg_scan::{SetOnce, flag_value, mark_seen, unknown_argument};
use crate::commands::CliError;
use crate::output::verification::escape_json_string;
use crate::stdout::{print, println};
use prikk_store::{ArchiveImportOptions, export_archive};

/// Dispatch `prikk archive [export|verify|import]`.
pub fn run_archive(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut iter = args.into_iter();
    match iter.next().as_deref() {
        Some("export") => run_export(root, iter.collect()),
        Some("verify") => run_verify(iter.collect()),
        Some("import") => run_import(root, iter.collect()),
        Some(other) => Err(CliError::Usage(format!(
            "unknown archive subcommand: {other} (expected export, verify, or import)"
        ))),
        None => Err(CliError::Usage(
            "archive requires a subcommand: export, verify, import".to_string(),
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
    let not_checked_count = report.not_checked_signatures.len();
    println!(
        "signatures checked: {}, not checked: {not_checked_count}",
        report.signatures_checked
    );
    for key in &report.maintainer_keys {
        println!(
            "maintainer key {} (carried, not adopted): {}",
            key.key_id, key.public_key_hex
        );
    }
    for line in &report.unresolvable_signatures {
        println!("unresolvable signature: {line}");
    }
    for line in &report.not_checked_signatures {
        println!("{line}");
    }
    for line in &report.damaged {
        println!("damaged: {line}");
    }
    for line in &report.interrupted_appends {
        println!("warning: {line}");
    }
    if let Some(note) = signature_not_checked_note(report.signatures_checked, not_checked_count) {
        println!("note: {note}");
    }
    if let Some(note) = report.note {
        println!("note: {note}");
    }
}

/// External review 024: stripping (or swapping) the archive's own carried maintainer-key section
/// turns every signature into *"not checked"* -- correct per §9.4 (R2: trust is the receiver's,
/// never the archive's own), but a report that only ever printed `signatures checked: N` read
/// identically whether 0 of 16 or 16 of 16 signatures were actually checked against this
/// archive's own carried material. Both counts are now always shown together; this note explains
/// what a nonzero `not_checked` count means, only when there is one to explain.
fn signature_not_checked_note(checked: usize, not_checked: usize) -> Option<String> {
    if not_checked == 0 {
        return None;
    }
    let total = checked + not_checked;
    Some(format!(
        "{checked} of {total} signatures were checked against this archive's own carried \
         material; the other {not_checked} are checked only by the receiving repository's own \
         `prikk verify`, after import -- not a weaker check, the step that was always going to \
         check them for real"
    ))
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
    let not_checked: Vec<String> = report
        .not_checked_signatures
        .iter()
        .map(|line| escape_json_string(line))
        .collect();
    let not_checked_count = report.not_checked_signatures.len();
    let not_checked_note = signature_not_checked_note(report.signatures_checked, not_checked_count)
        .map(|note| escape_json_string(&note))
        .unwrap_or_else(|| "null".to_string());
    json.push_str(&format!(
        "  \"signatures\": {{ \"checked\": {}, \"not_checked_count\": {not_checked_count}, \
         \"unresolvable\": [{}], \"not_checked\": [{}], \"note\": {not_checked_note} }},\n",
        report.signatures_checked,
        unresolvable.join(", "),
        not_checked.join(", ")
    ));
    let damaged: Vec<String> = report
        .damaged
        .iter()
        .map(|line| escape_json_string(line))
        .collect();
    json.push_str(&format!("  \"damaged\": [{}],\n", damaged.join(", ")));
    let interrupted_appends: Vec<String> = report
        .interrupted_appends
        .iter()
        .map(|line| escape_json_string(line))
        .collect();
    json.push_str(&format!(
        "  \"interrupted_appends\": [{}],\n",
        interrupted_appends.join(", ")
    ));
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
        match report.note {
            Some(note) => escape_json_string(note),
            None => "null".to_string(),
        }
    ));
    json.push_str("}\n");
    print!("{json}");
}

struct ImportArgs {
    input: PathBuf,
    adopt: bool,
    format_json: bool,
}

fn parse_import_args(args: Vec<String>) -> std::result::Result<ImportArgs, CliError> {
    let mut input = None;
    let mut adopt = false;
    let mut format_json = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--adopt" => mark_seen(&mut adopt, "--adopt")?,
            "--format" => {
                let value = flag_value(&mut iter, "archive import --format")?;
                match value.as_str() {
                    "json" => mark_seen(&mut format_json, "--format")?,
                    other => {
                        return Err(CliError::Usage(format!(
                            "archive import --format does not support {other:?}"
                        )));
                    }
                }
            }
            other if other.starts_with('-') => {
                return Err(unknown_argument("archive import", other));
            }
            _ => input.set_once("<file>", PathBuf::from(arg))?,
        }
    }
    let input =
        input.ok_or_else(|| CliError::Usage("archive import requires <file>".to_string()))?;
    Ok(ImportArgs {
        input,
        adopt,
        format_json,
    })
}

/// `prikk archive import <file> [--adopt] [--format json]` (§9.8/§9.9). Opens the destination
/// repository and reads `parsed.input` -- every refusal before the journal's own first write
/// leaves the repository untouched (§9.1/§9.5); past it, see `import_archive`'s own doc comment.
fn run_import(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let parsed = parse_import_args(args)?;
    let layout = crate::open_repository(root)?;
    let file = std::fs::File::open(&parsed.input).map_err(|err| {
        format!(
            "failed to open archive at {}: {err}",
            parsed.input.display()
        )
    })?;
    let mut reader = std::io::BufReader::new(file);
    let mut options = ArchiveImportOptions::default();
    options.adopt = parsed.adopt;
    let report = prikk_store::import_archive(&layout, &mut reader, &options)
        .map_err(|err| err.to_string())?;

    if parsed.format_json {
        print_import_report_json(&report);
    } else {
        print_import_report(&report);
    }
    Ok(())
}

fn print_import_report(report: &prikk_store::ArchiveImportReport) {
    println!("resumed: {}", report.resumed);
    for (object_type, count) in &report.written_object_counts {
        println!("  written {object_type}: {count}");
    }
    for (object_type, count) in &report.merged_object_counts {
        if *count > 0 {
            println!("  merged {object_type}: {count}");
        }
    }
    for dropped in &report.dropped_signatures {
        println!(
            "dropped signature: {} {} by key {} ({})",
            dropped.object_type,
            dropped.object_id,
            dropped.key_id,
            dropped.reason.describe()
        );
    }
    println!("author keys recorded: {}", report.recorded_author_key_count);
    for landing in &report.landing_outcomes {
        if !landing.landed {
            println!(
                "{}: not landed -- {} (would have landed RefState {})",
                landing.ref_name,
                landing
                    .remote_reason
                    .as_deref()
                    .unwrap_or("collides with another ref"),
                landing
                    .collision_target
                    .map(|id| id.to_string())
                    .unwrap_or_default()
            );
        } else if landing.adopted {
            println!("{}: adopted as a local branch", landing.ref_name);
        } else if let Some(reason) = &landing.remote_reason {
            println!(
                "{}: landed as {} (not adopted -- {reason})",
                landing.ref_name, landing.landed_as
            );
        } else {
            println!("{}: landed as {}", landing.ref_name, landing.landed_as);
        }
    }
    if let Some(note) = publication_trust_note(&report.landing_outcomes) {
        println!("note: {note}");
    }
    println!("already imported: {}", report.already_imported);
}

/// I-F item 1: names every **planned** entry (landed or not) whose own tip carries no maintainer
/// signature this destination repository already trusts (`LandingOutcome::publication_trusted`,
/// checked against the destination's own adopted policy -- not merely "was `--adopt` used").
/// I-E's own narrower rule (landed refs only) missed a collided received pointer: it loses I-C's
/// own landing-collision rule and is never written as a ref, but its own object was still
/// imported by stage 1, and the destination's own `prikk verify` checks it regardless of whether
/// any ref names it -- so a user who did nothing unusual got a failing `verify` with no warning
/// at all about why. Each entry names its own signing key id(s)
/// (`LandingOutcome::signing_key_ids`), so `trust maintainer add` has something concrete to act
/// on (`verify`'s own lines name only object ids, never a key id). The `--adopt` hint is given
/// only when at least one named entry is the exporter's own ([`prikk_store::LandingOrigin::
/// Local`]) -- a received pointer can never be adopted, so the hint must never be offered for one
/// alone.
fn publication_trust_note(landing_outcomes: &[prikk_store::LandingOutcome]) -> Option<String> {
    let untrusted: Vec<&prikk_store::LandingOutcome> = landing_outcomes
        .iter()
        .filter(|landing| !landing.publication_trusted)
        .collect();
    if untrusted.is_empty() {
        return None;
    }
    let adopt_hint_applies = untrusted
        .iter()
        .any(|landing| landing.origin == prikk_store::LandingOrigin::Local);
    let mut parts: Vec<String> = untrusted
        .iter()
        .map(|landing| {
            let keys = if landing.signing_key_ids.is_empty() {
                "an unrecorded key".to_string()
            } else {
                landing.signing_key_ids.join(", ")
            };
            if landing.landed {
                format!(
                    "{} landed, signed by {keys}, not yet trusted",
                    landing.ref_name
                )
            } else {
                format!(
                    "{} imported without a ref (the exporter's own ref holds the name), signed \
                     by {keys}, not yet trusted",
                    landing.ref_name
                )
            }
        })
        .collect();
    let pronoun = if untrusted.len() == 1 { "its" } else { "their" };
    let distinct_key_count = untrusted
        .iter()
        .flat_map(|landing| landing.signing_key_ids.iter())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let (key_noun, once_clause) = if distinct_key_count == 1 {
        ("key", "once it is")
    } else {
        ("keys", "once they are")
    };
    parts.push(if adopt_hint_applies {
        format!(
            "`prikk verify` will fail {pronoun} publication trust until `trust maintainer add` \
             adopts the signing {key_noun} (or re-run with --adopt {once_clause})"
        )
    } else {
        format!(
            "`prikk verify` will fail {pronoun} publication trust until `trust maintainer add` \
             adopts the signing {key_noun}"
        )
    });
    Some(parts.join("; "))
}

fn print_import_report_json(report: &prikk_store::ArchiveImportReport) {
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str("  \"schema_version\": \"prepo-import-report-v1\",\n");
    json.push_str("  \"ok\": true,\n");
    json.push_str(&format!("  \"resumed\": {},\n", report.resumed));
    json.push_str("  \"written_objects\": {\n");
    let written_lines: Vec<String> = report
        .written_object_counts
        .iter()
        .map(|(object_type, count)| {
            format!(
                "    {}: {count}",
                escape_json_string(&object_type.to_string())
            )
        })
        .collect();
    json.push_str(&written_lines.join(",\n"));
    json.push_str("\n  },\n");
    json.push_str("  \"merged_objects\": {\n");
    let merged_lines: Vec<String> = report
        .merged_object_counts
        .iter()
        .map(|(object_type, count)| {
            format!(
                "    {}: {count}",
                escape_json_string(&object_type.to_string())
            )
        })
        .collect();
    json.push_str(&merged_lines.join(",\n"));
    json.push_str("\n  },\n");
    let dropped_lines: Vec<String> = report
        .dropped_signatures
        .iter()
        .map(|dropped| {
            format!(
                "{{ \"object_type\": {}, \"object_id\": {}, \"key_id\": {}, \"reason\": {} }}",
                escape_json_string(&dropped.object_type.to_string()),
                escape_json_string(&dropped.object_id.to_string()),
                escape_json_string(&dropped.key_id),
                escape_json_string(dropped.reason.describe())
            )
        })
        .collect();
    json.push_str(&format!(
        "  \"dropped_signatures\": [{}],\n",
        dropped_lines.join(", ")
    ));
    json.push_str(&format!(
        "  \"recorded_author_key_count\": {},\n",
        report.recorded_author_key_count
    ));
    let landing_lines: Vec<String> = report
        .landing_outcomes
        .iter()
        .map(|landing| {
            format!(
                "{{ \"ref_name\": {}, \"landed_as\": {}, \"adopted\": {}, \"remote_reason\": {}, \
                 \"landed\": {}, \"collision_target\": {}, \"publication_trusted\": {} }}",
                escape_json_string(&landing.ref_name),
                escape_json_string(&landing.landed_as),
                landing.adopted,
                match &landing.remote_reason {
                    Some(reason) => escape_json_string(reason),
                    None => "null".to_string(),
                },
                landing.landed,
                match landing.collision_target {
                    Some(id) => escape_json_string(&id.to_string()),
                    None => "null".to_string(),
                },
                landing.publication_trusted
            )
        })
        .collect();
    json.push_str(&format!(
        "  \"landings\": [{}],\n",
        landing_lines.join(", ")
    ));
    json.push_str(&format!(
        "  \"note\": {},\n",
        if let Some(note) = publication_trust_note(&report.landing_outcomes) {
            escape_json_string(&note)
        } else {
            "null".to_string()
        }
    ));
    json.push_str(&format!(
        "  \"already_imported\": {}\n",
        report.already_imported
    ));
    json.push_str("}\n");
    print!("{json}");
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::signature_not_checked_note;

    /// External review 024: when every signature was checked, there is nothing to explain.
    #[test]
    fn no_note_when_nothing_is_unchecked() {
        assert_eq!(signature_not_checked_note(3, 0), None);
    }

    /// The reviewer's own stripped-key-section shape: `checked: 0, not_checked: 1` (singular,
    /// matching the store-level test `archive::verify::tests::
    /// verify_shows_both_signature_counts_when_the_key_section_is_stripped`). The note must state
    /// both halves of the real total, and that the receiver's own `verify` is the real check.
    #[test]
    fn note_names_both_counts_when_the_key_section_is_stripped() {
        let note = signature_not_checked_note(0, 1).expect("a note when not_checked > 0");
        assert!(
            note.contains("0 of 1 signatures were checked"),
            "note: {note}"
        );
        assert!(
            note.contains(
                "the other 1 are checked only by the receiving repository's own \
                            `prikk verify`, after import"
            ),
            "note: {note}"
        );
    }

    /// A partial case: some checked, some not -- the counts must still both appear, against the
    /// real total (checked + not_checked), not just the not_checked count alone.
    #[test]
    fn note_names_both_counts_in_a_partial_case() {
        let note = signature_not_checked_note(5, 2).expect("a note when not_checked > 0");
        assert!(
            note.contains("5 of 7 signatures were checked"),
            "note: {note}"
        );
        assert!(note.contains("the other 2 are checked"), "note: {note}");
    }
}

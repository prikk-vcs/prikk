//! Structural extraction for governed shell and workflow procedures.

use super::{Scan, basename, inert_head, publication, rust_policy, scan_mode};

pub(super) fn allowed(tokens: &[String], index: usize, head: &str) -> bool {
    let tail = tokens.get(index + 1..).unwrap_or_default();
    match basename(head) {
        "cargo" => {
            rust_policy(tail)
                || publication(tail).is_some()
                || release_notes_procedure(tail)
                || generate_installer_procedure(tail)
                || policy_gate_procedure(tail)
                || tail
                    .split_first()
                    .is_some_and(|(command, arguments)| cargo(command, arguments))
        }
        "mdbook" => tail == ["build"],
        // DC-70, added per architect review (B1): `tar`, `rustc`, and `gh` can each execute
        // another program (tar via --to-command/-I, rustc via proc macros and build scripts,
        // gh as a general-purpose API client), so — unlike the inert set — they need an
        // exact-match entry, not blanket approval with any arguments.
        "tar" => tar(tail),
        // DC-87 Stage 2, criterion 7: `grep`/`sort`/`cat`/`diff` are all read-only text utilities
        // with no flag that invokes another program — unlike `tar`/`rustc`/`gh` above, none of
        // these needed the exact-match treatment for the reason those three did; they get it
        // anyway, per DC-70's own distinction (`inert_head` is reserved for commands that cannot
        // execute another program under *any* arguments — these four are read-only for the
        // specific arguments this workflow gives them, not unconditionally, so an exact-tail entry
        // is the more conservative choice even though a wider one could likely be justified).
        "grep" => tail == ["^block "],
        "sort" => {
            tail == [">", "../windows-object-ids.txt"] || tail == [">", "../linux-object-ids.txt"]
        }
        "cat" => tail == ["../windows-object-ids.txt"] || tail == ["../linux-object-ids.txt"],
        "diff" => tail == ["-u", "linux-object-ids.txt", "windows-object-ids.txt"],
        "rustc" => {
            tail == [
                "-vV",
                ">>",
                "dist/prikk-x86_64-unknown-linux-gnu.build-info.txt",
            ] || tail
                == [
                    "-vV",
                    ">>",
                    "dist/prikk-aarch64-unknown-linux-gnu.build-info.txt",
                ]
                // RFC 107 Stage 2: same shape, the two new targets' own build-info files.
                || tail
                    == [
                        "-vV",
                        ">>",
                        "dist/prikk-aarch64-apple-darwin.build-info.txt",
                    ]
                || tail
                    == [
                        "-vV",
                        ">>",
                        "dist/prikk-x86_64-pc-windows-msvc.build-info.txt",
                    ]
        }
        "gh" => gh_release_create(tail),
        // RFC 107 Stage 2: the Windows package step's PowerShell cmdlets. Each is exact-match for
        // the same `tar`/`rustc`/`gh` reason -- `Copy-Item`, `Compress-Archive`, and the rest can
        // all reach outside `dist`/`stage` under some argument shape. Confirmed against the real
        // lexer before writing release.yml, not assumed to parse
        // (`RFC-107-stage-2-report-ruling-v1.md` §3): no pipe and no `$var = …` assignment appear
        // in any of these, since the lexer splits the former into separate commands and
        // unconditionally rejects the latter as a dynamic command head.
        "New-Item" => windows_new_item(tail),
        "Copy-Item" => windows_copy_item(tail),
        "Compress-Archive" => windows_compress_archive(tail),
        "Set-Content" => windows_set_content(tail),
        "Add-Content" => windows_add_content(tail),
        _ => inert_head(head),
    }
}

fn windows_new_item(tail: &[String]) -> bool {
    // The lexer trims trailing punctuation from tokens (`normalize_token`, the same rule that
    // reduces `tar`'s trailing `.` argument elsewhere in this file) -- "stage," lexes as "stage",
    // not "stage,", confirmed directly rather than assumed from the comma's presence in the
    // source line.
    tail == ["-ItemType", "Directory", "-Force", "-Path", "stage", "dist"]
}

fn windows_copy_item(tail: &[String]) -> bool {
    tail == [
        "target/x86_64-pc-windows-msvc/release/prikk.exe",
        "stage/prikk.exe",
    ] || tail == ["LICENSE", "stage/LICENSE"]
}

fn windows_compress_archive(tail: &[String]) -> bool {
    tail == [
        "-Path",
        "stage/prikk.exe",
        "stage/LICENSE",
        "-DestinationPath",
        "dist/prikk-x86_64-pc-windows-msvc.zip",
    ]
}

/// Two shapes: the build-info `target:` line, and the checksum line. The checksum value is a
/// `$(...)` subexpression embedded inside the quoted `-Value` string, not a bare unquoted one --
/// the lexer's quote-tracking keeps the whole thing as a single token this way, confirmed directly
/// (an earlier, unquoted-parenthesized attempt split into multiple commands the same way a pipe
/// does). The two literal spaces before the filename match `sha256sum`'s own output shape exactly
/// (`RFC-107-stage-2-implementation-ruling-v1.md` §1).
fn windows_set_content(tail: &[String]) -> bool {
    tail == [
        "-Path",
        "dist/prikk-x86_64-pc-windows-msvc.build-info.txt",
        "-Value",
        "target: x86_64-pc-windows-msvc",
    ] || tail
        == [
            "-Path",
            "dist/prikk-x86_64-pc-windows-msvc.zip.sha256",
            "-Value",
            "$((Get-FileHash dist/prikk-x86_64-pc-windows-msvc.zip -Algorithm SHA256).Hash.ToLower())  prikk-x86_64-pc-windows-msvc.zip",
            "-NoNewline",
        ]
}

fn windows_add_content(tail: &[String]) -> bool {
    let path = "dist/prikk-x86_64-pc-windows-msvc.build-info.txt";
    tail == ["-Path", path, "-Value", "commit: $env:GITHUB_SHA"]
        || tail == ["-Path", path, "-Value", "tag: $env:GITHUB_REF_NAME"]
        || tail
            == [
                "-Path",
                path,
                "-Value",
                "build: cargo build -p prikk --release --target x86_64-pc-windows-msvc --locked",
            ]
}

fn tar(tail: &[String]) -> bool {
    tail == [
        "-C",
        "stage",
        "-czf",
        "dist/prikk-x86_64-unknown-linux-gnu.tar.gz",
        "prikk",
        "LICENSE",
    ] || tail
        == [
            "-C",
            "stage",
            "-czf",
            "dist/prikk-aarch64-unknown-linux-gnu.tar.gz",
            "prikk",
            "LICENSE",
        ]
        // RFC 107 Stage 2: macOS's package step, identical shape to the two Linux targets above.
        || tail
            == [
                "-C",
                "stage",
                "-czf",
                "dist/prikk-aarch64-apple-darwin.tar.gz",
                "prikk",
                "LICENSE",
            ]
        // DC-71 B2 ruling: the CI fixture round-trip through tar, not the artifact zip, which
        // does not preserve empty directories — create on the fixture job, extract on the
        // conformance job. The lexer's sentence-trailing-period trim (normalize_token) reduces
        // tar's literal "." (current directory) argument to an empty, dropped token, so the
        // create form's tail is four elements, not five — confirmed against the tokenizer, not
        // assumed from the shell source.
        || tail == ["-czf", "fixture-repo.tar.gz", "-C", "fixture-repo"]
        || tail == ["-xzf", "fixture-repo.tar.gz", "-C", "fixture-repo"]
        // DC-87 Stage 2, criterion 7: the same create/extract round-trip as the fixture-repo pair
        // above, for the Windows-mutated repository handed from windows-mutate to
        // verify-cross-platform-history.
        || tail == ["-czf", "windows-mutated-repo.tar.gz", "-C", "fixture-repo"]
        || tail == ["-xzf", "windows-mutated-repo.tar.gz", "-C", "windows-repo"]
        // RFC 115/DC-89: the same create/extract round-trip, for the receiver's own repository
        // handed from receiver-prepare to receiver-accept across the cross-host sync exchange --
        // it must survive the same tar-not-zip transport as the fixture and the Windows-mutated
        // repository above, for the identical DC-71 B2 reason (empty directories).
        || tail == ["-czf", "receiver-repo.tar.gz", "-C", "receiver-repo"]
        || tail == ["-xzf", "receiver-repo.tar.gz", "-C", "receiver-repo"]
}

/// `gh release create $TAG <assets...> --repo prikk-vcs/prikk --title $TAG --notes-file <path>`.
/// The release tag cannot be enumerated in advance the way `cargo build`'s two targets are, so
/// this matches on shape instead: every other token is a fixed literal, and the two `$TAG`
/// positions (the release identifier and its title) must be the identical token, whatever it is.
fn gh_release_create(tail: &[String]) -> bool {
    let [
        action,
        subcommand,
        tag,
        assets @ ..,
        repo_flag,
        repo,
        title_flag,
        title,
        notes_flag,
        notes,
    ] = tail
    else {
        return false;
    };
    action == "release"
        && subcommand == "create"
        && assets
            == [
                "dist/*.tar.gz",
                "dist/*.tar.gz.sha256",
                "dist/*.zip",
                "dist/*.zip.sha256",
                "dist/*.build-info.txt",
                // Universal installer/uninstaller handoff v1 §4.1: install.sh/uninstall.sh and
                // their checksums, generated the same way and by the same job as everything
                // else in this glob list.
                "dist/*.sh",
                "dist/*.sh.sha256",
            ]
        && repo_flag == "--repo"
        && repo == "prikk-vcs/prikk"
        && title_flag == "--title"
        && title == tag
        && notes_flag == "--notes-file"
        && notes == "release-notes.md"
}

/// `cargo run -p prikk-release-policy --locked -- release-notes $TAG dist > release-notes.md`.
/// RFC 107 Stage 1. Mirrors `gh_release_create`'s shape-matching for the same reason: the release
/// tag cannot be enumerated in advance, so every other token is fixed and the tag is free to vary.
/// A dedicated matcher rather than widening `rust_policy` -- that helper pins the literal `check`
/// subcommand `reference-check` uses to verify what the docs advertise
/// (`RFC-107-stage-1-report-ruling-v1.md` §1), and widening a shared predicate as a side effect of
/// an unrelated procedure is how allowlists rot.
fn release_notes_procedure(tail: &[String]) -> bool {
    tail == [
        "run",
        "-p",
        "prikk-release-policy",
        "--locked",
        "--",
        "release-notes",
        "$TAG",
        "dist",
        ">",
        "release-notes.md",
    ]
}

/// `cargo run -p prikk-release-policy --locked -- generate-installer dist`. Universal
/// installer/uninstaller handoff v1 §4.1: no free-varying token here (unlike
/// `release_notes_procedure`'s tag) -- the output directory is always the fixed `dist`, so this is
/// a literal exact match rather than a shape match.
fn generate_installer_procedure(tail: &[String]) -> bool {
    tail == [
        "run",
        "-p",
        "prikk-release-policy",
        "--locked",
        "--",
        "generate-installer",
        "dist",
    ]
}

/// RFC 141 §2.4, closed: the four steps CI's `policy` job must run, each as one whole `run:` script
/// in `.github/workflows/ci.yml`. `policy_gate_procedure` below only *permits* them, so without this
/// list any one could be deleted from the job and no gate would notice; `boundary-check` now
/// requires every one and names the one missing.
pub(crate) const REQUIRED_CI_POLICY_STEPS: [&str; 4] = [
    "cargo run --locked -p prikk-release-policy -- check",
    "cargo run --locked -p prikk-release-policy -- boundary-check",
    "cargo run --locked -p prikk-release-policy -- reference-check",
    "cargo run --locked -p prikk-release-policy -- size-check",
];

/// Every `run:` script in a workflow, as the scanner extracts it -- the same parse the procedure
/// allowlist reads, so a step the required list finds is a step the allowlist has classified.
pub(crate) fn yaml_run_scripts(text: &str) -> Result<Vec<String>, &'static str> {
    yaml_scripts(text)
}

/// RFC 141 increment 4: CI's `policy` job runs the three gates besides `check` (which `rust_policy`
/// already accepts) in the invocation form EXECUTION-ORDER §6 rule 9 names. Exact match, no flag
/// and no other subcommand -- `boundary-check --graph` is a read-only report, not a gate step, and
/// is refused here like anything else the job does not spell.
fn policy_gate_procedure(tail: &[String]) -> bool {
    let [run, locked, package_flag, package, separator, gate] = tail else {
        return false;
    };
    run == "run"
        && locked == "--locked"
        && package_flag == "-p"
        && package == "prikk-release-policy"
        && separator == "--"
        && matches!(
            gate.as_str(),
            "boundary-check" | "reference-check" | "size-check"
        )
}

fn cargo(command: &str, arguments: &[String]) -> bool {
    match command {
        "fmt" => arguments == ["--check"] || arguments == ["--all", "--", "--check"],
        "test" => {
            arguments == ["--workspace"]
                || arguments == ["--workspace", "--locked"]
                // DC-87 Stage 2, fourth CI run: windows-mutation/macos-mutation's own Test step,
                // spelled out exactly (DC-70 B1 precedent) -- `--no-fail-fast` so one Windows/macOS
                // failure doesn't hide every failure behind it in the same run.
                || arguments == ["--workspace", "--locked", "--no-fail-fast"]
                // DC-81: the macOS CI job's NFR-PERF-01 data-collection step, spelled out exactly
                // (DC-70 B1 precedent — never widened to accept an arbitrary name/flag set after
                // `--`). Runs exactly one `#[ignore]`d test by name; it is not a gate.
                || arguments
                    == [
                        "--workspace",
                        "--locked",
                        "--",
                        "measure_directory_sync_fsync_vs_fcntl_fullfsync",
                        "--ignored",
                        "--nocapture",
                    ]
        }
        "clippy" => {
            arguments
                == [
                    "--workspace",
                    "--all-targets",
                    "--all-features",
                    "--locked",
                    "--",
                    "-D",
                    "warnings",
                ]
        }
        // DC-71: CI must populate the cache for every target before the boundary check runs
        // `cargo metadata --locked --offline`. `fetch` downloads only; it cannot publish.
        "fetch" => arguments == ["--locked"],
        "check" => arguments == ["--workspace", "--all-targets", "--locked"],
        // RFC 126 §4: `ci.yml`'s doc-lint gate. The `-D rustdoc::private_intra_doc_links` lint
        // travels via the step's own `RUSTDOCFLAGS` env var, not a `--` passthrough on this line
        // (`cargo doc` has no such passthrough, unlike `clippy`/`test`) -- so the scanned command
        // itself stays exactly this, with no trailing flags to spell out here.
        "doc" => arguments == ["--workspace", "--no-deps"],
        // RFC 126 §3: `security-audit.yml`'s scheduled job. Deliberately bare -- no `--no-fetch`,
        // since fetching a current advisory database is this job's entire reason to run on a
        // schedule rather than relying on whatever the standing local gate's own `--no-fetch`
        // invocation last saw.
        "audit" => arguments.is_empty(),
        "build" => {
            arguments == ["--workspace", "--locked"]
                // DC-70: release.yml's two per-target release-binary builds, spelled out in
                // full rather than templated, so each is an exact, reviewable entry.
                || arguments
                    == [
                        "-p",
                        "prikk",
                        "--release",
                        "--target",
                        "x86_64-unknown-linux-gnu",
                        "--locked",
                    ]
                || arguments
                    == [
                        "-p",
                        "prikk",
                        "--release",
                        "--target",
                        "aarch64-unknown-linux-gnu",
                        "--locked",
                    ]
                // RFC 107 Stage 2: the two new release-binary builds, same exact-entry shape.
                || arguments
                    == [
                        "-p",
                        "prikk",
                        "--release",
                        "--target",
                        "aarch64-apple-darwin",
                        "--locked",
                    ]
                || arguments
                    == [
                        "-p",
                        "prikk",
                        "--release",
                        "--target",
                        "x86_64-pc-windows-msvc",
                        "--locked",
                    ]
                // DC-71: ci.yml's read-only-fixture jobs build only the prikk binary, debug
                // profile, on whichever platform the job runs.
                || arguments == ["-p", "prikk", "--locked"]
        }
        "install" => {
            arguments
                == [
                    "mdbook",
                    "--no-default-features",
                    "--features",
                    "search",
                    "--vers",
                    "^0.5",
                    "--locked",
                ]
                // DC-77: docs.yml's mdbook-mermaid install, so Mermaid diagrams render as
                // pictures rather than code blocks. Exact match only — this arm accepts exactly
                // this vector, not `mdbook-mermaid` with any arguments (DC-70 B1 precedent).
                || arguments == ["mdbook-mermaid", "--vers", "^0.17", "--locked"]
                // RFC 126 §3: security-audit.yml's own install step. `cargo-audit` is not
                // preinstalled on GitHub runners; installing the tool directly (rather than a
                // third-party `audit-check`-style action) was the handoff's own explicit ruling.
                || arguments == ["cargo-audit", "--locked"]
        }
        _ => false,
    }
}

pub(crate) fn shell(text: &str) -> Scan {
    scan_mode(text, true)
}

pub(crate) fn yaml(text: &str) -> Scan {
    match yaml_scripts(text) {
        Ok(scripts) => {
            let mut result = Scan::default();
            for script in scripts {
                let scanned = scan_mode(&script, true);
                result.invocations.extend(scanned.invocations);
                result.errors.extend(scanned.errors);
            }
            result
        }
        Err(error) => Scan {
            invocations: Vec::new(),
            errors: vec![error],
        },
    }
}

fn yaml_scripts(text: &str) -> Result<Vec<String>, &'static str> {
    let lines: Vec<&str> = text.lines().collect();
    let mut scripts = Vec::new();
    let mut index = 0;
    while let Some(line) = lines.get(index) {
        let indent = indentation(line);
        let mapping = sequence_mapping(line.trim_start());
        let Some(value) = run_value(mapping)? else {
            index += 1;
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            if parent_key(&lines, index, indent) == Some("defaults") {
                index += 1;
                continue;
            }
            return Err("empty-yaml-run-scalar");
        }
        if value.starts_with('|') || value.starts_with('>') {
            let folded = value.starts_with('>');
            let (script, next) = block(&lines, index + 1, indent, folded);
            if script.is_empty() {
                return Err("empty-yaml-run-block");
            }
            scripts.push(script);
            index = next;
            continue;
        }
        scripts.push(scalar(value)?);
        index += 1;
    }
    Ok(scripts)
}

fn sequence_mapping(line: &str) -> &str {
    line.strip_prefix('-').map(str::trim_start).unwrap_or(line)
}

fn run_value(mapping: &str) -> Result<Option<&str>, &'static str> {
    if mapping.starts_with('{') {
        return flow_run_value(mapping);
    }
    let Ok((key, value)) = split_field(mapping) else {
        return Ok(None);
    };
    Ok((normalized_key(key) == "run").then_some(value))
}

fn flow_run_value(mapping: &str) -> Result<Option<&str>, &'static str> {
    let inner = mapping
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
        .ok_or("malformed-yaml-flow-mapping")?;
    let mut run = None;
    for field in flow_fields(inner)? {
        let (key, value) = split_field(field)?;
        if normalized_key(key) == "run" && run.replace(value.trim()).is_some() {
            return Err("duplicate-yaml-run-key");
        }
    }
    Ok(run)
}

fn normalized_key(key: &str) -> &str {
    key.trim().trim_matches(['\'', '"'])
}

fn parent_key<'a>(lines: &[&'a str], index: usize, child_indent: usize) -> Option<&'a str> {
    lines.get(..index)?.iter().rev().find_map(|line| {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') || indentation(line) >= child_indent {
            return None;
        }
        split_field(sequence_mapping(trimmed))
            .ok()
            .map(|(key, _)| normalized_key(key))
    })
}

fn flow_fields(mapping: &str) -> Result<Vec<&str>, &'static str> {
    let mut fields = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut depth = 0_u32;
    for (index, character) in mapping.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quote == Some('"') && character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if character == active {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            '[' | '{' => depth += 1,
            ']' | '}' => depth = depth.checked_sub(1).ok_or("malformed-yaml-flow-nesting")?,
            ',' if depth == 0 => {
                fields.push(mapping.get(start..index).unwrap_or_default());
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    if quote.is_some() || escaped || depth != 0 {
        return Err("malformed-yaml-flow-mapping");
    }
    fields.push(mapping.get(start..).unwrap_or_default());
    Ok(fields)
}

fn split_field(field: &str) -> Result<(&str, &str), &'static str> {
    let mut quote = None;
    let mut escaped = false;
    let mut depth = 0_u32;
    for (index, character) in field.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quote == Some('"') && character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if character == active {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            '[' | '{' => depth += 1,
            ']' | '}' => depth = depth.checked_sub(1).ok_or("malformed-yaml-flow-nesting")?,
            ':' if depth == 0 => {
                let key = field.get(..index).unwrap_or_default();
                let value = field
                    .get(index + character.len_utf8()..)
                    .unwrap_or_default();
                return Ok((key, value));
            }
            _ => {}
        }
    }
    Err("malformed-yaml-flow-field")
}

fn scalar(value: &str) -> Result<String, &'static str> {
    let value = value.trim_end_matches('}').trim_end();
    if value.starts_with(['|', '>']) {
        return Err("unsupported-inline-yaml-run");
    }
    if value.starts_with('"') || value.starts_with('\'') {
        return Err("unsupported-quoted-yaml-run-scalar");
    }
    Ok(value.to_owned())
}

fn block(lines: &[&str], mut index: usize, parent_indent: usize, folded: bool) -> (String, usize) {
    let mut output = String::new();
    while let Some(line) = lines.get(index) {
        if !line.trim().is_empty() && indentation(line) <= parent_indent {
            break;
        }
        if !output.is_empty() {
            output.push(if folded { ' ' } else { '\n' });
        }
        output.push_str(line.trim());
        index += 1;
    }
    (output, index)
}

fn indentation(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

#[cfg(test)]
#[path = "procedure/tests.rs"]
mod tests;

//! RFC 160 P4, at the CLI: **a damaged repository never crashes the tools that diagnose it.**
//!
//! For every container file a repository actually has -- every object container with a record, the object index, the ref log
//! container, the pointer index, the trust and author key containers, the WAL -- the first record's length field is set to 2^62 and
//! `prikk verify` and `prikk doctor` are run on it. Each must **exit with a non-zero status** (an exit status, never a signal: an
//! allocation failure aborts) and **say something** (a finding, not silence). The matrix runs on a **sealed** repository and on an
//! **unsealed** one: on an unsealed repository nothing replays a sealed block's state over the damaged object, which is how a
//! damaged header used to go unreported (`verify` printed `object items: 0 scanned, 0 failed` and exited 0).
//!
//! `PRIKK_HOSTILE_BASELINE_BINARY` names an older binary (0.47.0's); when set, the same matrix runs on it and its results are printed
//! as `BASELINE ...` lines for the round's report (G2). It asserts nothing about the baseline.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

/// One container file of a repository and where its first record's length field sits.
struct Target {
    /// Relative to `.prikk/`.
    relative: String,
    /// Byte offset of the big-endian `u64` body length in the first record's header.
    length_field: usize,
}

/// A frame header is `magic(8) version(2)` and then the length; but the ref container puts the 32-byte ref key first, and the WAL a
/// `seq(8)`. The magic says which (a magic that is none of these is not a container this matrix knows: skipped).
fn length_field_for(magic: &[u8]) -> Option<usize> {
    match magic {
        b"PREFCON1" => Some(8 + 2 + 32),
        b"PWALR001" => Some(8 + 2 + 8),
        m if m.len() == 8 && m.starts_with(b"P") && m.iter().all(|b| b.is_ascii_alphanumeric()) => {
            Some(8 + 2)
        }
        _ => None,
    }
}

fn discover(repo: &Path) -> Vec<Target> {
    let root = repo.join(".prikk");
    let mut found = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            if bytes.len() < 50 {
                continue;
            }
            let Some(length_field) = length_field_for(&bytes[..8]) else {
                continue;
            };
            let relative = path
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            found.push(Target {
                relative,
                length_field,
            });
        }
    }
    found.sort_by(|a, b| a.relative.cmp(&b.relative));
    found
}

fn unsealed_repository(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    for index in 0..3 {
        std::fs::write(
            repo.join(format!("f{index}.txt")),
            format!("file {index}\n").repeat(40),
        )
        .unwrap();
    }
    support::ok(&support::commit(&repo, "heads/main", "damage me"), "commit");
    repo
}

fn sealed_repository(tag: &str) -> PathBuf {
    let repo = unsealed_repository(tag);
    support::ok(&support::seal(&repo, "heads/main"), "seal");
    support::ok(
        &support::tag_create(&repo, "tags/v1", "heads/main"),
        "tag create",
    );
    repo
}

/// What one run of a command did: an exit status (`None` = ended by a signal) and its combined output.
struct Ran {
    code: Option<i32>,
    text: String,
}

fn run(binary: Option<&Path>, repo: &Path, args: &[&str]) -> Ran {
    let mut command = match binary {
        Some(path) => {
            let mut command = Command::new(path);
            command.current_dir(repo);
            support::isolate_key_environment_for(&mut command, Some(repo));
            command
        }
        None => support::prikk(repo),
    };
    let output = command.args(args).output().unwrap();
    Ran {
        code: output.status.code(),
        text: format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    }
}

fn damage(repo: &Path, target: &Target, claimed: u64) {
    let path = repo.join(".prikk").join(&target.relative);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[target.length_field..target.length_field + 8].copy_from_slice(&claimed.to_be_bytes());
    std::fs::write(&path, bytes).unwrap();
}

const CLAIMED: u64 = 1 << 62;

/// **Rows that exit 0 today, on 0.47.0 as well** -- damage the tools cannot tell from an interrupted append, named for a ruling in the
/// round's report (F3). They must still end by an exit status and print no allocation failure; they are the only rows allowed to exit
/// 0, and the list is here so that it is a list, not a habit. (Each one is a file whose reader classifies a frame that claims more
/// bytes than remain as a *torn tail*, and no index or checksum exists to say otherwise: the WAL, the object index -- whose entries are
/// fixed-width, so a header that claims another length is in fact malformed -- and the author key index.)
const OPEN: &[(&str, &str, &str)] = &[
    (
        "unsealed",
        "active/default/queue.wal",
        "WAL: a torn tail is repairable by `doctor --repair-wal-tail`; a damaged header reads as one",
    ),
    (
        "unsealed",
        "containers/index.container",
        "object index: a header claiming another length than the fixed width reads as a torn tail",
    ),
    (
        "sealed",
        "trust/author-keys.container",
        "author key index: a first frame that claims more than remains reads as a torn tail",
    ),
    (
        "unsealed",
        "trust/author-keys.container",
        "author key index: as above",
    ),
];

/// One row of the matrix.
struct Row {
    repository: &'static str,
    target: String,
    verify: Ran,
    doctor: Ran,
}

fn matrix(binary: Option<&Path>) -> Vec<Row> {
    let mut rows = Vec::new();
    for (kind, build) in [
        ("sealed", sealed_repository as fn(&str) -> PathBuf),
        ("unsealed", unsealed_repository as fn(&str) -> PathBuf),
    ] {
        let template = build(&format!("hostile-{kind}"));
        for target in discover(&template) {
            let copy = support::unique_repo(&format!("hostile-{kind}-copy"));
            let _ = std::fs::remove_dir_all(&copy);
            support::copy_dir_recursive(&template, &copy);
            damage(&copy, &target, CLAIMED);
            let verify = run(binary, &copy, &["verify"]);
            let doctor = run(binary, &copy, &["doctor"]);
            rows.push(Row {
                repository: kind,
                target: target.relative,
                verify,
                doctor,
            });
            let _ = std::fs::remove_dir_all(&copy);
        }
        let _ = std::fs::remove_dir_all(&template);
    }
    rows
}

/// **P4 at the CLI.** Every damaged first record, in every container file the repository has, on a sealed and on an unsealed
/// repository: `verify` and `doctor` end by an exit status (never a signal), non-zero, saying something. A run that "succeeds" (exit
/// 0) over a record whose length is 2^62 is the blind spot RFC 160 §7 names, and is red here.
/// **Perturb:** (a) remove the ranged reader's clamp and the frame-length comparison (either alone leaves the other guarding): the
/// object-container rows end by a signal; (b) restore the `continue` in `verify/objects.rs`'s index pass: the unsealed blob and
/// patch rows exit 0.
#[test]
fn verify_and_doctor_end_by_an_exit_status_and_say_something_on_every_damaged_first_record() {
    let rows = matrix(None);
    // The matrix must have covered the container types each repository really has.
    for (kind, needed) in [
        (
            "sealed",
            &[
                "containers/blob/a.container",
                "containers/patch/a.container",
                "containers/block/a.container",
                "containers/index.container",
                "refs/containers/log-a.container",
                "refs/containers/pointer-index-a.container",
            ][..],
        ),
        (
            "unsealed",
            &[
                "containers/blob/a.container",
                "containers/index.container",
                "active/default/queue.wal",
            ][..],
        ),
    ] {
        let mine: Vec<&Row> = rows.iter().filter(|row| row.repository == kind).collect();
        for needed in needed {
            assert!(
                mine.iter().any(|row| row.target == *needed),
                "{kind}: the matrix found no {needed}: {:?}",
                mine.iter().map(|row| &row.target).collect::<Vec<_>>()
            );
        }
    }
    let mut failures = Vec::new();
    for row in &rows {
        let open = OPEN
            .iter()
            .find(|(repository, target, _)| *repository == row.repository && *target == row.target);
        for (command, ran) in [("verify", &row.verify), ("doctor", &row.doctor)] {
            let label = format!("{} / {} / {command}", row.repository, row.target);
            match ran.code {
                None => failures.push(format!("{label}: ended by a signal\n{}", ran.text)),
                Some(0) if open.is_none() => {
                    failures.push(format!(
                        "{label}: exit 0 over a damaged first record\n{}",
                        ran.text
                    ));
                }
                Some(_) if ran.text.trim().is_empty() => {
                    failures.push(format!("{label}: exited silently"));
                }
                Some(_) => {}
            }
            if ran.text.contains("memory allocation") {
                failures.push(format!("{label}: attempted the claimed allocation"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
}

/// **G2** -- the same matrix on this build and on an older binary (`PRIKK_HOSTILE_BASELINE_BINARY`), printed as a table of exit
/// statuses for the report. Run deliberately (`--ignored --nocapture`); it asserts nothing.
#[test]
#[ignore = "measurement unit G2 (RFC 160 P4's CLI matrix on this build and 0.47.0); run deliberately"]
fn hostile_lengths_g2_table() {
    let baseline = std::env::var("PRIKK_HOSTILE_BASELINE_BINARY").ok();
    let mine = matrix(None);
    let theirs = baseline
        .as_deref()
        .map(|path| matrix(Some(Path::new(path))));
    let show = |ran: &Ran| match ran.code {
        Some(code) => format!("exit {code}"),
        None => "SIGNAL".to_string(),
    };
    println!(
        "| repository | file | this: verify | this: doctor | 0.47.0: verify | 0.47.0: doctor |"
    );
    println!("|---|---|---|---|---|---|");
    for (index, row) in mine.iter().enumerate() {
        let (old_verify, old_doctor) = theirs
            .as_ref()
            .and_then(|rows| rows.get(index))
            .map_or(("-".to_string(), "-".to_string()), |old| {
                (show(&old.verify), show(&old.doctor))
            });
        println!(
            "| {} | `{}` | {} | {} | {} | {} |",
            row.repository,
            row.target,
            show(&row.verify),
            show(&row.doctor),
            old_verify,
            old_doctor
        );
    }
}

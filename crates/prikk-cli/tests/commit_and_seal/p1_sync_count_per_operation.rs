//! 0.58.0 P1: how many times each operation reaches the platform's sync, counted at the primitive, and whether the counter agrees
//! with the kernel.
//!
//! **What is counted.** `prikk-store`'s `test-support` build (the build these tests run against; no shipped build has it) counts every
//! call to the platform's sync at its two primitives: a file's (`anchored::sync_file`: `fsync` on Linux, `F_FULLFSYNC` on macOS,
//! `FlushFileBuffers` on Windows) and a directory's (`AnchoredDirectory::sync`). With `PRIKK_TEST_REPORT_SYNC_CALLS` set, the binary
//! prints `sync-calls: N` on exit. The numbers pinned below are that count, per operation kind, on the fixtures built here.
//!
//! **What the kernel witnesses.** Linux only. Where `strace` runs, each operation is run under `strace -f -e trace=fsync,fdatasync`
//! and the counter must equal the kernel's count of the same run. On macOS and Windows the pinned numbers are the primitive's calls
//! with no kernel witness, and this file does not say more. Where `strace` is absent or not permitted the agreement is skipped with
//! its reason printed, and the pin stands on the counter.
//!
//! **The one known difference**, named: a command that writes a file the user named (`--output`, `--claims-out`, `cat --output`)
//! syncs it in `prikk-cli`'s `durable_output`, with `std`, outside `prikk-store`. The counter does not count that, and `strace` does.
//! None of the operations below passes such a flag.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::Path;
use std::process::{Command, Output};

use crate::support;

const FILES: usize = 10;

/// One run's counter, and the kernel's count of the same run where `strace` could witness it.
struct Counted {
    output: Output,
    counter: usize,
    kernel: Option<usize>,
}

fn strace_available() -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Err("strace witnesses Linux only".to_string());
    }
    match Command::new("strace").arg("-V").output() {
        Ok(out) if out.status.success() => {}
        Ok(_) | Err(_) => return Err("strace is not installed".to_string()),
    }
    // ptrace can be denied (a container, a sandbox): try the cheapest command.
    let probe = Command::new("strace")
        .args(["-f", "-e", "trace=fsync", "-o", "/dev/null", "true"])
        .output();
    match probe {
        Ok(out) if out.status.success() => Ok(()),
        _ => Err("strace cannot trace here (ptrace denied)".to_string()),
    }
}

/// Run `prikk <args>` in `repo` with the counter report on, under `strace` when it can witness. `extra_env` carries a signer.
fn counted(repo: &Path, extra_env: &[(&str, String)], args: &[&str]) -> Counted {
    let trace_file =
        std::env::temp_dir().join(format!("p1-strace-{}.txt", support::unique_suffix()));
    let witnessed = strace_available();
    let mut command = if witnessed.is_ok() {
        let mut command = Command::new("strace");
        command
            .args(["-f", "-e", "trace=fsync,fdatasync", "-o"])
            .arg(&trace_file)
            .arg(support::prikk_bin_path());
        command.current_dir(repo);
        command
    } else {
        let mut command = Command::new(support::prikk_bin_path());
        command.current_dir(repo);
        command
    };
    // The same key isolation `support::prikk` applies.
    support::isolate_key_environment_for(&mut command, Some(repo));
    command.env("PRIKK_TEST_REPORT_SYNC_CALLS", "1");
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command.args(args);
    let output = command.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let counter = stderr
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("sync-calls: "))
        .unwrap_or_else(|| {
            panic!("no `sync-calls:` line (is the binary built with prikk-store's test-support?): {stderr}")
        })
        .trim()
        .parse::<usize>()
        .unwrap();
    let kernel = witnessed.ok().map(|()| {
        let trace = std::fs::read_to_string(&trace_file).unwrap_or_default();
        let _ = std::fs::remove_file(&trace_file);
        trace
            .lines()
            .filter(|line| {
                let call = line.split_whitespace().nth(1).unwrap_or("");
                call.starts_with("fsync(") || call.starts_with("fdatasync(")
            })
            .count()
    });
    Counted {
        output,
        counter,
        kernel,
    }
}

fn maintainer_env() -> Vec<(&'static str, String)> {
    vec![
        (
            "PRIKK_MAINTAINER_KEY_ID",
            support::MAINTAINER_KEY_ID.to_string(),
        ),
        (
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED))
                .display()
                .to_string(),
        ),
    ]
}

fn author_env() -> Vec<(&'static str, String)> {
    vec![
        ("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID.to_string()),
        (
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX)
                .display()
                .to_string(),
        ),
    ]
}

/// `FILES` files, written and committed one commit each (queued, not sealed).
fn queue_commits(repo: &Path) {
    for index in 0..FILES {
        std::fs::write(
            repo.join(format!("f{index}.txt")),
            format!("line {index}\n"),
        )
        .unwrap();
        support::ok(
            &support::commit(repo, "heads/main", &format!("c{index}")),
            "commit",
        );
    }
}

fn expect_ok(counted: &Counted, what: &str) {
    assert!(
        counted.output.status.success(),
        "{what} failed: {}",
        String::from_utf8_lossy(&counted.output.stderr)
    );
}

/// Every operation kind: its counter, pinned; and the kernel's count of the same run, equal, where `strace` witnessed it.
#[test]
fn each_operation_kind_counts_what_the_kernel_counts() {
    let mut rows: Vec<(&str, usize, Option<usize>)> = Vec::new();
    let mut record = |name: &'static str, counted: &Counted| {
        expect_ok(counted, name);
        rows.push((name, counted.counter, counted.kernel));
    };

    // commit: one commit of FILES new files.
    let committed = support::unique_repo("p1-commit");
    support::init(&committed);
    for index in 0..FILES {
        std::fs::write(
            committed.join(format!("f{index}.txt")),
            format!("line {index}\n"),
        )
        .unwrap();
    }
    record(
        "commit",
        &counted(
            &committed,
            &author_env(),
            &["commit", "--ref", "heads/main", "-m", "many"],
        ),
    );

    // seal: FILES queued commits, one seal.
    let sealed = support::unique_repo("p1-seal");
    support::init(&sealed);
    queue_commits(&sealed);
    support::trust_maintainer(&sealed);
    record(
        "seal",
        &counted(
            &sealed,
            &maintainer_env(),
            &["seal", "--allow-no-audit", "--ref", "heads/main"],
        ),
    );

    // the readers, on the sealed repository.
    record("status", &counted(&sealed, &[], &["status"]));
    record("verify", &counted(&sealed, &[], &["verify"]));
    record("log", &counted(&sealed, &[], &["log"]));

    // bundle import, into a fresh repository.
    let bundle = std::env::temp_dir().join(format!("p1-{}.bundle", support::unique_suffix()));
    support::ok(
        &support::prikk(&sealed)
            .args(["bundle", "export", "--ref", "heads/main", "--output"])
            .arg(&bundle)
            .output()
            .unwrap(),
        "bundle export",
    );
    let bundle_target = support::unique_repo("p1-bundle-import");
    support::init(&bundle_target);
    record(
        "bundle import",
        &counted(
            &bundle_target,
            &[],
            &["bundle", "import", "--input", bundle.to_str().unwrap()],
        ),
    );

    // archive import, into a fresh repository.
    let archive = std::env::temp_dir().join(format!("p1-{}.archive", support::unique_suffix()));
    support::ok(
        &support::prikk(&sealed)
            .args(["archive", "export"])
            .arg(&archive)
            .output()
            .unwrap(),
        "archive export",
    );
    let archive_target = support::unique_repo("p1-archive-import");
    support::init(&archive_target);
    record(
        "archive import",
        &counted(
            &archive_target,
            &[],
            &["archive", "import", archive.to_str().unwrap()],
        ),
    );

    // sync accept and sync seal: the receiver asks (have), the sealed repository answers (build), the receiver accepts, then seals.
    let receiver = support::unique_repo("p1-sync");
    support::init(&receiver);
    let have = std::env::temp_dir().join(format!("p1-{}.have", support::unique_suffix()));
    support::ok(
        &support::prikk(&receiver)
            .args(["sync", "have", "heads/main", "--output"])
            .arg(&have)
            .output()
            .unwrap(),
        "sync have",
    );
    let artifact = std::env::temp_dir().join(format!("p1-{}.artifact", support::unique_suffix()));
    let mut build = support::prikk(&sealed);
    for (key, value) in maintainer_env() {
        build.env(key, value);
    }
    support::ok(
        &build
            .args(["sync", "build", "heads/main", "--have"])
            .arg(&have)
            .arg("--output")
            .arg(&artifact)
            .output()
            .unwrap(),
        "sync build",
    );
    let accepted = counted(
        &receiver,
        &[],
        &["sync", "accept", artifact.to_str().unwrap()],
    );
    let claim = String::from_utf8_lossy(&accepted.output.stdout)
        .lines()
        .find_map(|line| {
            line.trim_start()
                .strip_prefix("claim ")
                .and_then(|rest| rest.split(':').next())
                .map(|id| id.trim().to_string())
        })
        .expect("the accepted claim");
    record("sync accept", &accepted);
    support::trust_maintainer(&receiver);
    record(
        "sync seal",
        &counted(
            &receiver,
            &maintainer_env(),
            &["sync", "seal", "heads/main", "--claim", &claim],
        ),
    );

    for (name, counter, kernel) in &rows {
        eprintln!("P1-ROW {name}: counter={counter} kernel={kernel:?}");
    }
    match strace_available() {
        Ok(()) => {
            for (name, counter, kernel) in &rows {
                assert_eq!(
                    Some(*counter),
                    *kernel,
                    "{name}: the counter and strace -f -e trace=fsync,fdatasync disagree"
                );
            }
        }
        Err(reason) => eprintln!("kernel agreement skipped: {reason}"),
    }
    let pinned: &[(&str, usize)] = PINNED;
    let measured: Vec<(&str, usize)> = rows
        .iter()
        .map(|(name, counter, _)| (*name, *counter))
        .collect();
    assert_eq!(
        measured, pinned,
        "the counter per operation kind on these fixtures ({FILES} files / commits)"
    );
}

/// Counter per operation kind on this file's fixtures. P2's baseline: each of these is what a directory opened once per held lock
/// is measured against.
const PINNED: &[(&str, usize)] = &[
    ("commit", 89),
    ("seal", 186),
    ("status", 1),
    ("verify", 1),
    ("log", 1),
    ("bundle import", 141),
    ("archive import", 164),
    ("sync accept", 121),
    ("sync seal", 95),
];

//! RFC 155 implementation Part E's own required measurement: `prikk archive export`'s peak RSS,
//! bounded independently of repository size (RFC 155 §9.2/§9.3). Measured at the release-gate node
//! counts this project already uses elsewhere (`RELEASE_GATE_NODE_COUNTS`,
//! `rfc133_node_count_memory.rs:286`: `[100, 32_000, 64_000]`) and at roughly 1 GiB of blob content,
//! `SAMPLES_PER_POINT` times each per this project's own "one sample is not a measurement" rule.
//!
//! Peak RSS comes from `getrusage(RUSAGE_CHILDREN)` via `tests/support/rusage_child.py`, the same
//! method `rfc133_node_count_memory.rs`/`rfc158_incoming_bound.rs` already use -- Linux-only (the
//! same reason those two are), gated accordingly.
//!
//! `#[ignore]`d like every other heavy measurement instrument in this crate: run deliberately with
//! `cargo test -p prikk --release --locked --test rfc155_archive_export_memory -- --ignored
//! --nocapture`, inside an R1 cgroup scope.

#![cfg(target_os = "linux")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Output;

const SAMPLES_PER_POINT: usize = 3;
const RELEASE_GATE_NODE_COUNTS: [usize; 3] = [100, 32_000, 64_000];

#[cfg(target_os = "linux")]
const RUSAGE_CHILD_SCRIPT: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rusage_child.py");

#[cfg(target_os = "linux")]
struct MeasuredRun {
    exit_code: Option<i32>,
    peak_rss_kib: i64,
    stderr: String,
}

#[cfg(target_os = "linux")]
fn measure_rss(cwd: &Path, args: &[&str], envs: &[(&str, &str)]) -> MeasuredRun {
    let mut command = std::process::Command::new("python3");
    command
        .arg(RUSAGE_CHILD_SCRIPT)
        .arg(cwd)
        .arg(env!("CARGO_BIN_EXE_prikk"))
        .args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    let output: Output = command
        .output()
        .expect("spawning rusage_child.py -- is python3 on PATH?");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let mut lines = stdout.splitn(2, '\n');
    let rss_line = lines.next().unwrap_or_default();
    let peak_rss_kib: i64 = rss_line.trim().parse().unwrap_or_else(|err| {
        panic!(
            "rusage_child.py's first stdout line {rss_line:?} is not an integer: {err}; \
             stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    MeasuredRun {
        exit_code: output.status.code(),
        peak_rss_kib,
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    }
}

/// Deterministically populate `root` with `file_count` small files, spread across subdirectories
/// (1,000 files per directory) so no single directory gets large enough to slow the filesystem
/// itself down at the top node count.
fn generate_flat_files(root: &Path, file_count: usize) {
    const PER_DIR: usize = 1000;
    for index in 0..file_count {
        let dir = root.join(format!("d{}", index / PER_DIR));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("f{index}.txt")), format!("file {index}\n")).unwrap();
    }
}

fn median(mut samples: Vec<i64>) -> i64 {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

/// `total_bytes` of real (non-sparse) content, spread across several files each under
/// `DEFAULT_BUNDLE_MAX_OBJECT_BYTES` (256 MiB) -- the per-object ceiling `bundle`/`archive`
/// `verify`/`import` already enforce on untrusted input (RFC 158 Stage A §1), which a single
/// object over that size cannot pass even though an ordinary local `commit` has no such ceiling
/// of its own. Release 0.51.0 prep P2 found this directly: the export-only memory test's own single
/// 1 GiB blob exports fine (export only streams bytes, never re-bounding an object's declared
/// size) but `archive verify`/`import` refuse it with `ObjectOverBound`, consistently with how
/// `bundle` already treats any object that size. Multiple blobs, each under the ceiling, is the
/// correct "about 1 GiB of blobs" (plural) shape for a verify/import measurement.
fn write_many_blobs_totaling(repo: &Path, total_bytes: usize, per_blob_bytes: usize) {
    let mut written = 0usize;
    let mut blob_index = 0usize;
    while written < total_bytes {
        let this_blob_bytes = per_blob_bytes.min(total_bytes - written);
        let mut content = vec![0_u8; this_blob_bytes];
        for (index, byte) in content.iter_mut().enumerate() {
            *byte = ((index + blob_index) % 251) as u8;
        }
        std::fs::write(repo.join(format!("big-{blob_index}.bin")), &content).unwrap();
        written += this_blob_bytes;
        blob_index += 1;
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "RFC 155 implementation Part E: measures archive export's peak RSS; run deliberately, see module docs"]
fn archive_export_rss_at_release_gate_node_counts() {
    let mut rows = Vec::new();
    for &count in &RELEASE_GATE_NODE_COUNTS {
        let repo = support::unique_repo(&format!("rfc155-archive-rss-{count}"));
        support::init(&repo);
        generate_flat_files(&repo, count);
        support::ok(
            &support::commit(&repo, "heads/main", "node-count corpus"),
            "commit",
        );
        support::ok(&support::seal(&repo, "heads/main"), "seal");

        let mut peaks = Vec::new();
        for sample in 0..SAMPLES_PER_POINT {
            let output_path = repo.join(format!("out-{sample}.prepo"));
            let run = measure_rss(
                &repo,
                &[
                    "archive",
                    "export",
                    output_path.file_name().unwrap().to_str().unwrap(),
                ],
                &[],
            );
            assert_eq!(
                run.exit_code,
                Some(0),
                "export must succeed at N={count}: {}",
                run.stderr
            );
            peaks.push(run.peak_rss_kib);
            let _ = std::fs::remove_file(&output_path);
        }
        let median_kib = median(peaks.clone());
        eprintln!("N={count}: peak RSS samples (KiB) = {peaks:?}, median = {median_kib}");
        rows.push((count, median_kib, peaks));
        let _ = std::fs::remove_dir_all(&repo);
    }

    // The bound this measurement exists to show: peak RSS stays small and does not grow
    // proportionally with node count. 256 MiB is a generous ceiling -- far above what streaming
    // export should ever need for files this small -- chosen to catch a gross regression (e.g. a
    // reintroduced whole-file buffer) without being a flaky tight bound.
    const CEILING_KIB: i64 = 256 * 1024;
    for (count, median_kib, _) in &rows {
        assert!(
            *median_kib < CEILING_KIB,
            "N={count}: median peak RSS {median_kib} KiB exceeds the {CEILING_KIB} KiB ceiling"
        );
    }

    let mut report =
        String::from("# RFC 155 archive export -- peak RSS at release-gate node counts\n\n");
    report.push_str(&format!(
        "Generated by `cargo test -p prikk --release --locked --test rfc155_archive_export_memory -- \
         --ignored --nocapture`. {SAMPLES_PER_POINT} samples per point, median shown; peak RSS via \
         `getrusage(RUSAGE_CHILDREN)`.\n\n| N | median peak RSS (KiB) | samples (KiB) |\n|---:|---:|---|\n"
    ));
    for (count, median_kib, peaks) in &rows {
        report.push_str(&format!("| {count} | {median_kib} | {peaks:?} |\n"));
    }
    let out_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.git-exclude/measurements/rfc155");
    std::fs::create_dir_all(&out_dir).expect("creating the measurement directory");
    std::fs::write(out_dir.join("impl-E-export-rss-release-gate.md"), &report).unwrap();
    eprintln!("{report}");
}

/// A real (not sparse) ~1 GiB blob, committed and sealed, then exported -- the byte-heavy case the
/// design round measured for the streamed-copy path (D3's own 775 ms/flat-RSS figure, warm cache)
/// but never through the real `archive export` command until now.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "RFC 155 implementation Part E: measures archive export's peak RSS on ~1 GiB of real blob content; run deliberately"]
fn archive_export_rss_at_one_gib_of_blob_content() {
    let repo = support::unique_repo("rfc155-archive-rss-1gib");
    support::init(&repo);
    const ONE_GIB: usize = 1024 * 1024 * 1024;
    let mut content = vec![0_u8; ONE_GIB];
    // Deterministic, non-trivial content -- not all-zero, so the OS cannot special-case it as a
    // hole, and not so expensive to generate that this instrument's own setup dominates its cost.
    for (index, byte) in content.iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }
    std::fs::write(repo.join("big.bin"), &content).unwrap();
    drop(content);
    support::ok(
        &support::commit(&repo, "heads/main", "1 GiB blob"),
        "commit",
    );
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let mut peaks = Vec::new();
    let mut wall_ms = Vec::new();
    for sample in 0..SAMPLES_PER_POINT {
        let output_path = repo.join(format!("out-{sample}.prepo"));
        let start = std::time::Instant::now();
        let run = measure_rss(
            &repo,
            &[
                "archive",
                "export",
                output_path.file_name().unwrap().to_str().unwrap(),
            ],
            &[],
        );
        wall_ms.push(start.elapsed().as_millis());
        assert_eq!(
            run.exit_code,
            Some(0),
            "export must succeed: {}",
            run.stderr
        );
        peaks.push(run.peak_rss_kib);
        let _ = std::fs::remove_file(&output_path);
    }
    let median_kib = median(peaks.clone());
    eprintln!(
        "1 GiB blob: peak RSS samples (KiB) = {peaks:?}, median = {median_kib}, wall (ms) = {wall_ms:?}"
    );

    // A generous ceiling for the same reason as the node-count test: this is nowhere near 1 GiB,
    // which is exactly the property "streamed, memory independent of size" claims.
    const CEILING_KIB: i64 = 256 * 1024;
    assert!(
        median_kib < CEILING_KIB,
        "median peak RSS {median_kib} KiB exceeds the {CEILING_KIB} KiB ceiling for a 1 GiB export"
    );

    let report = format!(
        "# RFC 155 archive export -- peak RSS at ~1 GiB of blob content\n\n\
         Generated by `cargo test -p prikk --release --locked --test rfc155_archive_export_memory -- \
         --ignored --nocapture`. {SAMPLES_PER_POINT} samples, median shown.\n\n\
         | peak RSS samples (KiB) | median (KiB) | wall samples (ms) |\n|---|---:|---|\n\
         | {peaks:?} | {median_kib} | {wall_ms:?} |\n"
    );
    let out_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.git-exclude/measurements/rfc155");
    std::fs::create_dir_all(&out_dir).expect("creating the measurement directory");
    std::fs::write(out_dir.join("impl-E-export-rss-1gib.md"), &report).unwrap();
    eprintln!("{report}");
    let _ = std::fs::remove_dir_all(&repo);
}

/// Release 0.51.0 prep, P2 item 2: `archive verify`'s own peak RSS on the same ~1 GiB blob
/// archive export already measures above -- verify has never been measured for memory before this
/// part. One export (unmeasured setup), then `SAMPLES_PER_POINT` measured `archive verify` runs
/// against the same archive file.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "Release 0.51.0 prep P2: measures archive verify's peak RSS on ~1 GiB of real blob content; run deliberately"]
fn archive_verify_rss_at_one_gib_of_blob_content() {
    let repo = support::unique_repo("rfc155-archive-verify-rss-1gib");
    support::init(&repo);
    const ONE_GIB: usize = 1024 * 1024 * 1024;
    const PER_BLOB: usize = 128 * 1024 * 1024;
    write_many_blobs_totaling(&repo, ONE_GIB, PER_BLOB);
    support::ok(
        &support::commit(&repo, "heads/main", "~1 GiB across several blobs"),
        "commit",
    );
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let archive_path = repo.join("source.prepo");
    let export_status = std::process::Command::new(env!("CARGO_BIN_EXE_prikk"))
        .current_dir(&repo)
        .args(["archive", "export", "source.prepo"])
        .status()
        .expect("spawning archive export for the verify fixture");
    assert!(
        export_status.success(),
        "archive export must succeed to build the verify fixture"
    );
    let archive_size_bytes = std::fs::metadata(&archive_path).unwrap().len();

    let mut peaks = Vec::new();
    let mut wall_ms = Vec::new();
    for _ in 0..SAMPLES_PER_POINT {
        let start = std::time::Instant::now();
        let run = measure_rss(&repo, &["archive", "verify", "source.prepo"], &[]);
        wall_ms.push(start.elapsed().as_millis());
        assert_eq!(
            run.exit_code,
            Some(0),
            "verify must succeed: {}",
            run.stderr
        );
        peaks.push(run.peak_rss_kib);
    }
    let median_kib = median(peaks.clone());
    let archive_size_kib = (archive_size_bytes / 1024) as i64;
    eprintln!(
        "1 GiB archive ({archive_size_bytes} bytes): verify peak RSS samples (KiB) = {peaks:?}, \
         median = {median_kib}, wall (ms) = {wall_ms:?}"
    );

    // §9.4's own guarantee is bounded by the single largest object in the archive (itself capped
    // at `DEFAULT_BUNDLE_MAX_OBJECT_BYTES`, 256 MiB, by RFC 158 Stage A's own untrusted-input
    // policy -- found directly in this part: a single object over that size cannot even reach
    // this test, refused by `ObjectOverBound` before any RSS is measured), not by the archive's
    // own total size. A ceiling equal to the per-object bound itself leaves no room for a
    // transient decode copy, so this uses a multiple of it instead -- generous enough not to be
    // flaky, still far below the archive's own total size, so a real "whole archive held in
    // memory" regression (this part's own prohibited shortcut) is still caught.
    const PER_BLOB_KIB: i64 = (PER_BLOB / 1024) as i64;
    const CEILING_KIB: i64 = PER_BLOB_KIB * 4;
    assert!(
        median_kib < CEILING_KIB,
        "median peak RSS {median_kib} KiB exceeds the {CEILING_KIB} KiB ceiling (4x the largest \
         single object, {PER_BLOB_KIB} KiB) for verifying a \
         ~1 GiB archive"
    );
    assert!(
        median_kib < archive_size_kib / 2,
        "median peak RSS {median_kib} KiB is not clearly independent of the archive's own size \
         ({archive_size_kib} KiB) -- expected well under half of it"
    );

    let report = format!(
        "# RFC 155 archive verify -- peak RSS at ~1 GiB of blob content\n\n\
         Generated by `cargo test -p prikk --release --locked --test rfc155_archive_export_memory -- \
         --ignored --nocapture`. {SAMPLES_PER_POINT} samples, median shown. Archive size: \
         {archive_size_bytes} bytes.\n\n\
         | peak RSS samples (KiB) | median (KiB) | wall samples (ms) |\n|---|---:|---|\n\
         | {peaks:?} | {median_kib} | {wall_ms:?} |\n"
    );
    let out_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.git-exclude/measurements/rfc155");
    std::fs::create_dir_all(&out_dir).expect("creating the measurement directory");
    std::fs::write(
        out_dir.join("release-0.51.0-prep-verify-rss-1gib.md"),
        &report,
    )
    .unwrap();
    eprintln!("{report}");
    let _ = std::fs::remove_dir_all(&repo);
}

/// Release 0.51.0 prep, P2 item 2: `archive import`'s own peak RSS on the same ~1 GiB blob
/// archive -- never measured before this part. The prohibited shortcut this part exists to catch
/// is holding the whole archive in memory, so each sample imports into a **fresh** destination
/// repository and peak RSS is compared against the archive's own on-disk size, not only against
/// the same fixed ceiling the export/verify tests use.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "Release 0.51.0 prep P2: measures archive import's peak RSS on ~1 GiB of real blob content; run deliberately"]
fn archive_import_rss_at_one_gib_of_blob_content() {
    let source_repo = support::unique_repo("rfc155-archive-import-rss-1gib-source");
    support::init(&source_repo);
    const ONE_GIB: usize = 1024 * 1024 * 1024;
    const PER_BLOB: usize = 128 * 1024 * 1024;
    write_many_blobs_totaling(&source_repo, ONE_GIB, PER_BLOB);
    support::ok(
        &support::commit(&source_repo, "heads/main", "~1 GiB across several blobs"),
        "commit",
    );
    support::ok(&support::seal(&source_repo, "heads/main"), "seal");

    let archive_path = source_repo.join("source.prepo");
    let export_status = std::process::Command::new(env!("CARGO_BIN_EXE_prikk"))
        .current_dir(&source_repo)
        .args(["archive", "export", "source.prepo"])
        .status()
        .expect("spawning archive export for the import fixture");
    assert!(
        export_status.success(),
        "archive export must succeed to build the import fixture"
    );
    let archive_size_bytes = std::fs::metadata(&archive_path).unwrap().len();
    let archive_size_kib = (archive_size_bytes / 1024) as i64;

    let mut peaks = Vec::new();
    let mut wall_ms = Vec::new();
    for sample in 0..SAMPLES_PER_POINT {
        let dest_repo =
            support::unique_repo(&format!("rfc155-archive-import-rss-1gib-dest-{sample}"));
        support::init(&dest_repo);
        let start = std::time::Instant::now();
        let run = measure_rss(
            &dest_repo,
            &["archive", "import", archive_path.to_str().unwrap()],
            &[],
        );
        wall_ms.push(start.elapsed().as_millis());
        assert_eq!(
            run.exit_code,
            Some(0),
            "import must succeed: {}",
            run.stderr
        );
        peaks.push(run.peak_rss_kib);
        let _ = std::fs::remove_dir_all(&dest_repo);
    }
    let median_kib = median(peaks.clone());
    eprintln!(
        "1 GiB archive ({archive_size_bytes} bytes = {archive_size_kib} KiB): import peak RSS \
         samples (KiB) = {peaks:?}, median = {median_kib}, wall (ms) = {wall_ms:?}"
    );

    // The bound this test exists to prove: import's own peak RSS must not scale with the
    // archive's own size (the "prohibited shortcut" this part's own handoff names: holding the
    // whole archive in memory) -- it is bounded by the single largest object instead (itself
    // capped at `DEFAULT_BUNDLE_MAX_OBJECT_BYTES`, see the verify test's own comment for why a
    // ceiling equal to that bound itself would leave no room for decode overhead), plus an
    // explicit comparison against the archive's own size so a future regression that merely stays
    // under the per-object ceiling (but now scales with archive size) is still visible.
    const PER_BLOB_KIB: i64 = (PER_BLOB / 1024) as i64;
    const CEILING_KIB: i64 = PER_BLOB_KIB * 4;
    assert!(
        median_kib < CEILING_KIB,
        "median peak RSS {median_kib} KiB exceeds the {CEILING_KIB} KiB ceiling (4x the largest \
         single object, {PER_BLOB_KIB} KiB) for importing a ~1 GiB archive"
    );
    assert!(
        median_kib < archive_size_kib / 4,
        "median peak RSS {median_kib} KiB is not clearly bounded independently of the archive's \
         own size ({archive_size_kib} KiB) -- expected well under a quarter of it"
    );

    let report = format!(
        "# RFC 155 archive import -- peak RSS at ~1 GiB of blob content\n\n\
         Generated by `cargo test -p prikk --release --locked --test rfc155_archive_export_memory -- \
         --ignored --nocapture`. {SAMPLES_PER_POINT} samples, median shown. Archive size: \
         {archive_size_bytes} bytes ({archive_size_kib} KiB).\n\n\
         | peak RSS samples (KiB) | median (KiB) | wall samples (ms) |\n|---|---:|---|\n\
         | {peaks:?} | {median_kib} | {wall_ms:?} |\n"
    );
    let out_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.git-exclude/measurements/rfc155");
    std::fs::create_dir_all(&out_dir).expect("creating the measurement directory");
    std::fs::write(
        out_dir.join("release-0.51.0-prep-import-rss-1gib.md"),
        &report,
    )
    .unwrap();
    eprintln!("{report}");
    let _ = std::fs::remove_dir_all(&source_repo);
}

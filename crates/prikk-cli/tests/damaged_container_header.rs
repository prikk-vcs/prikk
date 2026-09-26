//! RFC 102, the append-length round, Addendum 1 §1: a container frame whose header claims an absurd length is a **finding**, never an
//! allocation. An object read used to take the frame length from the header on disk and size a buffer from it; a damaged header made
//! `prikk verify` abort with `memory allocation of N bytes failed`. It is now an integrity error that `verify` reports.

mod support;

use std::path::Path;

/// Set the first frame of `repo`'s blob container to claim a body of `claimed` bytes (the big-endian `u64` at bytes 10..18 of a frame).
fn claim_body_length(repo: &Path, claimed: u64) {
    let container = repo.join(".prikk/containers/blob/a.container");
    let mut bytes = std::fs::read(&container).expect("the blob container");
    bytes[10..18].copy_from_slice(&claimed.to_be_bytes());
    std::fs::write(&container, bytes).expect("writing the damaged container");
}

fn sealed_repository(tag: &str) -> std::path::PathBuf {
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
    support::ok(&support::seal(&repo, "heads/main"), "seal");
    repo
}

/// **The CLI control.** A *sealed* repository whose blob header claims 2^62 bytes, and then 2^40: `prikk verify` reports the damage and
/// exits non-zero (an exit status, not a signal), with no allocation failure. (0.47.0's `verify` on the same repository is stated in the
/// round's report.) *Perturb: remove both the reader's clamp and the length comparison in `read_object_envelope_at`: `verify` aborts
/// (`memory allocation of … bytes failed`, SIGABRT) and this goes red.*
#[test]
fn verify_reports_a_damaged_frame_header_and_does_not_abort() {
    for claimed in [1_u64 << 62, 1_u64 << 40, 1_u64 << 20] {
        let repo = sealed_repository("damaged-header");
        claim_body_length(&repo, claimed);
        let output = support::prikk(&repo).arg("verify").output().unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.status.code().is_some(),
            "claimed {claimed}: verify ended by a signal, not an exit status: {text}"
        );
        assert_ne!(
            output.status.code(),
            Some(0),
            "claimed {claimed}: verify reports the damage: {text}"
        );
        assert!(
            !text.contains("memory allocation"),
            "claimed {claimed}: no allocation was attempted: {text}"
        );
        eprintln!("CLAIMED {claimed}: exit {:?}\n{text}", output.status.code());
        // For the round's report only: what an older binary (`PRIKK_APPEND_BASELINE_BINARY`, 0.47.0's) says of the same repository.
        if let Ok(baseline) = std::env::var("PRIKK_APPEND_BASELINE_BINARY") {
            let mut command = std::process::Command::new(baseline);
            command.current_dir(&repo).arg("verify");
            support::isolate_key_environment_for(&mut command, Some(&repo));
            let old = command.output().unwrap();
            eprintln!(
                "BASELINE {claimed}: exit {:?}, signal-killed {}\n{}{}",
                old.status.code(),
                old.status.code().is_none(),
                String::from_utf8_lossy(&old.stdout),
                String::from_utf8_lossy(&old.stderr)
            );
        }
        let _ = std::fs::remove_dir_all(repo);
    }
}

// SPDX-License-Identifier: Apache-2.0
// adapted from the external review kit for prikk (reviews 053 and 055): `lib053.sh`'s `mk()` and K1's `shapes.sh`, in Rust.
//! One function per shape: it builds the repository state a writer exists for, and says what "sound" means for it (the case table's *crash* row).
//! Every state is built through the CLI (`setup`, `commit`, `seal`, and the damage a repair verb repairs, written as bytes), so a shape is what a user
//! could have on disk.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::panic
)]

use super::harness::{Ctx, Shape, args, copy_tree, snapshot, text};
use std::path::{Path, PathBuf};

fn shape(
    name: &'static str,
    base: PathBuf,
    argv: Vec<String>,
    check: impl Fn(&Ctx, &Path) -> bool + 'static,
) -> Shape {
    Shape {
        name,
        base,
        argv,
        check: Box::new(check),
        verify_may_fail: false,
        follow_named: true,
        program: None,
    }
}

const FIXTURE_SEED: u8 = 0xA5;

fn append(path: &Path, n: usize) {
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(&vec![FIXTURE_SEED; n])
        .unwrap();
}

fn first_with_prefix(dir: &Path, prefix: &str) -> PathBuf {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|f| f.to_string_lossy().starts_with(prefix))
        })
        .collect();
    found.sort();
    found.remove(0)
}

fn pointer_index(dir: &Path) -> PathBuf {
    first_with_prefix(&dir.join(".prikk/refs/containers"), "pointer-index-a")
}

fn three_files(ctx: &Ctx, dir: &Path) {
    for i in 1..=3 {
        std::fs::write(dir.join(format!("f{i}.txt")), format!("f {i}\n")).unwrap();
    }
    let _ = ctx;
}

pub fn commit(ctx: &Ctx) -> Shape {
    let base = ctx.repo("base-commit");
    three_files(ctx, &base);
    shape(
        "commit",
        base,
        args(&["commit", "--from-worktree", "-m", "k"]),
        |ctx, d| {
            ctx.exit(d, &["worktree-status"]) == 0
                && text(&ctx.out(d, &["status"])).lines().any(|l| {
                    l.starts_with("queued patches: ") && !l.starts_with("queued patches: 0")
                })
        },
    )
}

pub fn seal(ctx: &Ctx) -> Shape {
    let base = ctx.repo("base-seal");
    three_files(ctx, &base);
    ctx.ok(&base, &["commit", "--from-worktree", "-m", "k"]);
    shape(
        "seal",
        base,
        args(&["seal", "--allow-no-audit"]),
        |ctx, d| ctx.tree_files(d) == 3,
    )
}

pub fn seal_checkpoint(ctx: &Ctx) -> Shape {
    let base = ctx.repo("base-cp");
    ctx.gens(&base, 63, true, 1);
    std::fs::write(base.join("s.txt"), "step 64\n").unwrap();
    ctx.ok(&base, &["commit", "--from-worktree", "-m", "s64"]);
    shape(
        "seal-checkpoint",
        base,
        args(&["seal", "--allow-no-audit"]),
        |ctx, d| ctx.tree_files(d) == 1,
    )
}

pub fn sync_seal(ctx: &Ctx) -> Shape {
    let source = ctx.repo("src-sync");
    ctx.gens(&source, 3, true, 1);
    let base = ctx.repo("base-sync");
    let have = ctx.root.join("have.bin");
    let art = ctx.root.join("art.bin");
    let claims = ctx.root.join("claims.txt");
    ctx.ok(
        &base,
        &[
            "sync",
            "have",
            "heads/main",
            "--output",
            have.to_str().unwrap(),
        ],
    );
    ctx.ok(
        &source,
        &[
            "sync",
            "build",
            "heads/main",
            "--have",
            have.to_str().unwrap(),
            "--output",
            art.to_str().unwrap(),
            "--force",
        ],
    );
    ctx.ok(
        &base,
        &[
            "sync",
            "accept",
            art.to_str().unwrap(),
            "--claims-out",
            claims.to_str().unwrap(),
        ],
    );
    shape(
        "sync-seal-batch",
        base,
        args(&[
            "sync",
            "seal",
            "heads/main",
            "--claims",
            claims.to_str().unwrap(),
        ]),
        |ctx, d| ctx.tree_files(d) == 1,
    )
}

pub fn uncommit(ctx: &Ctx) -> Shape {
    let base = ctx.repo("base-uncommit");
    std::fs::write(base.join("one.txt"), "one\n").unwrap();
    ctx.ok(&base, &["commit", "--from-worktree", "-m", "one"]);
    std::fs::write(base.join("two.txt"), "two\n").unwrap();
    let out = ctx.ok(&base, &["commit", "--from-worktree", "-m", "two"]);
    let id = first_hex64(&out);
    shape("uncommit", base, args(&["uncommit", &id]), |ctx, d| {
        // The commit is taken back and the files stay, so the next commit from the worktree must go through and queue them.
        ctx.tree_files(d) == 0
            && ctx.exit(d, &["commit", "--from-worktree", "-m", "again"]) == 0
            && text(&ctx.out(d, &["status"]))
                .lines()
                .any(|l| l.starts_with("queued patches: ") && !l.starts_with("queued patches: 0"))
    })
}

fn first_hex64(text: &str) -> String {
    text.split(|c: char| !c.is_ascii_hexdigit())
        .find(|w| w.len() == 64)
        .unwrap_or_else(|| panic!("no 64-hex id in {text}"))
        .to_string()
}

pub fn repair_wal_tail(ctx: &Ctx) -> Shape {
    let base = ctx.repo("base-wal");
    std::fs::write(base.join("a.txt"), "a\n").unwrap();
    ctx.ok(&base, &["commit", "--from-worktree", "-m", "a"]);
    append(&base.join(".prikk/active/default/queue.wal"), 100);
    shape(
        "repair-wal-tail",
        base,
        args(&["doctor", "--repair-wal-tail"]),
        |ctx, d| ctx.tree_files(d) == 0 && ctx.exit(d, &["doctor"]) == 0,
    )
}

fn two_gens_with_pointer_tail(ctx: &Ctx, name: &str) -> PathBuf {
    let base = ctx.repo(name);
    ctx.gens(&base, 2, false, 1);
    append(&pointer_index(&base), 50);
    base
}

pub fn repair_tails(ctx: &Ctx) -> Shape {
    let base = two_gens_with_pointer_tail(ctx, "base-tails");
    shape(
        "repair-tails",
        base,
        args(&["doctor", "--repair-tails"]),
        |ctx, d| ctx.tree_files(d) == 2 && ctx.exit(d, &["doctor"]) == 0,
    )
}

pub fn repair_pointer_index_tail(ctx: &Ctx) -> Shape {
    let base = two_gens_with_pointer_tail(ctx, "base-ptail");
    shape(
        "repair-pointer-index-tail",
        base,
        args(&["doctor", "--repair-pointer-index-tail"]),
        |ctx, d| ctx.tree_files(d) == 2 && ctx.clean(d),
    )
}

pub fn repair_index(ctx: &Ctx) -> Shape {
    let base = ctx.repo("base-index");
    ctx.gens(&base, 2, false, 1);
    append(
        &first_with_prefix(&base.join(".prikk/containers"), "index"),
        50,
    );
    shape(
        "repair-index",
        base,
        args(&["doctor", "--repair-index"]),
        |ctx, d| ctx.tree_files(d) == 2 && ctx.clean(d),
    )
}

pub fn tag_create(ctx: &Ctx) -> Shape {
    let base = ctx.repo("base-tag");
    ctx.gens(&base, 1, true, 1);
    shape(
        "tag-create",
        base,
        args(&["tag", "create", "tags/k1", "--target", "heads/main"]),
        |ctx, d| text(&ctx.out(d, &["tag", "list"])).contains("tags/k1"),
    )
}

pub fn branch_create(ctx: &Ctx) -> Shape {
    let base = ctx.repo("base-branch");
    ctx.gens(&base, 1, true, 1);
    shape(
        "branch-create",
        base,
        args(&["branch", "create", "heads/k1", "--from", "heads/main"]),
        |ctx, d| text(&ctx.out(d, &["branch", "list"])).contains("heads/k1"),
    )
}

fn candidate_repo(ctx: &Ctx, name: &str) -> PathBuf {
    let base = ctx.repo(name);
    ctx.gens(&base, 1, true, 1);
    let tmp = base.join(".prikk/refs/tmp");
    std::fs::create_dir_all(tmp.join("e")).unwrap();
    let mut a = b"PREFPTR1".to_vec();
    a.extend_from_slice(&10u64.to_be_bytes());
    a.extend_from_slice(b"heads/main");
    a.extend_from_slice(&[0; 32]);
    std::fs::write(tmp.join("a.tmp"), a).unwrap();
    std::fs::write(tmp.join("b.tmp"), b"junk\n").unwrap();
    base
}

pub fn remove_ref_candidates(ctx: &Ctx) -> Shape {
    let base = candidate_repo(ctx, "base-rc");
    shape(
        "remove-ref-candidates",
        base,
        args(&["doctor", "--remove-ref-candidates"]),
        |ctx, d| {
            std::fs::read_dir(d.join(".prikk/refs/tmp")).map_or(true, |mut r| r.next().is_none())
                && ctx.exit(d, &["verify"]) == 0
        },
    )
}

/// A repair run (`repair`), then the sweep of `--recovery-restore <its run>`; the check compares every file's bytes with what they were before the repair.
fn restore_of(ctx: &Ctx, name: &'static str, base: PathBuf, repair: &[&str]) -> Shape {
    let before = snapshot(&base);
    let out = ctx.out(&base, repair);
    let list = text(&ctx.out(&base, &["doctor", "--recovery-list"]));
    let id = list
        .lines()
        .find_map(|l| {
            l.strip_prefix("run ")
                .map(|r| r.chars().take(16).collect::<String>())
        })
        .unwrap_or_else(|| {
            panic!(
                "{name}: the repair {repair:?} made no recovery run: {}{list}",
                text(&out)
            )
        });
    let mut s = shape(
        name,
        base,
        args(&["doctor", "--recovery-restore", &id]),
        move |_, d| snapshot(d) == before,
    );
    s.verify_may_fail = true;
    s
}

pub fn restore_cut(ctx: &Ctx) -> Shape {
    let base = two_gens_with_pointer_tail(ctx, "base-rs");
    restore_of(
        ctx,
        "recovery-restore (a Cut run)",
        base,
        &["doctor", "--repair-tails"],
    )
}

pub fn restore_remove(ctx: &Ctx) -> Shape {
    let base = candidate_repo(ctx, "base-rr");
    // The empty directory is removed with nothing saved, so it is not in what a restore brings back: compare files only.
    let before = snapshot_tmp_files(&base);
    let list_out = ctx.out(&base, &["doctor", "--remove-ref-candidates"]);
    assert!(list_out.status.success(), "{}", text(&list_out));
    let list = text(&ctx.out(&base, &["doctor", "--recovery-list"]));
    let id: String = list
        .lines()
        .find_map(|l| l.strip_prefix("run ").map(|r| r.chars().take(16).collect()))
        .expect("a run");
    let mut s = shape(
        "recovery-restore (a Remove run)",
        base,
        args(&["doctor", "--recovery-restore", &id]),
        move |_, d| snapshot_tmp_files(d) == before,
    );
    s.verify_may_fail = true;
    s
}

fn snapshot_tmp_files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    if let Ok(read) = std::fs::read_dir(dir.join(".prikk/refs/tmp")) {
        for e in read.flatten() {
            if e.file_type().unwrap().is_file() {
                out.push((
                    e.file_name().to_string_lossy().into_owned(),
                    std::fs::read(e.path()).unwrap(),
                ));
            }
        }
    }
    out.sort();
    out
}

pub fn recovery_clear(ctx: &Ctx) -> Shape {
    let base = two_gens_with_pointer_tail(ctx, "base-clear");
    ctx.ok(&base, &["doctor", "--repair-tails"]);
    shape(
        "recovery-clear",
        base,
        args(&["doctor", "--recovery-clear"]),
        |_, d| std::fs::metadata(d.join(".prikk/recovery/log")).map_or(true, |m| m.len() == 0),
    )
}

pub fn rebuild_pointer_index(ctx: &Ctx) -> Shape {
    let base = ctx.repo("base-rebuild");
    ctx.gens(&base, 1, true, 1);
    let g1 = ctx.root.join("rebuild.g1");
    let _ = std::fs::remove_dir_all(&g1);
    copy_tree(&base.join(".prikk"), &g1);
    ctx.gens(&base, 1, true, 2);
    std::fs::copy(
        g1.join("refs/containers/pointer-index-a.container"),
        base.join(".prikk/refs/containers/pointer-index-a.container"),
    )
    .unwrap();
    shape(
        "rebuild-pointer-index",
        base,
        args(&["doctor", "--rebuild-pointer-index"]),
        |ctx, d| ctx.tree_files(d) == 1 && ctx.exit(d, &["verify"]) == 0,
    )
}

pub fn rebuild_caches(ctx: &Ctx) -> Shape {
    let base = ctx.repo("base-caches");
    ctx.gens(&base, 2, true, 1);
    shape(
        "rebuild-caches",
        base,
        args(&["doctor", "--rebuild-caches"]),
        |ctx, d| ctx.tree_files(d) == 1 && ctx.clean(d),
    )
}

pub fn ref_complete(ctx: &Ctx) -> Shape {
    let base = ctx.repo("base-complete");
    ctx.gens(&base, 1, true, 1);
    let g1 = ctx.root.join("complete.g1");
    let _ = std::fs::remove_dir_all(&g1);
    copy_tree(&base.join(".prikk"), &g1);
    ctx.gens(&base, 1, true, 2);
    std::fs::copy(
        g1.join("refs/containers/log-a.container"),
        base.join(".prikk/refs/containers/log-a.container"),
    )
    .unwrap();
    shape(
        "ref-complete",
        base,
        args(&["ref", "complete", "heads/main"]),
        |ctx, d| ctx.tree_files(d) == 1 && ctx.exit(d, &["verify"]) == 0,
    )
}

fn damaged_commit_repo(ctx: &Ctx, name: &str) -> PathBuf {
    let base = ctx.repo(name);
    std::fs::write(base.join("a.txt"), "a\n").unwrap();
    ctx.ok(&base, &["commit", "--from-worktree", "-m", "a"]);
    let wal = base.join(".prikk/active/default/queue.wal");
    let mut bytes = std::fs::read(&wal).unwrap();
    let at = bytes.len() - 5;
    bytes[at] ^= 0xFF;
    std::fs::write(&wal, bytes).unwrap();
    base
}

pub fn discard_damaged_commits(ctx: &Ctx) -> Shape {
    let base = damaged_commit_repo(ctx, "base-discard");
    shape(
        "discard-damaged-commits",
        base,
        args(&["doctor", "--discard-damaged-commits"]),
        |ctx, d| ctx.exit(d, &["verify"]) == 0,
    )
}

fn orphaned_queue_repo(ctx: &Ctx, name: &str) -> PathBuf {
    let base = ctx.repo(name);
    std::fs::write(base.join("a.txt"), "a\n").unwrap();
    ctx.ok(&base, &["commit", "--from-worktree", "-m", "a"]);
    std::fs::remove_file(base.join(".prikk/active/default/ref-name")).unwrap();
    base
}

pub fn restore_queue_target(ctx: &Ctx) -> Shape {
    let base = orphaned_queue_repo(ctx, "base-rqt");
    shape(
        "restore-queue-target",
        base,
        args(&["doctor", "--restore-queue-target", "--ref", "heads/main"]),
        |ctx, d| ctx.exit(d, &["verify"]) == 0,
    )
}

pub fn compact_pointer_index(ctx: &Ctx) -> Shape {
    let base = ctx.repo("base-compact");
    ctx.gens(&base, 3, true, 1);
    shape(
        "compact --pointer-index",
        base,
        args(&["compact", "--pointer-index"]),
        |ctx, d| ctx.tree_files(d) == 1 && ctx.exit(d, &["verify"]) == 0,
    )
}

/// The W2 refusal: `branch switch` to a branch whose own queue holds a changed path. It writes no history, ref, queue or worktree byte, but it takes
/// the active lock and rewrites a derived cache, so it is swept (K1: 19 mutating syscalls).
pub fn switch_refused(ctx: &Ctx) -> Shape {
    let base = ctx.repo("base-w2");
    ctx.gens(&base, 1, false, 1);
    ctx.ok(
        &base,
        &["branch", "create", "heads/other", "--from", "heads/main"],
    );
    std::fs::write(base.join("g1.txt"), "changed\n").unwrap();
    ctx.ok(
        &base,
        &[
            "commit",
            "--ref",
            "heads/other",
            "--from-worktree",
            "-m",
            "x",
        ],
    );
    let mut s = shape(
        "branch-switch refused (W2)",
        base,
        args(&["branch", "switch", "heads/other"]),
        |ctx, d| {
            std::fs::read_to_string(d.join("g1.txt")).is_ok_and(|c| c == "changed\n")
                && text(&ctx.out(d, &["status"])).contains("active WAL records: 1")
                && ctx.exit(d, &["verify"]) == 0
        },
    );
    s.follow_named = false;
    s
}

pub fn cancel_import(ctx: &Ctx) -> Shape {
    use prikk_test_support::ptrace_sweep::{self, When};
    let source = ctx.repo("src-import");
    ctx.gens(&source, 3, false, 1);
    let archive = ctx.root.join("a.prepo");
    let _ = std::fs::remove_file(&archive);
    ctx.ok(&source, &["archive", "export", archive.to_str().unwrap()]);
    let base = ctx.repo("base-cancel");
    let sink = ctx.root.join("import-out.txt");
    let import = |dir: &Path| ctx.prikk(dir, &["archive", "import", archive.to_str().unwrap()]);
    // The import killed at about 60% of its mutating syscalls leaves a journal.
    let dry_dir = ctx.root.join("dry-import");
    let _ = std::fs::remove_dir_all(&dry_dir);
    copy_tree(&base, &dry_dir);
    let total = ptrace_sweep::trace(&import(&dry_dir), &sink, None, When::Entry)
        .unwrap()
        .mutating
        .len() as u64;
    let _ = std::fs::remove_dir_all(&dry_dir);
    ptrace_sweep::trace(&import(&base), &sink, Some(total * 6 / 10), When::Entry).unwrap();
    for lock in super::harness::lock_files(&base) {
        let _ = ctx.out(
            &base,
            &["unlock", "--lock", lock.to_str().unwrap(), "--force"],
        );
    }
    let doctor = text(&ctx.out(&base, &["doctor"]));
    let id = doctor
        .split("(id ")
        .nth(1)
        .map(|r| {
            r.chars()
                .take_while(char::is_ascii_hexdigit)
                .collect::<String>()
        })
        .unwrap_or_else(|| panic!("no interrupted import in: {doctor}"));
    shape(
        "cancel-import",
        base,
        args(&["doctor", "--cancel-import", &id]),
        |ctx, d| ctx.clean(d),
    )
}

// ---- the restores of every writer that saves a `Replace` (K2 ruling (D): the day a writer's saved bytes are shorter than, and not a prefix of, its
// new bytes, one of these goes red; that is the trigger for building (A)) ----------------------------------------------------------------------

fn restore_without_ref_name(
    ctx: &Ctx,
    name: &'static str,
    base: PathBuf,
    repair: &[&str],
) -> Shape {
    // `restore-queue-target` saves 0 bytes of `ref-name`: the restore leaves an empty file where the repair found none, and every reader treats the two
    // alike (measured), so the comparison skips that one path.
    let strip = |mut s: std::collections::BTreeMap<PathBuf, Vec<u8>>| {
        s.remove(Path::new("active/default/ref-name"));
        s
    };
    let before = strip(snapshot(&base));
    let out = ctx.out(&base, repair);
    let list = text(&ctx.out(&base, &["doctor", "--recovery-list"]));
    let id = list
        .lines()
        .find_map(|l| {
            l.strip_prefix("run ")
                .map(|r| r.chars().take(16).collect::<String>())
        })
        .unwrap_or_else(|| {
            panic!(
                "{name}: no recovery run after {repair:?}: {}{list}",
                text(&out)
            )
        });
    let mut s = shape(
        name,
        base,
        args(&["doctor", "--recovery-restore", &id]),
        move |_, d| strip(snapshot(d)) == before,
    );
    s.verify_may_fail = true;
    s
}

pub fn restore_of_uncommit(ctx: &Ctx) -> Shape {
    let base = ctx.repo("base-rs-uncommit");
    std::fs::write(base.join("one.txt"), "one\n").unwrap();
    ctx.ok(&base, &["commit", "--from-worktree", "-m", "one"]);
    std::fs::write(base.join("two.txt"), "two\n").unwrap();
    let id = first_hex64(&ctx.ok(&base, &["commit", "--from-worktree", "-m", "two"]));
    restore_of(
        ctx,
        "recovery-restore (an uncommit run: witness, baseline)",
        base,
        &["uncommit", &id],
    )
}

pub fn restore_of_discard(ctx: &Ctx) -> Shape {
    let base = damaged_commit_repo(ctx, "base-rs-discard");
    restore_of(
        ctx,
        "recovery-restore (a discard run: witness)",
        base,
        &["doctor", "--discard-damaged-commits"],
    )
}

pub fn restore_of_rebuild_shortened_slot(ctx: &Ctx) -> Shape {
    let base = ctx.repo("base-rs-rebuild");
    ctx.gens(&base, 3, false, 1);
    let path = pointer_index(&base);
    let mut bytes = std::fs::read(&path).unwrap();
    let n = bytes.len();
    for byte in &mut bytes[n / 3..n / 3 + 12] {
        *byte ^= 0xA5;
    }
    bytes.truncate(2 * n / 3);
    std::fs::write(&path, bytes).unwrap();
    restore_of(
        ctx,
        "recovery-restore (a rebuild run: a shortened slot)",
        base,
        &["doctor", "--rebuild-pointer-index"],
    )
}

pub fn restore_of_restore_queue_target(ctx: &Ctx) -> Shape {
    let base = orphaned_queue_repo(ctx, "base-rs-rqt");
    restore_without_ref_name(
        ctx,
        "recovery-restore (a restore-queue-target run: ref-name)",
        base,
        &["doctor", "--restore-queue-target", "--ref", "heads/main"],
    )
}

// ---- the other compactions ----------------------------------------------------------------------------------------------------------------

pub fn compact_received_index(ctx: &Ctx) -> Shape {
    let base = ctx.repo("base-compact-received");
    ctx.gens(&base, 2, true, 1);
    shape(
        "compact --received-index",
        base,
        args(&["compact", "--received-index"]),
        |ctx, d| ctx.clean(d),
    )
}

pub fn compact_trust_policy(ctx: &Ctx) -> Shape {
    let base = ctx.repo("base-compact-trust");
    ctx.gens(&base, 2, true, 1);
    shape(
        "compact --trust-policy",
        base,
        args(&["compact", "--trust-policy"]),
        |ctx, d| ctx.clean(d),
    )
}

// ---- the W3 producers' refusals (step W3's worlds): a queue owned by another branch holds a path the producer would change --------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Held {
    NewFile,
    Edit,
    Deletion,
}

pub const HELD: [Held; 3] = [Held::NewFile, Held::Edit, Held::Deletion];

fn queue_onto_feature(ctx: &Ctx, repo: &Path, held: Held, edit: &str, delete: &str) {
    match held {
        Held::NewFile => std::fs::write(repo.join("queued.txt"), "q\n").unwrap(),
        Held::Edit => {
            let mut bytes = std::fs::read(repo.join(edit)).unwrap();
            bytes.extend_from_slice(b"e\n");
            std::fs::write(repo.join(edit), bytes).unwrap();
        }
        Held::Deletion => std::fs::remove_file(repo.join(delete)).unwrap(),
    }
    ctx.ok(
        repo,
        &[
            "commit",
            "--ref",
            "heads/feature",
            "--from-worktree",
            "-m",
            "queued",
        ],
    );
}

fn whole_repo(dir: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    fn walk(base: &Path, dir: &Path, out: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let rel = path.strip_prefix(base).unwrap().to_path_buf();
            if rel == Path::new(".prikk/cache") || path.extension().is_some_and(|e| e == "lock") {
                continue;
            }
            if entry.file_type().unwrap().is_dir() {
                walk(base, &path, out);
            } else {
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn refusal_shape(_ctx: &Ctx, name: &'static str, base: PathBuf, argv: Vec<String>) -> Shape {
    let before = whole_repo(&base);
    let mut s = shape(name, base, argv, move |_, d| whole_repo(d) == before);
    s.follow_named = false;
    s
}

/// `merge --into heads/main --from heads/topic` while `heads/feature`'s queue holds a path the merge would change.
pub fn w3_merge(ctx: &Ctx, held: Held) -> Shape {
    let base = ctx.repo("base-w3-merge");
    for (file, content) in [("a.txt", "base\n"), ("k.txt", "keep\n")] {
        std::fs::write(base.join(file), content).unwrap();
        ctx.ok(&base, &["commit", "--from-worktree", "-m", file]);
        ctx.ok(&base, &["seal", "--allow-no-audit"]);
    }
    let log = ctx.ok(&base, &["log", "--ref", "heads/main"]);
    let baseline = log
        .lines()
        .find_map(|l| {
            l.strip_prefix("block ")
                .map(|r| r.chars().take(64).collect::<String>())
        })
        .expect("a block line");
    ctx.ok(
        &base,
        &["branch", "create", "heads/topic", "--from", "heads/main"],
    );
    ctx.ok(
        &base,
        &["branch", "create", "heads/feature", "--from", "heads/main"],
    );
    std::fs::write(base.join("t.txt"), "t\n").unwrap();
    ctx.ok(
        &base,
        &[
            "commit",
            "--ref",
            "heads/topic",
            "--from-worktree",
            "-m",
            "topic",
        ],
    );
    ctx.ok(&base, &["seal", "--allow-no-audit", "--ref", "heads/topic"]);
    std::fs::remove_file(base.join("t.txt")).unwrap();
    queue_onto_feature(ctx, &base, held, "a.txt", "k.txt");
    refusal_shape(
        ctx,
        "W3 refusal: merge --into",
        base,
        args(&[
            "merge",
            "--allow-no-audit",
            "--baseline-block",
            &baseline,
            "--into",
            "heads/main",
            "--from",
            "heads/topic",
        ]),
    )
}

fn adopt_world(ctx: &Ctx, name: &str, held: Held) -> (PathBuf, PathBuf, PathBuf) {
    let source = ctx.repo(&format!("{name}-src"));
    std::fs::write(source.join("n.txt"), "n\n").unwrap();
    ctx.ok(&source, &["commit", "--from-worktree", "-m", "genesis"]);
    ctx.ok(&source, &["seal", "--allow-no-audit"]);
    let archive = ctx.root.join(format!("{name}.prepo"));
    let bundle = ctx.root.join(format!("{name}.bundle"));
    let _ = (
        std::fs::remove_file(&archive),
        std::fs::remove_file(&bundle),
    );
    ctx.ok(&source, &["archive", "export", archive.to_str().unwrap()]);
    ctx.ok(
        &source,
        &[
            "bundle",
            "export",
            "--ref",
            "heads/main",
            "--output",
            bundle.to_str().unwrap(),
        ],
    );
    let dest = ctx.repo(name);
    std::fs::write(dest.join("o.txt"), "o\n").unwrap();
    std::fs::write(dest.join("p.txt"), "p\n").unwrap();
    ctx.ok(
        &dest,
        &[
            "commit",
            "--ref",
            "heads/other",
            "--from-worktree",
            "-m",
            "genesis",
        ],
    );
    ctx.ok(&dest, &["seal", "--allow-no-audit", "--ref", "heads/other"]);
    ctx.ok(
        &dest,
        &["branch", "create", "heads/feature", "--from", "heads/other"],
    );
    queue_onto_feature(ctx, &dest, held, "o.txt", "p.txt");
    (dest, archive, bundle)
}

pub fn w3_archive_adopt(ctx: &Ctx, held: Held) -> Shape {
    let (dest, archive, _) = adopt_world(ctx, "base-w3-archive", held);
    refusal_shape(
        ctx,
        "W3 refusal: archive import --adopt",
        dest,
        args(&["archive", "import", "--adopt", archive.to_str().unwrap()]),
    )
}

pub fn w3_bundle_adopt(ctx: &Ctx, held: Held) -> Shape {
    let (dest, _, bundle) = adopt_world(ctx, "base-w3-bundle", held);
    refusal_shape(
        ctx,
        "W3 refusal: bundle import --adopt",
        dest,
        args(&[
            "bundle",
            "import",
            "--adopt",
            "--input",
            bundle.to_str().unwrap(),
        ]),
    )
}

pub fn w3_branch_create(ctx: &Ctx, held: Held) -> Shape {
    let (dest, _, _) = adopt_world(ctx, "base-w3-branch", held);
    refusal_shape(
        ctx,
        "W3 refusal: branch create (the current branch)",
        dest,
        args(&["branch", "create", "heads/main", "--from", "heads/other"]),
    )
}

/// **A control that can fail** (K3 ruling 5): a "writer" that empties the WAL and then writes the bytes back with plain `write` calls. It is unsound at
/// the two ordinals between the emptying and the write, and the sweep must say so there and nowhere else. A harness that passes this cannot see damage.
pub fn control_destroyer(ctx: &Ctx) -> Shape {
    let base = ctx.repo("base-control");
    three_files(ctx, &base);
    ctx.ok(&base, &["commit", "--from-worktree", "-m", "k"]);
    let script = ctx.root.join("destroyer.py");
    std::fs::write(
        &script,
        "import os\nw = '.prikk/active/default/queue.wal'\ndata = open(w, 'rb').read()\nf = open(w, 'wb'); f.close()\nf = open(w, 'wb'); f.write(data); f.flush(); os.fsync(f.fileno()); f.close()\n",
    )
    .unwrap();
    let mut s = shape(
        "control: empties the WAL, then writes it back",
        base,
        args(&["-I", script.to_str().unwrap()]),
        |ctx, d| {
            ctx.exit(d, &["verify"]) == 0
                && text(&ctx.out(d, &["status"])).lines().any(|l| {
                    l.starts_with("queued patches: ") && !l.starts_with("queued patches: 0")
                })
        },
    );
    s.program = Some(PathBuf::from("python3"));
    s.follow_named = false;
    s
}

/// `prikk init` into an empty directory: a kill part-way must leave something `init` again (or `verify`) can finish or refuse by name.
pub fn init(ctx: &Ctx) -> Shape {
    let base = ctx.root.join("base-init");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    shape("init", base, args(&["init"]), |ctx, d| {
        ctx.exit(d, &["verify"]) == 0
    })
}

//! 0.59.0 step W3, through the compiled binary: the rule of step W2 for every command that writes the worktree.
//!
//! **No command that writes the worktree changes, on disk, a path that an active queue holds, whichever ref owns the queue.**
//! `merge --into <the current branch>`, `archive import --adopt`, `bundle import --adopt` and `branch create` of the current
//! branch each judged "the branch being left's files" against the worktree's record, which `commit --ref <x>` writes with `<x>`'s
//! queue folded on top, so a queued new file was deleted, a queued edit reverted and a queued deletion written back, exit 0
//! (measured on 0.58.0 and on step W2's build, `.git-exclude/logs/0.59.0-W3/producers-before.txt`). They now refuse before the
//! first write, in W2's sentence, naming `seal --ref <owner>` and `uncommit <id>`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use crate::support::ok;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Output;

use prikk_object::{ObjectId, ObjectType, RefStatePayload};
use prikk_store::{FileObjectStore, ObjectReader, RefStore, RepositoryLayout};

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn prikk(repo: &Path, args: &[&str]) -> Output {
    support::prikk(repo).args(args).output().unwrap()
}

fn tip_block_id(repo: &Path, ref_name: &str) -> ObjectId {
    let layout = RepositoryLayout::open(repo.to_path_buf()).unwrap();
    let object_store = FileObjectStore::new(layout.clone());
    let ref_state_id = RefStore::new(layout)
        .read_current_ref_state_id(ref_name)
        .unwrap()
        .unwrap();
    let envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)
        .unwrap()
        .target_object_id
}

/// Every file in the repository, worktree and `.prikk` both (except `.prikk/cache`).
fn whole_repo(repo: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![repo.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|name| name == "cache")
                && path
                    .parent()
                    .is_some_and(|parent| parent == repo.join(".prikk"))
            {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                files.insert(
                    path.strip_prefix(repo).unwrap().to_path_buf(),
                    std::fs::read(&path).unwrap(),
                );
            }
        }
    }
    files
}

fn read(repo: &Path, path: &str) -> Option<Vec<u8>> {
    std::fs::read(repo.join(path)).ok()
}

fn queue_count(repo: &Path) -> usize {
    let status = stdout(&prikk(repo, &["status", "--format", "json"]));
    let after = status.split("\"count\":").nth(1).expect("queue count");
    after
        .trim_start()
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

fn named_uncommit_id(refusal: &str) -> String {
    let after = refusal
        .split("`prikk uncommit ")
        .nth(1)
        .unwrap_or_else(|| panic!("no `prikk uncommit <id>` in {refusal}"));
    let id: String = after.chars().take_while(char::is_ascii_hexdigit).collect();
    assert_eq!(id.len(), 64, "a full patch id: {refusal}");
    id
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    NewFile,
    Edit,
    Deletion,
}

const SHAPES: [Shape; 3] = [Shape::NewFile, Shape::Edit, Shape::Deletion];

/// Which file each shape touches, in the world each producer uses (`merge`: `a.txt`/`k.txt`; the others: `o.txt`/`p.txt`).
struct Names {
    edit: &'static str,
    delete: &'static str,
}

impl Shape {
    fn queue(self, repo: &Path, names: &Names, onto: &str) {
        match self {
            Shape::NewFile => std::fs::write(repo.join("queued.txt"), b"q\n").unwrap(),
            Shape::Edit => {
                let mut bytes = std::fs::read(repo.join(names.edit)).unwrap();
                bytes.extend_from_slice(b"e\n");
                std::fs::write(repo.join(names.edit), bytes).unwrap();
            }
            Shape::Deletion => std::fs::remove_file(repo.join(names.delete)).unwrap(),
        }
        ok(
            &support::commit(repo, onto, "queued"),
            "commit --ref <owner>",
        );
    }

    /// The path the shape holds.
    fn held(self, names: &Names) -> &'static str {
        match self {
            Shape::NewFile => "queued.txt",
            Shape::Edit => names.edit,
            Shape::Deletion => names.delete,
        }
    }
}

const MERGE_NAMES: Names = Names {
    edit: "a.txt",
    delete: "k.txt",
};
const OTHER_NAMES: Names = Names {
    edit: "o.txt",
    delete: "p.txt",
};

// ---- the worlds ---------------------------------------------------------------------------------------------------------

/// `heads/main` (current) with `a.txt` and `k.txt`, a sealed `heads/topic` adding `t.txt`, and a queue for `heads/feature`.
fn merge_world(tag: &str, shape: Option<Shape>) -> (PathBuf, ObjectId) {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"base\n", "a");
    support::generation(&repo, "heads/main", "k.txt", b"keep\n", "k");
    let baseline = tip_block_id(&repo, "heads/main");
    ok(
        &support::branch_create(&repo, "heads/topic", "heads/main"),
        "branch create topic",
    );
    ok(
        &support::branch_create(&repo, "heads/feature", "heads/main"),
        "branch create feature",
    );
    support::generation(&repo, "heads/topic", "t.txt", b"t\n", "topic adds t.txt");
    std::fs::remove_file(repo.join("t.txt")).unwrap();
    if let Some(shape) = shape {
        shape.queue(&repo, &MERGE_NAMES, "heads/feature");
    }
    (repo, baseline)
}

fn merge(repo: &Path, baseline: ObjectId) -> Output {
    support::trust_maintainer(repo);
    support::prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args([
            "merge",
            "--allow-no-audit",
            "--baseline-block",
            &baseline.to_string(),
            "--into",
            "heads/main",
            "--from",
            "heads/topic",
        ])
        .output()
        .unwrap()
}

/// A destination whose current branch `heads/main` is unborn, `heads/other` sealed with `o.txt` and `p.txt`, and a queue for
/// `heads/feature` (created from `heads/other`).
fn dest_world(tag: &str, shape: Option<Shape>) -> PathBuf {
    let dest = support::unique_repo(tag);
    support::init(&dest);
    support::trust_maintainer(&dest);
    std::fs::write(dest.join("o.txt"), b"o\n").unwrap();
    std::fs::write(dest.join("p.txt"), b"p\n").unwrap();
    ok(
        &support::commit(&dest, "heads/other", "genesis"),
        "commit --ref heads/other",
    );
    ok(
        &support::seal(&dest, "heads/other"),
        "seal --ref heads/other",
    );
    ok(
        &support::branch_create(&dest, "heads/feature", "heads/other"),
        "branch create feature",
    );
    if let Some(shape) = shape {
        shape.queue(&dest, &OTHER_NAMES, "heads/feature");
    }
    dest
}

/// A source repository whose `heads/main` holds `n.txt`: an archive and a bundle of it, in `dir`.
fn adoptable(tag: &str) -> (PathBuf, PathBuf) {
    let src = support::unique_repo(&format!("{tag}-src"));
    support::init(&src);
    support::generation(&src, "heads/main", "n.txt", b"n\n", "genesis");
    let archive =
        std::env::temp_dir().join(format!("prikk-w3-{tag}-{}.prepo", support::unique_suffix()));
    let bundle = std::env::temp_dir().join(format!(
        "prikk-w3-{tag}-{}.bundle",
        support::unique_suffix()
    ));
    ok(
        &support::prikk(&src)
            .args(["archive", "export"])
            .arg(&archive)
            .output()
            .unwrap(),
        "archive export",
    );
    ok(
        &support::prikk(&src)
            .args(["bundle", "export", "--ref", "heads/main", "--output"])
            .arg(&bundle)
            .output()
            .unwrap(),
        "bundle export",
    );
    let _ = std::fs::remove_dir_all(&src);
    (archive, bundle)
}

// ---- the producers ------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Producer {
    Merge,
    Archive,
    Bundle,
    BranchCreate,
}

const PRODUCERS: [Producer; 4] = [
    Producer::Merge,
    Producer::Archive,
    Producer::Bundle,
    Producer::BranchCreate,
];

impl Producer {
    fn action(self) -> &'static str {
        match self {
            Producer::Merge => "merging heads/topic into heads/main",
            Producer::Archive | Producer::Bundle => "adopting heads/main",
            Producer::BranchCreate => "creating heads/main",
        }
    }

    fn names(self) -> &'static Names {
        match self {
            Producer::Merge => &MERGE_NAMES,
            _ => &OTHER_NAMES,
        }
    }

    /// A world for this producer; the third value is what the producer needs to run (a baseline, an archive, a bundle).
    fn world(self, tag: &str, shape: Option<Shape>) -> World {
        match self {
            Producer::Merge => {
                let (repo, baseline) = merge_world(tag, shape);
                World {
                    repo,
                    baseline: Some(baseline),
                    archive: None,
                    bundle: None,
                }
            }
            _ => {
                let repo = dest_world(tag, shape);
                let (archive, bundle) = adoptable(tag);
                World {
                    repo,
                    baseline: None,
                    archive: Some(archive),
                    bundle: Some(bundle),
                }
            }
        }
    }

    fn run(self, world: &World) -> Output {
        match self {
            Producer::Merge => merge(&world.repo, world.baseline.unwrap()),
            Producer::Archive => support::prikk(&world.repo)
                .args(["archive", "import", "--adopt"])
                .arg(world.archive.as_ref().unwrap())
                .output()
                .unwrap(),
            Producer::Bundle => {
                support::trust_maintainer(&world.repo);
                support::prikk(&world.repo)
                    .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
                    .env(
                        "PRIKK_MAINTAINER_SEED_FILE",
                        support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
                    )
                    .args(["bundle", "import", "--adopt", "--input"])
                    .arg(world.bundle.as_ref().unwrap())
                    .output()
                    .unwrap()
            }
            Producer::BranchCreate => {
                support::branch_create(&world.repo, "heads/main", "heads/other")
            }
        }
    }

    /// Whether a shape changes a held path on 0.58.0: an adoption of `n.txt` leaves a queued deletion of `p.txt` as it was
    /// (the file is absent and stays absent), so that row is unchanged.
    fn changes_the_held_path(self, shape: Shape) -> bool {
        !(matches!(self, Producer::Archive | Producer::Bundle) && shape == Shape::Deletion)
    }
}

struct World {
    repo: PathBuf,
    baseline: Option<ObjectId>,
    archive: Option<PathBuf>,
    bundle: Option<PathBuf>,
}

impl World {
    fn remove(self) {
        let _ = std::fs::remove_dir_all(&self.repo);
        for file in [self.archive, self.bundle].into_iter().flatten() {
            let _ = std::fs::remove_file(file);
        }
    }
}

/// The disk the user left, for every file the worlds use.
fn disk(repo: &Path) -> Vec<(&'static str, Option<Vec<u8>>)> {
    [
        "a.txt",
        "k.txt",
        "o.txt",
        "p.txt",
        "queued.txt",
        "t.txt",
        "n.txt",
    ]
    .into_iter()
    .map(|path| (path, read(repo, path)))
    .collect()
}

// ---- the tests ----------------------------------------------------------------------------------------------------------

/// Every producer that changed a held path refuses before the first write, with the repository byte-identical (so no marker, no
/// object, no journal), the queue untouched and the files as the user left them; the sentence names the owner, the paths, the
/// producer's own action and both ways out.
#[test]
fn every_producer_refuses_before_any_write_when_a_queue_holds_a_path_it_would_change() {
    for producer in PRODUCERS {
        for shape in SHAPES {
            if !producer.changes_the_held_path(shape) {
                continue;
            }
            let world = producer.world(&format!("w3-refuse-{producer:?}-{shape:?}"), Some(shape));
            let before = whole_repo(&world.repo);
            let disk_before = disk(&world.repo);
            let out = producer.run(&world);
            assert_eq!(
                out.status.code(),
                Some(1),
                "{producer:?}/{shape:?}: {}{}",
                stdout(&out),
                stderr(&out)
            );
            let message = stderr(&out);
            let held = shape.held(producer.names());
            assert!(
                message.contains(&format!(
                    "refusing: heads/feature's queue holds 1 commit(s) that change {held}, which {} would overwrite or delete on disk (nothing was written); `prikk seal --allow-no-audit --ref heads/feature` publishes them to heads/feature, or `prikk uncommit ",
                    producer.action()
                )) && message.contains("takes the newest commit back and leaves your files"),
                "{producer:?}/{shape:?}: {message}"
            );
            assert_eq!(
                disk(&world.repo),
                disk_before,
                "{producer:?}/{shape:?}: the files are as the user left them"
            );
            assert_eq!(
                whole_repo(&world.repo),
                before,
                "{producer:?}/{shape:?}: the repository is byte-identical"
            );
            assert_eq!(queue_count(&world.repo), 1, "{producer:?}/{shape:?}");
            world.remove();
        }
    }
}

/// A producer that did not change a held path on 0.58.0 is unchanged: an adoption of `n.txt` with a queued deletion of `p.txt`.
#[test]
fn a_producer_that_changes_no_held_path_is_unchanged() {
    for producer in [Producer::Archive, Producer::Bundle] {
        let world = producer.world(&format!("w3-unchanged-{producer:?}"), Some(Shape::Deletion));
        let out = producer.run(&world);
        ok(
            &out,
            &format!("{producer:?} with a queued deletion it does not touch"),
        );
        assert_eq!(read(&world.repo, "n.txt").as_deref(), Some(&b"n\n"[..]));
        assert_eq!(
            read(&world.repo, "p.txt"),
            None,
            "the queued deletion is still a deletion"
        );
        world.remove();
    }
}

/// A queue that holds no path the producer changes (a file created and deleted again) does not refuse any of them.
#[test]
fn a_queue_the_producer_does_not_touch_does_not_refuse() {
    for producer in PRODUCERS {
        let world = producer.world(&format!("w3-clean-{producer:?}"), None);
        let names = producer.names();
        let repo = &world.repo;
        std::fs::write(repo.join("queued.txt"), b"q\n").unwrap();
        ok(&support::commit(repo, "heads/feature", "add"), "commit add");
        std::fs::remove_file(repo.join("queued.txt")).unwrap();
        ok(
            &support::commit(repo, "heads/feature", "remove"),
            "commit remove",
        );
        let _ = names;
        let out = producer.run(&world);
        ok(
            &out,
            &format!("{producer:?}: a queue that changes nothing the producer touches"),
        );
        world.remove();
    }
}

/// The first way out each sentence names, `seal --ref <owner>`: the producer then goes through.
#[test]
fn seal_then_the_producer_goes_through() {
    for producer in PRODUCERS {
        let world = producer.world(&format!("w3-seal-{producer:?}"), Some(Shape::NewFile));
        assert_eq!(producer.run(&world).status.code(), Some(1), "{producer:?}");
        ok(
            &support::seal(&world.repo, "heads/feature"),
            "seal --ref heads/feature",
        );
        ok(
            &producer.run(&world),
            &format!("{producer:?} after the seal"),
        );
        assert_eq!(queue_count(&world.repo), 0, "{producer:?}");
        world.remove();
    }
}

/// The second way out, `uncommit <id>` with the id the sentence names: the queue is empty, the files stay, and the producer goes
/// through (a new file is untracked, so nothing stands in its way).
#[test]
fn uncommit_then_the_producer_goes_through_and_the_file_stays() {
    for producer in PRODUCERS {
        let world = producer.world(&format!("w3-uncommit-{producer:?}"), Some(Shape::NewFile));
        let refusal = stderr(&producer.run(&world));
        let id = named_uncommit_id(&refusal);
        ok(
            &prikk(&world.repo, &["uncommit", &id]),
            "the named uncommit",
        );
        assert_eq!(queue_count(&world.repo), 0, "{producer:?}");
        assert_eq!(
            read(&world.repo, "queued.txt").as_deref(),
            Some(&b"q\n"[..]),
            "{producer:?}: the file stays"
        );
        let out = producer.run(&world);
        ok(
            &out,
            &format!("{producer:?} after the uncommit: {}", stderr(&out)),
        );
        assert_eq!(
            read(&world.repo, "queued.txt").as_deref(),
            Some(&b"q\n"[..]),
            "{producer:?}: the file the user took back is still there"
        );
        world.remove();
    }
}

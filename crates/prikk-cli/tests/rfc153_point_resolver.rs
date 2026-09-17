//! RFC 153 §2 and §7.1, point-resolver handoff (`rfcs/handoffs/153-content-diff/point-resolver-handoff-v1.md`):
//! a point is a ref or a bare block id, resolved by one function, and `checkout`'s read-only modes take
//! either. Through the compiled binary, except the anchoring count, which reads the same replay through a
//! test-support seam.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Output;

use prikk_store::{ReceivedRefs, RepositoryLayout};

fn run(repo: &Path, args: &[&str]) -> Output {
    support::prikk(repo).args(args).output().unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn ok(repo: &Path, args: &[&str]) -> String {
    let output = run(repo, args);
    support::ok(&output, &args.join(" "));
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn refuses(repo: &Path, args: &[&str], code: i32, expected: &str) {
    let output = run(repo, args);
    assert_eq!(
        output.status.code(),
        Some(code),
        "{}: {}",
        args.join(" "),
        text(&output)
    );
    assert!(
        text(&output).contains(expected),
        "{}: expected {expected:?} in {}",
        args.join(" "),
        text(&output)
    );
}

/// Every block on `reference`, newest first, as `log` prints them.
fn blocks(repo: &Path, reference: &str) -> Vec<String> {
    ok(repo, &["log", "--ref", reference, "--limit", "1000"])
        .lines()
        .filter_map(|line| line.strip_prefix("block "))
        .map(|id| id.trim().to_string())
        .collect()
}

/// `a.txt` edited, reverted and edited again: four sealed blocks, oldest first `one`, `two`, `one`,
/// `three`. The tip is not a checkpoint. No trailing newline, so the test JSON parser (which does not
/// decode escapes) reads the text as written.
fn edit_revert_edit(tag: &str) -> (PathBuf, Vec<(String, &'static str)>) {
    let repo = support::unique_repo(tag);
    ok(&repo, &["setup", "."]);
    let contents = ["one", "two", "one", "three"];
    for (index, content) in contents.iter().enumerate() {
        std::fs::write(repo.join("a.txt"), content).unwrap();
        ok(&repo, &["commit", "-m", &format!("generation {index}")]);
        ok(&repo, &["seal", "--allow-no-audit"]);
    }
    let mut ids = blocks(&repo, "heads/main");
    ids.reverse();
    assert_eq!(ids.len(), contents.len());
    (repo, ids.into_iter().zip(contents).collect())
}

fn content_at(repo: &Path, point: &str) -> String {
    let json = support::json::parse(&ok(
        repo,
        &[
            "checkout",
            "--patch-plan",
            "--format",
            "json",
            "--content-path",
            "a.txt",
            "--ref",
            point,
        ],
    ));
    json.get("content").as_array()[0]
        .get("content")
        .get("text")
        .as_str()
        .to_string()
}

/// Every file under `root`, with its bytes: what "writes nothing" is compared against.
fn tree_bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    out
}

/// Control 1, the RFC 144 §4t case: two blocks on one ref are two points, each with its own content.
#[test]
fn control1_each_block_on_one_ref_reads_its_own_content() {
    let (repo, points) = edit_revert_edit("rfc153-point-each-block");
    for (block, content) in &points {
        assert_eq!(
            &content_at(&repo, block),
            content,
            "content at block {block}"
        );
    }
    assert_eq!(content_at(&repo, "heads/main"), "three");
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 2: the tip named by ref and by block id is the same point. Output is byte-identical except
/// the name: the JSON `ref` value, and the prose header's `ref:`/`block:` line.
#[test]
fn control2_the_tip_by_ref_and_by_block_id_is_byte_identical_but_for_the_name() {
    let (repo, points) = edit_revert_edit("rfc153-point-byte-identity");
    let tip = points.last().unwrap().0.clone();

    let by_ref = ok(
        &repo,
        &[
            "checkout",
            "--patch-plan",
            "--format",
            "json",
            "--content-path",
            "a.txt",
            "--ref",
            "heads/main",
        ],
    );
    let by_id = ok(
        &repo,
        &[
            "checkout",
            "--patch-plan",
            "--format",
            "json",
            "--content-path",
            "a.txt",
            "--ref",
            &tip,
        ],
    );
    let ref_line = "  \"ref\": \"heads/main\",";
    let id_line = format!("  \"ref\": \"{tip}\",");
    assert!(by_ref.contains(ref_line), "{by_ref}");
    assert!(by_id.contains(&id_line), "{by_id}");
    assert_eq!(by_ref.replace(ref_line, ""), by_id.replace(&id_line, ""));

    for mode in ["--patch-plan", "--patch-delete-plan"] {
        let by_ref = ok(&repo, &["checkout", mode, "--ref", "heads/main"]);
        let by_id = ok(&repo, &["checkout", mode, "--ref", &tip]);
        assert!(by_ref.contains("\nref: heads/main\n"), "{mode}: {by_ref}");
        assert!(
            by_id.contains(&format!("\nblock: {tip}\n")),
            "{mode}: {by_id}"
        );
        assert_eq!(
            by_ref.replace("\nref: heads/main\n", "\n"),
            by_id.replace(&format!("\nblock: {tip}\n"), "\n"),
            "{mode}"
        );
    }
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 3: a block id at depth replays from its checkpoint, exactly as its ref does -- one replay,
/// shared. 66 sealed blocks: checkpoints at the first and the 65th, so the tip replays one block.
#[test]
fn control3_a_block_id_at_depth_replays_from_its_checkpoint() {
    let repo = support::unique_repo("rfc153-point-anchored-depth");
    ok(&repo, &["setup", "."]);
    for generation in 0..66 {
        std::fs::write(repo.join("a.txt"), format!("generation {generation}")).unwrap();
        ok(
            &repo,
            &["commit", "-m", &format!("generation {generation}")],
        );
        ok(&repo, &["seal", "--allow-no-audit"]);
    }
    let ids = blocks(&repo, "heads/main");
    assert_eq!(ids.len(), 66);
    let layout = RepositoryLayout::open(repo.clone()).unwrap();
    let count = |name: &str| {
        let point = prikk_store::resolve_point(&layout, name, ReceivedRefs::Refused).unwrap();
        prikk_store::replayed_block_count_at_point_for_test_support(&layout, &point).unwrap()
    };
    let tip_by_ref = count("heads/main");
    let tip_by_id = count(&ids[0]);
    assert_eq!(
        tip_by_id, 1,
        "the tip replays only the block after its checkpoint"
    );
    assert_eq!(
        tip_by_id, tip_by_ref,
        "a block id replays exactly as its ref"
    );
    assert_eq!(count(&ids[1]), 0, "the 65th block is itself a checkpoint");
    assert_eq!(
        content_at(&repo, &ids[0]),
        "generation 65",
        "and the anchored replay reads the right content"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 4: what refuses, with which answer and exit code, and that a write mode given a block id
/// writes nothing.
#[test]
fn control4_refusals_and_write_modes() {
    let (repo, points) = edit_revert_edit("rfc153-point-refusals");
    let unknown = "0".repeat(64);
    refuses(
        &repo,
        &["checkout", "--patch-plan", "--ref", &unknown],
        1,
        &format!("error: precondition not met: block {unknown} is not in this repository"),
    );
    let patch = ok(&repo, &["log", "--ref", "heads/main", "--limit", "1"])
        .lines()
        .find_map(|line| line.trim().strip_prefix("patch "))
        .and_then(|rest| rest.split(':').next())
        .expect("log names the tip's patch")
        .to_string();
    refuses(
        &repo,
        &["checkout", "--patch-plan", "--ref", &patch],
        1,
        &format!("error: precondition not met: object {patch} is a patch, not a block"),
    );
    let tip = points.last().unwrap().0.clone();
    for malformed in ["main", &tip.to_uppercase(), &tip[..63], "HEAD"] {
        refuses(
            &repo,
            &["checkout", "--patch-plan", "--ref", malformed],
            2,
            "is neither a ref name (heads/…, tags/… or remotes/…) nor a block id (64 lowercase hex \
             characters)",
        );
    }

    // A different worktree, so a write would show.
    std::fs::write(repo.join("a.txt"), "local edit\n").unwrap();
    for mode in [
        "--patch-materialize",
        "--snapshot-materialize",
        "--patch-materialize-delete",
    ] {
        let before = tree_bytes(&repo);
        refuses(
            &repo,
            &["checkout", mode, "--ref", &points[0].0],
            1,
            &format!(
                "error: precondition not met: checkout {mode} writes the worktree, which needs a \
                 branch: the next `commit` authors against one, and a block id names no branch"
            ),
        );
        assert_eq!(
            before,
            tree_bytes(&repo),
            "{mode} with a block id wrote something"
        );
    }
    let _ = std::fs::remove_dir_all(&repo);
}

/// Every read-only mode takes a block id, and a block only received history reaches resolves: reading
/// is not adopting.
#[test]
fn read_only_modes_take_a_block_id_including_one_only_received_history_reaches() {
    let (src, points) = edit_revert_edit("rfc153-point-modes-src");
    let genesis = points[0].0.clone();
    let middle = points[1].0.clone();

    let plan = ok(&src, &["checkout", "--plan-only", "--ref", &genesis]);
    assert!(plan.contains(&format!("\nblock: {genesis}\n")), "{plan}");
    assert!(plan.contains("\nref-state: <none>\n"), "{plan}");
    assert!(
        plan.contains(&format!("\ntarget block: {genesis}\n")),
        "{plan}"
    );

    let snapshot = ok(&src, &["checkout", "--snapshot-plan", "--ref", &genesis]);
    assert!(snapshot.contains("snapshot files: 1"), "{snapshot}");
    // Not a checkpoint: the refusal names a route, and the route it names works for a block id.
    refuses(
        &src,
        &["checkout", "--snapshot-plan", "--ref", &middle],
        1,
        &format!("use `prikk checkout --patch-plan --ref {middle}`, which replays without one"),
    );
    ok(&src, &["checkout", "--patch-plan", "--ref", &middle]);

    let bundle = src.join("main.bundle");
    ok(
        &src,
        &[
            "bundle",
            "export",
            "--ref",
            "heads/main",
            "--output",
            bundle.to_str().unwrap(),
        ],
    );
    let dst = support::unique_repo("rfc153-point-modes-dst");
    ok(&dst, &["init", "."]);
    ok(
        &dst,
        &["bundle", "import", "--input", bundle.to_str().unwrap()],
    );
    assert_eq!(content_at(&dst, &middle), "two");
    // The received ref itself is still refused by checkout (0.45.0's per-command rule).
    refuses(
        &dst,
        &["checkout", "--patch-plan", "--ref", "remotes/heads/main"],
        1,
        "error: precondition not met: remotes/heads/main is a received ref",
    );
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&dst);
}

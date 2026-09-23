//! Shared CLI end-to-end test harness (DC-67).
//!
//! DC-61, DC-65, and DC-66's test files each rolled their own `commit`/`seal`/key setup — copy-pasted
//! three times before this consolidation. Every test in `dc67_ordinary_use_conformance.rs` uses this
//! module instead of a fourth (and fifth, sixth, ...) copy. Existing files are left as they are; this
//! is not a retrofit, only the point past which no one should copy-paste it again.

#![allow(dead_code)]
// The helpers below are fixture plumbing: a failure to create a scratch directory is a broken
// machine, not a condition a test should carry a `Result` for.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

/// RFC 147 §2e: the one JSON value parser for CLI end-to-end tests, so a test that reads
/// `--format json` does not carry a third hand-written copy of one.
pub mod json;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub fn prikk(repo: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_prikk"));
    cmd.current_dir(repo);
    isolate_key_environment_for(&mut cmd, Some(repo));
    cmd
}

/// Point every test invocation at an empty, per-process key directory, and strip any `PRIKK_*` the
/// developer's own shell is carrying.
///
/// **RFC 148 made this necessary and its absence was not theoretical.** Once prikk looks in
/// `$XDG_CONFIG_HOME/prikk` for a seed, a test that asserts "signing fails when no key is
/// configured" passes or fails depending on whether the person running it happens to have keys — and
/// a test that runs `prikk setup` writes into the real `~/.config/prikk`. Both happened here on the
/// first full run after the change: `prikk setup` created keys in my own home directory, and every
/// no-key-configured assertion then failed because the CLI correctly found them.
///
/// So the harness makes the ambient state explicit: **every variable `default_key_dir` consults on
/// any platform** — `XDG_CONFIG_HOME`, `HOME` and `APPDATA` — points at a scratch directory, and the
/// retired seed variables are removed rather than left to be refused. A test that *wants* key
/// material sets `PRIKK_*_SEED_FILE` itself, after this call, and overrides it.
///
/// "Every variable on any platform" is the rule, and it is not a stylistic one: the first version set
/// the two Unix variables and shipped, and Windows CI went red because `APPDATA` was still the
/// runner's own.
pub fn isolate_key_environment(cmd: &mut Command) {
    isolate_key_environment_for(cmd, None);
}

/// The same isolation, keyed to one repository.
///
/// **Per repository, not per process.** A single config home shared by a whole test binary looked
/// right and was not: `prikk setup` refuses to overwrite an existing seed, so the second `setup` in
/// one binary failed on the first one's keys. Keying the directory to the repository under test
/// gives each test its own, while repeated calls within one test — `setup`, then `commit`, then
/// `seal` — still share it, which is exactly the lifetime the real thing has.
pub fn isolate_key_environment_for(cmd: &mut Command, repo: Option<&Path>) {
    let home = isolated_config_home(repo);
    cmd.env("XDG_CONFIG_HOME", &home)
        .env("HOME", &home)
        // **`APPDATA` too, unconditionally.** `key_material::default_key_dir` reads `XDG_CONFIG_HOME`
        // / `HOME` on Unix and `APPDATA` on Windows, and the first version of this seam set only the
        // first two -- so on Windows every test's `setup` wrote into the *runner's own*
        // `%APPDATA%\prikk`, in parallel, and the second one collided with the first's seeds. `main`
        // went red on the "Windows mutation test suite" while every host gate and both cross-target
        // compiles were green, because compiling Windows code is not running it.
        //
        // Set on every platform rather than behind a `cfg`: it is inert where it is unread, and a
        // seam that neutralises a different set of variables per platform is a seam with a second
        // way to be incomplete. `isolated_key_dir` resolves to `<home>/prikk` either way.
        .env("APPDATA", &home)
        .env_remove("PRIKK_AUTHOR_SEED")
        .env_remove("PRIKK_MAINTAINER_SEED")
        .env_remove("PRIKK_AUTHOR_SEED_FILE")
        .env_remove("PRIKK_MAINTAINER_SEED_FILE")
        .env_remove("PRIKK_AUTHOR_KEY_ID")
        .env_remove("PRIKK_MAINTAINER_KEY_ID");
}

/// The key directory a test's isolated config home resolves to — `<config home>/prikk`, the same
/// path `key_material::default_key_dir` computes from `XDG_CONFIG_HOME`.
pub fn isolated_key_dir(repo: &Path) -> PathBuf {
    isolated_config_home(Some(repo)).join("prikk")
}

/// One empty config home per repository (or one per process for a command with no repository).
/// Empty on purpose: the default state a test should see is "no keys anywhere", and anything else
/// is set explicitly by the test that needs it.
fn isolated_config_home(repo: Option<&Path>) -> PathBuf {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    static HOMES: OnceLock<Mutex<HashMap<PathBuf, PathBuf>>> = OnceLock::new();
    let key = repo.map_or_else(|| PathBuf::from("<no repository>"), Path::to_path_buf);
    let homes = HOMES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut homes = homes.lock().expect("config-home cache");
    if let Some(home) = homes.get(&key) {
        return home.clone();
    }
    let mut dir = std::env::temp_dir();
    dir.push(format!("prikk-test-config-home-{}", unique_suffix()));
    std::fs::create_dir_all(&dir).expect("create isolated config home");
    homes.insert(key, dir.clone());
    dir
}

pub fn ok(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed (status {:?})\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// DC-84: a temp-directory-naming suffix that is genuinely collision-free across threads of one test
/// binary, not just across separate processes. Every prikk-cli integration test file that built its
/// own `unique_repo`/`unique_root`/`unique_dir` from `process::id()` plus a nanosecond timestamp
/// shared the same latent defect DC-83 found and measured: `process::id()` is constant for every
/// thread of one process, so it cannot distinguish two racing threads of the *same* binary, and a
/// bare-barrier stress test showed real nanosecond collisions under thread contention (214 in
/// 128,000 samples — a rate, not a hypothetical). The `fetch_add` sequence number below is the only
/// part that actually guarantees uniqueness, regardless of clock resolution or thread scheduling —
/// confirmed by the same stress test at zero collisions once added (see the crate's
/// `helper_uniqueness` test module). Process id and the timestamp are kept: the id still separates
/// this binary's temp directories from an unrelated process using the same scheme, and the timestamp
/// keeps directory names human-orderable.
pub fn unique_suffix() -> String {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}-{nanos}-{sequence}", std::process::id())
}

pub fn unique_repo(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("prikk-cli-dc67-{tag}-{}", unique_suffix()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub const AUTHOR_KEY_ID: &str = "dc67-test-author";
pub const AUTHOR_SEED_HEX: &str =
    "3300445566778899001122334455667788990011223344556677889900112233";
pub const MAINTAINER_KEY_ID: &str = "dc67-test-maintainer";
pub const MAINTAINER_SEED: [u8; 32] = [
    0x71, 0x71, 0x82, 0x82, 0x93, 0x93, 0xa4, 0xa4, 0xb5, 0xb5, 0xc6, 0xc6, 0xd7, 0xd7, 0xe8, 0xe8,
    0xf9, 0xf9, 0x0a, 0x0a, 0x1b, 0x1b, 0x2c, 0x2c, 0x3d, 0x3d, 0x4e, 0x4e, 0x5f, 0x5f, 0x60, 0x60,
];

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn maintainer_public_key_hex() -> String {
    use prikk_store::MaintainerSigner;
    let signer =
        prikk_store::Ed25519MaintainerSigner::from_seed(MAINTAINER_KEY_ID, &MAINTAINER_SEED)
            .expect("fixed maintainer seed derives a valid signer");
    hex(&signer.public_key_bytes())
}

/// RFC 148: a seed reaches prikk through a **file**, never the environment. This writes `seed_hex`
/// to a mode-`0600` file and returns its path, for `PRIKK_<ROLE>_SEED_FILE`.
///
/// **One helper, not 118 edits.** Every test that signs used to set `PRIKK_*_SEED` with a hex
/// string; each of those became `.env("PRIKK_*_SEED_FILE", support::seed_file(<same hex>))`, so the
/// fixture keys are unchanged and only the channel moved. Writing the file here rather than in each
/// test is what keeps the mode rule in one place too — a test that wrote `0644` by hand would now be
/// refused by the reader, and every such test would have had to learn why.
///
/// Memoised per hex value: a binary that signs a hundred times writes one file per distinct key, not
/// one per call. The files live in the process's temp directory and are left for the OS to reap,
/// like every other fixture here.
pub fn seed_file(seed_hex: &str) -> PathBuf {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    static FILES: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
    let files = FILES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut files = files.lock().expect("seed-file cache");
    if let Some(path) = files.get(seed_hex) {
        return path.clone();
    }

    let mut dir = std::env::temp_dir();
    dir.push(format!("prikk-test-seeds-{}", unique_suffix()));
    std::fs::create_dir_all(&dir).expect("create seed directory");
    let path = dir.join("seed");
    write_private_file(&path, seed_hex);
    files.insert(seed_hex.to_string(), path.clone());
    path
}

#[cfg(unix)]
fn write_private_file(path: &Path, contents: &str) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .expect("create seed file");
    file.write_all(contents.as_bytes())
        .expect("write seed file");
}

#[cfg(not(unix))]
fn write_private_file(path: &Path, contents: &str) {
    std::fs::write(path, contents).expect("write seed file");
}

pub fn init(repo: &Path) {
    ok(&prikk(repo).arg("init").output().unwrap(), "init");
}

/// Commit on `ref_name` (repository-relative, e.g. `"heads/main"`).
pub fn commit(repo: &Path, ref_name: &str, message: &str) -> Output {
    prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", AUTHOR_KEY_ID)
        .env("PRIKK_AUTHOR_SEED_FILE", seed_file(AUTHOR_SEED_HEX))
        .args(["commit", "--ref", ref_name, "-m", message])
        .output()
        .unwrap()
}

/// Trust the fixed maintainer key (idempotent-enough for repeated calls within one test — only
/// `seal` itself must succeed).
pub fn trust_maintainer(repo: &Path) {
    let _ = prikk(repo)
        .args([
            "trust",
            "maintainer",
            "add",
            "--key-id",
            MAINTAINER_KEY_ID,
            "--public-key",
            &maintainer_public_key_hex(),
        ])
        .output()
        .unwrap();
}

/// Seal `ref_name`, trusting the fixed maintainer key first.
pub fn seal(repo: &Path, ref_name: &str) -> Output {
    trust_maintainer(repo);
    prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            seed_file(&hex(&MAINTAINER_SEED)),
        )
        .args(["seal", "--allow-no-audit", "--ref", ref_name])
        .output()
        .unwrap()
}

/// One generation: write `path` with `content`, commit, seal — the mutate/commit/seal cycle §3
/// defines a "generation" as. Asserts both steps succeed.
pub fn generation(repo: &Path, ref_name: &str, path: &str, content: &[u8], message: &str) {
    std::fs::write(repo.join(path), content).unwrap();
    ok(
        &commit(repo, ref_name, message),
        &format!("commit: {message}"),
    );
    ok(&seal(repo, ref_name), &format!("seal: {message}"));
}

pub fn branch_create(repo: &Path, name: &str, from: &str) -> Output {
    trust_maintainer(repo);
    prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            seed_file(&hex(&MAINTAINER_SEED)),
        )
        .args(["branch", "create", name, "--from", from])
        .output()
        .unwrap()
}

pub fn branch_close(repo: &Path, name: &str) -> Output {
    trust_maintainer(repo);
    prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            seed_file(&hex(&MAINTAINER_SEED)),
        )
        .args(["branch", "close", name])
        .output()
        .unwrap()
}

pub fn tag_create(repo: &Path, name: &str, target: &str) -> Output {
    trust_maintainer(repo);
    prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            seed_file(&hex(&MAINTAINER_SEED)),
        )
        .args(["tag", "create", name, "--target", target])
        .output()
        .unwrap()
}

pub fn verify(repo: &Path) -> Output {
    prikk(repo).arg("verify").output().unwrap()
}

/// Every file under `.prikk`, except lock files, which a refused command creates and removes
/// itself in the course of refusing -- shared by every "a refusal writes nothing" control (DC-44's
/// own 0.44.0-sense definition: byte-for-byte, not just "the object count is unchanged").
pub fn store_bytes(repo: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let mut files = std::collections::BTreeMap::new();
    let mut stack = vec![repo.join(".prikk")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains("lock"))
            {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                files.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    files
}

/// Append an attributable torn tail to the shared ref-log container, by duplicating (truncated) the
/// header of whichever real record currently sits last in the file.
///
/// RFC 102 Stage 4: under the old per-ref-file model, appending *any* trailing garbage to a ref's own
/// log file simulated "this ref's own torn write" -- the file's identity alone did the attribution.
/// The shared container instead attributes a torn tail to a ref via the frame header's own
/// `ref_name_key` field (`refs/container.rs`'s `trailing_tail_ref_name_key`), which requires at least
/// a full header's worth of intact bytes (magic(8) + version(2) + ref_name_key(32) + body_len(8) +
/// checksum(32) = 82) to even attempt reading. Bare garbage shorter than that is simply unattributable
/// to any ref -- not a torn write for the ref under test, a torn write for nobody. This duplicates the
/// last real frame's own header (plus a few body bytes, to stay a torn *record* rather than a torn
/// *header*) so the appended bytes carry a genuine, correctly-attributed `ref_name_key` -- whichever
/// ref actually owns the container's current last record, which every caller here has arranged to be
/// the ref under test by publishing to it most recently.
pub fn append_torn_ref_log_tail(container_path: &Path) {
    const MAGIC: &[u8; 8] = b"PREFCON1";
    const HEADER_LEN: usize = 8 + 2 + 32 + 8 + 32;
    let bytes = std::fs::read(container_path).unwrap();
    let start = bytes
        .windows(MAGIC.len())
        .rposition(|window| window == MAGIC)
        .expect("ref log container has at least one real record to duplicate");
    let end = (start + HEADER_LEN + 8).min(bytes.len());
    assert!(
        end < bytes.len(),
        "duplicated span must land inside the real record's body, not consume it entirely, \
         or the result would be a complete record rather than a torn one"
    );
    let torn = bytes[start..end].to_vec();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(container_path)
        .unwrap();
    use std::io::Write as _;
    file.write_all(&torn).unwrap();
}

/// RFC 142 show-degradation handoff v2, control 1's fixture: corrupt a real repository's blob
/// container so that a specific, already-referenced blob decodes to *different, still internally
/// valid* content -- reaching `object_store.rs`'s own id-recomputation check (`read_object_at_entry`,
/// the content-hash verification a content-addressed store's read path performs) rather than the
/// container's own earlier frame-checksum check one layer up. A bare bit-flip inside a frame's body
/// would fail that earlier checksum instead (a different, less specific error) and never reach the
/// check this fixture exists to exercise.
///
/// Requires the blob container to hold at least two frames of identical total length (magic(8) +
/// version(2) + body_len(8) + checksum(32) header, then body -- `container.rs`'s own frame shape).
/// Swapping two equal-length, individually well-formed frames in place leaves every other frame's
/// offset unchanged and each swapped frame internally self-consistent (its own checksum still
/// matches its own bytes, since the swap moves intact frames rather than editing them) -- but now
/// the object id the repository's index claims for one offset decodes to *the other* frame's
/// content, exactly the disagreement `object_store.rs:130-135` exists to catch. Callers arrange two
/// equal-length, distinct-content blobs (e.g. two files of the same byte length) before sealing.
pub fn swap_two_equal_length_blob_frames(repo: &Path) {
    const MAGIC: &[u8; 8] = b"PCONBLB1";
    const HEADER_LEN: usize = 8 + 2 + 8 + 32;
    let container_path = repo.join(".prikk/containers/blob/a.container");
    let mut bytes = std::fs::read(&container_path).unwrap();

    let mut frames = Vec::new();
    let mut offset = 0usize;
    while offset + HEADER_LEN <= bytes.len() {
        assert_eq!(
            &bytes[offset..offset + 8],
            MAGIC,
            "unexpected frame magic at byte offset {offset} in {container_path:?}"
        );
        let body_len =
            u64::from_be_bytes(bytes[offset + 10..offset + 18].try_into().unwrap()) as usize;
        let frame_len = HEADER_LEN + body_len;
        frames.push((offset, frame_len));
        offset += frame_len;
    }
    assert_eq!(
        offset,
        bytes.len(),
        "blob container has a trailing partial frame -- fixture must seal cleanly first"
    );

    let mut offsets_by_len: std::collections::HashMap<usize, Vec<usize>> =
        std::collections::HashMap::new();
    for &(frame_offset, frame_len) in &frames {
        offsets_by_len
            .entry(frame_len)
            .or_default()
            .push(frame_offset);
    }
    let (frame_a, frame_b) = offsets_by_len
        .into_values()
        .find_map(|offsets| match offsets.as_slice() {
            [a, b, ..] => Some((*a, *b)),
            _ => None,
        })
        .expect(
            "two blob frames of identical total length to swap -- caller must write two blobs of \
             equal byte length before sealing",
        );
    let frame_len = frames.iter().find(|&&(o, _)| o == frame_a).unwrap().1;

    let a_bytes = bytes[frame_a..frame_a + frame_len].to_vec();
    let b_bytes = bytes[frame_b..frame_b + frame_len].to_vec();
    bytes[frame_a..frame_a + frame_len].copy_from_slice(&b_bytes);
    bytes[frame_b..frame_b + frame_len].copy_from_slice(&a_bytes);

    std::fs::write(&container_path, &bytes).unwrap();
}

pub fn copy_dir_recursive(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let file_type = entry.file_type().unwrap();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &dst_path);
        } else {
            std::fs::copy(entry.path(), &dst_path).unwrap();
        }
    }
}

/// DC-67 criterion 2, the load-bearing technique: copy `repo`'s `.prikk` into a fresh directory,
/// `checkout --patch-materialize` it there, and return the rebuilt worktree's root. `verify` passing
/// proves history is *structurally* valid; reading files back from the returned root and asserting
/// their bytes is what proves it is *semantically* correct.
///
/// `checkout --patch-materialize` takes the **repository** path (the directory containing `.prikk`),
/// not an output directory — passing the wrong one is silently plausible and was gotten wrong twice
/// during DC-66 verification.
pub fn rebuild_from_sealed_history(repo: &Path, tag: &str) -> PathBuf {
    let materialize_root = unique_repo(&format!("{tag}-materialize"));
    std::fs::create_dir_all(materialize_root.join(".prikk")).unwrap();
    copy_dir_recursive(&repo.join(".prikk"), &materialize_root.join(".prikk"));
    let out = prikk(&materialize_root)
        .arg("checkout")
        .arg("--patch-materialize")
        .output()
        .unwrap();
    ok(&out, "checkout --patch-materialize");
    materialize_root
}

/// A path's identity for comparison, never its spelling (RFC 132 refusal sweep, Addendum 4). The product
/// prints the form it resolved: on macOS a temp directory's `/var/…` is printed as `/private/var/…`; on
/// Windows the unresolved `C:\Users\RUNNER~1\…` is printed, while `canonicalize` gives `\\?\C:\Users\…`.
/// So both sides go through `canonicalize`. A path that does not exist is its parent's identity joined with
/// its last component.
pub fn path_identity(path: &Path) -> PathBuf {
    if let Ok(resolved) = std::fs::canonicalize(path) {
        return resolved;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => {
            path_identity(parent).join(name)
        }
        _ => path.to_path_buf(),
    }
}

/// Assert that a path the product printed names the same file-system entry as `expected`.
pub fn assert_same_path(printed: &str, expected: &Path) {
    assert_eq!(
        path_identity(Path::new(printed)),
        path_identity(expected),
        "printed path {printed:?} is not {expected:?}"
    );
}

/// Split `text` at the one line containing `prefix`: the rest of that line is a printed path. Returns the
/// text with that path replaced by `<PATH>`, for a byte-for-byte comparison of everything else, and the
/// path itself, for [`assert_same_path`]. Never compare an absolute path as text.
pub fn split_line_path(text: &str, prefix: &str) -> (String, String) {
    let mut path = None;
    let mut rest = String::new();
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let ending = &line[body.len()..];
        match body.find(prefix) {
            Some(at) if path.is_none() => {
                let cut = at + prefix.len();
                path = Some(body[cut..].to_string());
                rest.push_str(&body[..cut]);
                rest.push_str("<PATH>");
                rest.push_str(ending);
            }
            _ => rest.push_str(line),
        }
    }
    (
        rest,
        path.unwrap_or_else(|| panic!("no line containing {prefix:?} in {text:?}")),
    )
}

/// Apply the unified hunks `prikk diff` renders to `left`, and return the result -- **the "renderer cannot
/// lie" applier** (RFC 153 §6.1). It is deliberately a second, independent implementation of what `patch(1)`
/// does, in plain `std`, so it runs on every platform whether or not a `patch` binary exists, and shares no
/// code with the renderer it checks.
///
/// It is strict: a context or deleted line that is not really there at that place is an error, not a
/// shrug. A hunk header's left range says where the hunk starts (an empty range names the line *before* it,
/// as `diff -u` writes it), and `\ No newline at end of file` says the line just above it has no terminator.
pub fn apply_unified_hunks(left: &str, hunks: &[String]) -> Result<String, String> {
    let left_lines: Vec<&str> = left.split_inclusive('\n').collect();
    let mut out = String::new();
    let mut taken = 0usize;
    for hunk in hunks {
        let mut lines: Vec<&str> = hunk.split('\n').collect();
        if lines.pop() != Some("") {
            return Err(format!("a hunk must end with a newline: {hunk:?}"));
        }
        let header = lines.first().ok_or("an empty hunk")?;
        let ranges = header
            .strip_prefix("@@ -")
            .and_then(|rest| rest.strip_suffix(" @@"))
            .ok_or_else(|| format!("not a hunk header: {header:?}"))?;
        let (left_range, right_range) = ranges
            .split_once(" +")
            .ok_or_else(|| format!("no right range in {header:?}"))?;
        let parse_range = |range: &str| -> Result<(usize, usize), String> {
            match range.split_once(',') {
                Some((start, count)) => Ok((
                    start.parse::<usize>().map_err(|e| e.to_string())?,
                    count.parse::<usize>().map_err(|e| e.to_string())?,
                )),
                None => Ok((range.parse::<usize>().map_err(|e| e.to_string())?, 1)),
            }
        };
        let (start, count) = parse_range(left_range)?;
        let (right_start, right_count) = parse_range(right_range)?;
        let before = if count == 0 { start } else { start - 1 };
        while taken < before {
            out.push_str(left_lines.get(taken).ok_or("a hunk starts past the end")?);
            taken += 1;
        }
        // The header is a claim about the body and about where this hunk lands in the right side; `patch`
        // checks both, so this applier does too -- a header that is off by one is a lie, not a cosmetic slip.
        let written_before = out.split_inclusive('\n').count();
        let expected_right_start = if right_count == 0 {
            written_before
        } else {
            written_before + 1
        };
        if right_start != expected_right_start {
            return Err(format!(
                "{header:?}: the right side of this hunk starts at line {expected_right_start}, not {right_start}"
            ));
        }
        let (mut seen_left, mut seen_right) = (0usize, 0usize);
        let body = &lines[1..];
        for (index, line) in body.iter().enumerate() {
            if line.starts_with('\\') {
                continue;
            }
            let unterminated = body
                .get(index + 1)
                .is_some_and(|next| next.starts_with("\\ No newline"));
            let marker = line.chars().next().ok_or("a blank hunk line")?;
            let text = &line[marker.len_utf8()..];
            let content = if unterminated {
                text.to_string()
            } else {
                format!("{text}\n")
            };
            match marker {
                ' ' | '-' => {
                    seen_left += 1;
                    seen_right += usize::from(marker == ' ');
                    let have = left_lines.get(taken).ok_or("a hunk reads past the end")?;
                    if *have != content {
                        return Err(format!(
                            "left line {} is {have:?}, but the hunk says {content:?}",
                            taken + 1
                        ));
                    }
                    taken += 1;
                    if marker == ' ' {
                        out.push_str(&content);
                    }
                }
                '+' => {
                    seen_right += 1;
                    out.push_str(&content);
                }
                other => return Err(format!("an unknown hunk marker {other:?}")),
            }
        }
        if (seen_left, seen_right) != (count, right_count) {
            return Err(format!(
                "{header:?} counts {count} left and {right_count} right lines, but the body has \
                 {seen_left} and {seen_right}"
            ));
        }
    }
    while let Some(line) = left_lines.get(taken) {
        out.push_str(line);
        taken += 1;
    }
    Ok(out)
}

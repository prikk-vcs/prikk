//! 0.59.0 H2c: a seed path the user names must have an existing directory, and `setup` and `key generate` refuse **before** they write anything.
//!
//! Before: `setup <repo> --maintainer-seed-out <missing dir>/m.seed` printed `initialized Prikk repository at …` and then failed on the key-id file, leaving a
//! `.prikk` with no key adopted (a released command that starts what it cannot finish, against RFC 148 §2c). `key generate --out` into a missing directory
//! failed with nothing written, but named `<path>.key-id` and not the path given. prikk does not create the directory: where a secret lives is the user's
//! choice.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]
#![cfg(target_family = "unix")]

use std::path::{Path, PathBuf};
use std::process::Output;

use crate::support;

fn work(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("h2c-{tag}-{}", support::unique_suffix()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// `prikk <args>` in `cwd`, with the test key environment (`support::prikk` isolates the key directory per `cwd`).
fn run(cwd: &Path, args: &[&str]) -> Output {
    support::prikk(cwd).args(args).output().unwrap()
}

/// Every file under `dir`, sorted, relative: what a refusal must leave exactly as it was.
fn listing(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            out.push(
                path.strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            );
            if path.is_dir() {
                walk(&path, base, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// The flag sets: each flag alone (the other role follows the key directory) and both together, each naming `<dir>/<name>`.
fn flag_sets(dir: &Path) -> Vec<(&'static str, Vec<String>)> {
    let seed = |name: &str| dir.join(name).to_string_lossy().into_owned();
    vec![
        (
            "author only",
            vec!["--author-seed-out".into(), seed("a.seed")],
        ),
        (
            "maintainer only",
            vec!["--maintainer-seed-out".into(), seed("m.seed")],
        ),
        (
            "both",
            vec![
                "--author-seed-out".into(),
                seed("a.seed"),
                "--maintainer-seed-out".into(),
                seed("m.seed"),
            ],
        ),
    ]
}

/// **A missing parent, for each flag and both: exit 1, the sentence, no `.prikk`, nothing written.** Red control: the check removed
/// (`require_seed_directory` returning `Ok`): `.prikk` is left behind and `initialized` is printed, and this goes red.
#[test]
fn setup_refuses_a_missing_seed_directory_before_it_initialises_anything() {
    for (label, _) in flag_sets(Path::new("/x")) {
        let cwd = work("setup-missing");
        let project = cwd.join("project");
        let missing = cwd.join("keys/not/there");
        let sets = flag_sets(&missing);
        let (_, flags) = sets.iter().find(|(name, _)| *name == label).unwrap();
        let before = listing(&cwd);
        let mut args = vec!["setup", project.to_str().unwrap()];
        args.extend(flags.iter().map(String::as_str));
        let out = run(&cwd, &args);
        let text = stderr(&out);
        assert_eq!(out.status.code(), Some(1), "{label}: {text}");
        assert!(
            text.contains("cannot write the seed to "),
            "{label}: {text}"
        );
        assert!(
            text.contains(&format!(
                "the directory {} does not exist",
                missing.display()
            )),
            "{label}: names the directory: {text}"
        );
        assert!(
            text.contains(&format!("mkdir -p {}", missing.display())),
            "{label}: and something typeable that works: {text}"
        );
        assert!(
            !stdout(&out).contains("initialized"),
            "{label}: {}",
            stdout(&out)
        );
        assert!(!project.join(".prikk").exists(), "{label}: no .prikk left");
        assert!(!project.exists(), "{label}: not even the project directory");
        assert_eq!(listing(&cwd), before, "{label}: nothing written anywhere");
    }
}

/// **A parent that is a file: the same.**
#[test]
fn setup_refuses_a_seed_directory_that_is_a_file() {
    for (label, _) in flag_sets(Path::new("/x")) {
        let cwd = work("setup-file");
        let project = cwd.join("project");
        let not_a_directory = cwd.join("keys");
        std::fs::write(&not_a_directory, b"a file, not a directory").unwrap();
        let sets = flag_sets(&not_a_directory);
        let (_, flags) = sets.iter().find(|(name, _)| *name == label).unwrap();
        let before = listing(&cwd);
        let mut args = vec!["setup", project.to_str().unwrap()];
        args.extend(flags.iter().map(String::as_str));
        let out = run(&cwd, &args);
        let text = stderr(&out);
        assert_eq!(out.status.code(), Some(1), "{label}: {text}");
        assert!(
            text.contains(&format!("{} is not a directory", not_a_directory.display())),
            "{label}: {text}"
        );
        assert!(
            !project.exists() && !stdout(&out).contains("initialized"),
            "{label}"
        );
        assert_eq!(listing(&cwd), before, "{label}: nothing written");
        assert_eq!(
            std::fs::read(&not_a_directory).unwrap(),
            b"a file, not a directory",
            "{label}: the file is as it was"
        );
    }
}

/// **One good directory and one bad: nothing is written for the good one either** (the check precedes every write).
#[test]
fn a_good_seed_directory_beside_a_bad_one_writes_nothing() {
    let cwd = work("setup-mixed");
    let good = cwd.join("good");
    std::fs::create_dir_all(&good).unwrap();
    let project = cwd.join("project");
    let before = listing(&cwd);
    let out = run(
        &cwd,
        &[
            "setup",
            project.to_str().unwrap(),
            "--author-seed-out",
            good.join("a.seed").to_str().unwrap(),
            "--maintainer-seed-out",
            cwd.join("bad/m.seed").to_str().unwrap(),
        ],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("bad does not exist") || stderr(&out).contains("/bad does not exist"),
        "{}",
        stderr(&out)
    );
    assert_eq!(
        listing(&cwd),
        before,
        "no author seed in the good directory, no repository"
    );
}

/// **`key generate --out` into a missing directory or a file: the same sentence shape, naming the path given, nothing written.**
#[test]
fn key_generate_names_the_path_given_and_writes_nothing() {
    let cwd = work("keygen");
    let missing = cwd.join("nowhere");
    let target = missing.join("x.seed");
    let before = listing(&cwd);
    let out = run(
        &cwd,
        &["key", "generate", "--out", target.to_str().unwrap()],
    );
    let text = stderr(&out);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(
        text.contains(&format!("cannot write the seed to {}", target.display())),
        "{text}"
    );
    assert!(
        !text.contains("x.seed.key-id"),
        "it names the path given, not its key-id file: {text}"
    );
    assert!(
        text.contains(&format!("mkdir -p {}", missing.display())),
        "{text}"
    );
    assert_eq!(listing(&cwd), before, "nothing written");

    let file = cwd.join("afile");
    std::fs::write(&file, b"x").unwrap();
    let before = listing(&cwd);
    let out = run(
        &cwd,
        &[
            "key",
            "generate",
            "--out",
            file.join("x.seed").to_str().unwrap(),
        ],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(&format!("{} is not a directory", file.display())),
        "{}",
        stderr(&out)
    );
    assert_eq!(listing(&cwd), before);
}

/// **Today's success cases are unchanged:** an existing directory, a bare file name in the working directory, and a path inside prikk's own key directory
/// (which prikk creates).
#[test]
fn an_existing_directory_still_works() {
    let cwd = work("ok");
    let keys = cwd.join("keys");
    std::fs::create_dir_all(&keys).unwrap();
    let project = cwd.join("project");
    let out = run(
        &cwd,
        &[
            "setup",
            project.to_str().unwrap(),
            "--author-seed-out",
            keys.join("a.seed").to_str().unwrap(),
            "--maintainer-seed-out",
            keys.join("m.seed").to_str().unwrap(),
        ],
    );
    support::ok(&out, "setup into an existing directory");
    assert!(project.join(".prikk").is_dir());
    for name in ["a.seed", "a.seed.key-id", "m.seed", "m.seed.key-id"] {
        assert!(keys.join(name).is_file(), "{name}");
    }
    let out = run(&cwd, &["key", "generate", "--out", "bare.seed"]);
    support::ok(&out, "key generate with a bare file name");
    assert!(cwd.join("bare.seed").is_file() && cwd.join("bare.seed.key-id").is_file());
}

// ---------------------------------------------------------------------------------------------------------------------------------------------
// 0.59.0 H2d: a seed directory that exists but cannot be written is refused the same way.

/// A directory at `mode` (0500: no write; 0600: write but no search, so nothing can be created in it either) under `cwd`, or `None` when this process is root (root passes `access(2)` whatever the mode, so the refusal cannot be seen: the
/// tests skip rather than pass for the wrong reason).
fn unwritable_directory(cwd: &Path, mode: u32) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    if rustix::process::geteuid().is_root() {
        eprintln!("skipped: running as root, which `access` always passes");
        return None;
    }
    let dir = cwd.join("ro");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode)).unwrap();
    Some(dir)
}

fn make_writable_again(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
}

/// **An unwritable parent, for each flag and both: exit 1, the directory and the OS's words, `chmod u+wx`, no `.prikk`, nothing written.** Red control: the
/// `access` check removed: `.prikk` is left behind, `initialized` is printed.
#[test]
fn setup_refuses_an_unwritable_seed_directory_before_it_initialises_anything() {
    for (label, mode) in flag_sets(Path::new("/x"))
        .into_iter()
        .flat_map(|(label, _)| [(label, 0o500), (label, 0o600)])
    {
        let cwd = work("setup-ro");
        let Some(ro) = unwritable_directory(&cwd, mode) else {
            return;
        };
        let project = cwd.join("project");
        let sets = flag_sets(&ro);
        let (_, flags) = sets.iter().find(|(name, _)| *name == label).unwrap();
        let before = listing(&cwd);
        let mut args = vec!["setup", project.to_str().unwrap()];
        args.extend(flags.iter().map(String::as_str));
        let out = run(&cwd, &args);
        let text = stderr(&out);
        make_writable_again(&ro);
        assert_eq!(out.status.code(), Some(1), "{label}: {text}");
        assert!(
            text.contains(&format!(
                "the directory {} is not writable: Permission denied",
                ro.display()
            )),
            "{label}: names the directory and the OS's words: {text}"
        );
        assert!(
            text.contains(&format!("chmod u+wx {}", ro.display())),
            "{label}: and something typeable: {text}"
        );
        assert!(
            !stdout(&out).contains("initialized"),
            "{label}: {}",
            stdout(&out)
        );
        assert!(
            !project.exists(),
            "{label}: no .prikk, no project directory"
        );
        assert_eq!(listing(&cwd), before, "{label}: nothing written");
    }
}

/// **`key generate --out` into an unwritable directory:** the same sentence, nothing written.
#[test]
fn key_generate_refuses_an_unwritable_seed_directory() {
    let cwd = work("keygen-ro");
    let Some(ro) = unwritable_directory(&cwd, 0o500) else {
        return;
    };
    let target = ro.join("x.seed");
    let before = listing(&cwd);
    let out = run(
        &cwd,
        &["key", "generate", "--out", target.to_str().unwrap()],
    );
    make_writable_again(&ro);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(&format!(
            "cannot write the seed to {}: the directory {} is not writable",
            target.display(),
            ro.display()
        )),
        "{}",
        stderr(&out)
    );
    assert_eq!(listing(&cwd), before, "nothing written");
}

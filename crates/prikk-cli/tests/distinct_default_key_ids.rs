//! Distinct default key ids (`rfcs/handoffs/135-first-run-entrance-and-configuration/distinct-default-key-ids-handoff-v1.md`,
//! addendum 1), through the compiled binary.
//!
//! An **installation** here is a config home of its own (`XDG_CONFIG_HOME`, `HOME` and `APPDATA` all
//! pointing at it, every `PRIKK_*` key variable removed), so two installations on one machine are as
//! separate as two machines. A **legacy** installation holds seeds with no key-id file — the shape
//! 0.44.0's `setup` left.
//!
//! Controls 5 and the `key generate --out` routes of control 6 are Unix-only because `key generate --out`
//! refuses outright on Windows (`key.rs::write_seed_to_path_platform`); everything else runs everywhere.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use prikk_object::{ObjectId, SignerRole};
use prikk_store::{FileObjectStore, ObjectReader, RepositoryLayout};

const LEGACY_AUTHOR_SEED: &str = "11aa22bb33cc44dd55ee66ff77008899aabbccddeeff00112233445566778899";
const LEGACY_MAINTAINER_SEED: &str =
    "99887766554433221100ffeeddccbbaa99887766554433221100ffeeddccbbaa";

struct Installation {
    home: PathBuf,
}

impl Installation {
    fn new(tag: &str) -> Self {
        let home = support::unique_repo(&format!("{tag}-home"));
        Self { home }
    }

    /// A 0.44.0-shaped installation: both seeds in the key directory, no key-id file. `salt` makes two
    /// legacy installations hold different keys.
    fn legacy(tag: &str, salt: u8) -> Self {
        let installation = Self::new(tag);
        let dir = installation.key_dir();
        std::fs::create_dir_all(&dir).unwrap();
        for (name, hex) in [
            ("author.seed", LEGACY_AUTHOR_SEED),
            ("maintainer.seed", LEGACY_MAINTAINER_SEED),
        ] {
            let mut bytes = hex.to_string();
            bytes.replace_range(0..2, &format!("{salt:02x}"));
            write_private(&dir.join(name), &format!("{bytes}\n"));
        }
        installation
    }

    fn key_dir(&self) -> PathBuf {
        self.home.join("prikk")
    }

    fn command(&self, dir: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_prikk"));
        command
            .current_dir(dir)
            .env("XDG_CONFIG_HOME", &self.home)
            .env("HOME", &self.home)
            .env("APPDATA", &self.home);
        for var in [
            "PRIKK_AUTHOR_SEED",
            "PRIKK_MAINTAINER_SEED",
            "PRIKK_AUTHOR_SEED_FILE",
            "PRIKK_MAINTAINER_SEED_FILE",
            "PRIKK_AUTHOR_KEY_ID",
            "PRIKK_MAINTAINER_KEY_ID",
        ] {
            command.env_remove(var);
        }
        command
    }

    fn run(&self, dir: &Path, args: &[&str]) -> Output {
        self.command(dir).args(args).output().unwrap()
    }

    fn run_with(&self, dir: &Path, env: &[(&str, &str)], args: &[&str]) -> Output {
        let mut command = self.command(dir);
        for (name, value) in env {
            command.env(name, value);
        }
        command.args(args).output().unwrap()
    }

    fn setup(&self, tag: &str) -> PathBuf {
        let repo = support::unique_repo(tag);
        let out = self.run(&repo, &["setup", "."]);
        support::ok(&out, "setup");
        repo
    }

    fn status_field(&self, dir: &Path, env: &[(&str, &str)], role: &str, field: &str) -> String {
        let out = self.run_with(
            dir,
            env,
            &["key", "status", "--role", role, "--format", "json"],
        );
        support::ok(&out, "key status");
        json_field(&text(&out), field)
    }

    fn public_key(&self, dir: &Path, role: &str) -> String {
        let out = self.run(dir, &["key", "public", "--role", role]);
        support::ok(&out, "key public");
        text(&out)
            .trim()
            .strip_prefix("public key: ")
            .unwrap()
            .to_string()
    }

    /// Write a file, commit it, seal it, export `heads/main`; returns the bundle.
    fn commit_seal_export(&self, repo: &Path, env: &[(&str, &str)], name: &str) -> PathBuf {
        std::fs::write(repo.join(name), format!("{name}\n")).unwrap();
        support::ok(&self.run_with(repo, env, &["commit", "-m", name]), "commit");
        support::ok(
            &self.run_with(repo, env, &["seal", "--allow-no-audit"]),
            "seal",
        );
        let bundle = repo.join(format!("{name}.bundle"));
        support::ok(
            &self.run_with(
                repo,
                env,
                &[
                    "bundle",
                    "export",
                    "--ref",
                    "heads/main",
                    "--output",
                    bundle.to_str().unwrap(),
                ],
            ),
            "bundle export",
        );
        bundle
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn json_field(json: &str, field: &str) -> String {
    let prefix = format!("\"{field}\": ");
    json.lines()
        .find_map(|line| line.trim().strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("no {field} in {json}"))
        .trim_end_matches(',')
        .trim_matches('"')
        .to_string()
}

#[cfg(unix)]
fn write_private(path: &Path, contents: &str) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(contents.as_bytes()).unwrap();
}

#[cfg(not(unix))]
fn write_private(path: &Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
}

fn import(installation: &Installation, repo: &Path, bundle: &Path) -> Output {
    installation.run(
        repo,
        &["bundle", "import", "--input", bundle.to_str().unwrap()],
    )
}

fn is_derived(id: &str) -> bool {
    id.len() == 24
        && id.strip_prefix("ed25519-").is_some_and(|hex| {
            hex.chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        })
}

/// Control 1: two fresh `setup`s get distinct ids; A adopts B's maintainer under B's id, imports B's
/// bundle and verifies.
#[test]
fn control1_two_fresh_setups_exchange_history() {
    let a = Installation::new("keyids-c1-a");
    let b = Installation::new("keyids-c1-b");
    let repo_a = a.setup("keyids-c1-repo-a");
    let repo_b = b.setup("keyids-c1-repo-b");

    let ids: Vec<String> = [(&a, &repo_a), (&b, &repo_b)]
        .iter()
        .flat_map(|(installation, repo)| {
            ["author", "maintainer"]
                .map(|role| installation.status_field(repo, &[], role, "key_id"))
        })
        .collect();
    for id in &ids {
        assert!(is_derived(id), "a new key gets a derived id: {id}");
    }
    let unique: std::collections::BTreeSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), 4, "every new key has its own id: {ids:?}");
    assert_eq!(
        a.status_field(&repo_a, &[], "author", "key_id_source"),
        "key-file"
    );

    std::fs::write(repo_a.join("a.txt"), "a\n").unwrap();
    support::ok(&a.run(&repo_a, &["commit", "-m", "a"]), "A commits");
    let bundle = b.commit_seal_export(&repo_b, &[], "b.txt");
    let b_maintainer = ids[3].clone();
    support::ok(
        &a.run(
            &repo_a,
            &[
                "trust",
                "maintainer",
                "add",
                "--key-id",
                &b_maintainer,
                "--public-key",
                &b.public_key(&repo_b, "maintainer"),
            ],
        ),
        "A adopts B's maintainer under B's id",
    );
    support::ok(&import(&a, &repo_a, &bundle), "A imports B's bundle");
    let verify = a.run(&repo_a, &["verify"]);
    assert_eq!(verify.status.code(), Some(0), "{}", text(&verify));
}

/// Control 2: an existing installation keeps `author`/`maintainer`, and its repository still commits,
/// seals and verifies.
#[test]
fn control2_an_existing_installation_keeps_its_ids() {
    let legacy = Installation::legacy("keyids-c2", 0x21);
    let repo = support::unique_repo("keyids-c2-repo");
    support::ok(&legacy.run(&repo, &["init"]), "init");
    let maintainer_public = legacy.public_key(&repo, "maintainer");
    support::ok(
        &legacy.run(
            &repo,
            &[
                "trust",
                "maintainer",
                "add",
                "--key-id",
                "maintainer",
                "--public-key",
                &maintainer_public,
            ],
        ),
        "adopt as 0.44.0's setup did",
    );
    assert_eq!(
        legacy.status_field(&repo, &[], "author", "key_id"),
        "author"
    );
    assert_eq!(
        legacy.status_field(&repo, &[], "maintainer", "key_id"),
        "maintainer"
    );
    assert_eq!(
        legacy.status_field(&repo, &[], "author", "key_id_source"),
        "default"
    );
    legacy.commit_seal_export(&repo, &[], "legacy.txt");
    let verify = legacy.run(&repo, &["verify"]);
    assert_eq!(verify.status.code(), Some(0), "{}", text(&verify));

    // A second project on the same keys reuses them under the legacy ids, and says so.
    let second = support::unique_repo("keyids-c2-second");
    let out = legacy.run(&second, &["setup", "."]);
    support::ok(&out, "setup reusing legacy keys");
    let out = text(&out);
    assert!(out.contains("trusted maintainer key: maintainer"), "{out}");
    assert!(
        out.contains("shared legacy key id `author`") && out.contains("PRIKK_AUTHOR_KEY_ID"),
        "{out}"
    );
    assert!(!legacy.key_dir().join("author.key-id").exists());
}

/// Control 3: `PRIKK_MAINTAINER_KEY_ID=bob prikk setup` adopts `bob`, and prints it.
#[test]
fn control3_setup_honours_the_maintainer_key_id_variable() {
    let installation = Installation::new("keyids-c3");
    let repo = support::unique_repo("keyids-c3-repo");
    let out = installation.run_with(
        &repo,
        &[("PRIKK_MAINTAINER_KEY_ID", "bob")],
        &["setup", "."],
    );
    support::ok(&out, "setup");
    assert!(
        text(&out).contains("trusted maintainer key: bob"),
        "{}",
        text(&out)
    );
    let binding = installation.status_field(
        &repo,
        &[("PRIKK_MAINTAINER_KEY_ID", "bob")],
        "maintainer",
        "binding",
    );
    assert_eq!(binding, "matches");
}

/// Control 4: a key-id file that does not match its seed refuses signing, and `key status` reports it
/// unusable, naming the file and both ids.
#[test]
fn control4_a_key_id_file_that_is_not_its_seeds_refuses_signing() {
    let installation = Installation::new("keyids-c4");
    let repo = installation.setup("keyids-c4-repo");
    let derived = installation.status_field(&repo, &[], "author", "key_id");
    let file = installation.key_dir().join("author.key-id");
    std::fs::write(&file, "ed25519-0000000000000000\n").unwrap();

    std::fs::write(repo.join("f.txt"), "f\n").unwrap();
    let refused = installation.run(&repo, &["commit", "-m", "f"]);
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused));
    let message = text(&refused);
    assert!(
        message.contains("ed25519-0000000000000000")
            && message.contains(&derived)
            && message.contains(&file.display().to_string()),
        "{message}"
    );

    assert_eq!(
        installation.status_field(&repo, &[], "author", "usable"),
        "false"
    );
    assert_eq!(
        installation.status_field(&repo, &[], "author", "reason"),
        "key-id-file-mismatch"
    );
    let prose = text(&installation.run(&repo, &["key", "status", "--role", "author"]));
    assert!(
        prose.contains(&file.display().to_string())
            && prose.contains("ed25519-0000000000000000")
            && prose.contains(&derived),
        "{prose}"
    );
}

/// Control 5: `key generate --out p` writes `p.key-id` (0600), and `key status` with
/// `PRIKK_AUTHOR_SEED_FILE=p` reports `key_id_source: "key-file"`. A leftover key-id file refuses a new
/// seed at that path, writing neither.
#[cfg(unix)]
#[test]
fn control5_key_generate_writes_the_key_id_file() {
    use std::os::unix::fs::PermissionsExt;

    let installation = Installation::new("keyids-c5");
    let dir = support::unique_repo("keyids-c5-dir");
    let seed = dir.join("p");
    support::ok(
        &installation.run(&dir, &["key", "generate", "--out", seed.to_str().unwrap()]),
        "key generate",
    );
    let key_id_file = dir.join("p.key-id");
    let contents = std::fs::read_to_string(&key_id_file).unwrap();
    let id = contents.strip_suffix('\n').unwrap();
    assert!(is_derived(id), "{contents:?}");
    assert_eq!(
        std::fs::metadata(&key_id_file)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let env = [("PRIKK_AUTHOR_SEED_FILE", seed.to_str().unwrap())];
    assert_eq!(
        installation.status_field(&dir, &env, "author", "key_id_source"),
        "key-file"
    );
    assert_eq!(
        installation.status_field(&dir, &env, "author", "key_id"),
        id
    );
    assert_eq!(
        installation.status_field(&dir, &env, "author", "usable"),
        "true"
    );

    std::fs::remove_file(&seed).unwrap();
    let refused = installation.run(&dir, &["key", "generate", "--out", seed.to_str().unwrap()]);
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused));
    assert!(
        text(&refused).contains(&key_id_file.display().to_string()),
        "{}",
        text(&refused)
    );
    assert!(!seed.exists(), "a refused generate writes no seed");
}

/// Two legacy installations, A and B, each with a repository set up the 0.44.0 way.
fn two_legacy(tag: &str) -> (Installation, PathBuf, Installation, PathBuf) {
    let a = Installation::legacy(&format!("{tag}-a"), 0x41);
    let b = Installation::legacy(&format!("{tag}-b"), 0x42);
    let mut repos = Vec::new();
    for installation in [&a, &b] {
        let repo = support::unique_repo(&format!("{tag}-repo"));
        support::ok(&installation.run(&repo, &["init"]), "init");
        let public = installation.public_key(&repo, "maintainer");
        support::ok(
            &installation.run(
                &repo,
                &[
                    "trust",
                    "maintainer",
                    "add",
                    "--key-id",
                    "maintainer",
                    "--public-key",
                    &public,
                ],
            ),
            "adopt",
        );
        repos.push(repo);
    }
    let repo_b = repos.pop().unwrap();
    let repo_a = repos.pop().unwrap();
    (a, repo_a, b, repo_b)
}

/// B makes a new repository and history signed under `env`'s ids, and exports it.
fn distinct_history(b: &Installation, env: &[(&str, &str)], tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    let out = b.run_with(&repo, env, &["setup", "."]);
    support::ok(&out, "setup under distinct ids");
    b.commit_seal_export(&repo, env, "distinct.txt")
}

/// Control 6, the maintainer refusal: the advice runs from the refusing state.
#[test]
fn control6_the_maintainer_refusal_names_a_route_that_runs() {
    let (a, repo_a, b, repo_b) = two_legacy("keyids-c6m");
    std::fs::write(repo_a.join("a.txt"), "a\n").unwrap();
    support::ok(&a.run(&repo_a, &["commit", "-m", "a"]), "A commits");
    let b_public = b.public_key(&repo_b, "maintainer");
    let refused = a.run(
        &repo_a,
        &[
            "trust",
            "maintainer",
            "add",
            "--key-id",
            "maintainer",
            "--public-key",
            &b_public,
        ],
    );
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused));
    let message = text(&refused);
    for named in [
        "PRIKK_MAINTAINER_KEY_ID",
        "prikk key generate --out",
        "PRIKK_MAINTAINER_SEED_FILE",
        "prikk trust maintainer add --key-id",
    ] {
        assert!(
            message.contains(named),
            "the refusal names {named}: {message}"
        );
    }
    assert!(
        !message.contains("adopt the new key under a different id"),
        "{message}"
    );

    // Route 1: B signs under PRIKK_MAINTAINER_KEY_ID (and a distinct author id, so the import meets no
    // author collision either), A adopts under that id, imports, verifies.
    let env = [
        ("PRIKK_MAINTAINER_KEY_ID", "b-maintainer"),
        ("PRIKK_AUTHOR_KEY_ID", "b-author"),
    ];
    let bundle = distinct_history(&b, &env, "keyids-c6m-b2");
    support::ok(
        &a.run(
            &repo_a,
            &[
                "trust",
                "maintainer",
                "add",
                "--key-id",
                "b-maintainer",
                "--public-key",
                &b_public,
            ],
        ),
        "adopt under B's distinct id",
    );
    support::ok(&import(&a, &repo_a, &bundle), "import");
    let verify = a.run(&repo_a, &["verify"]);
    assert_eq!(verify.status.code(), Some(0), "{}", text(&verify));

    // Route 2 (Unix: `key generate --out`): a generated key, pointed at by PRIKK_MAINTAINER_SEED_FILE.
    #[cfg(unix)]
    {
        let keys = support::unique_repo("keyids-c6m-keys");
        let seed = keys.join("maintainer.seed");
        support::ok(
            &b.run(&keys, &["key", "generate", "--out", seed.to_str().unwrap()]),
            "generate",
        );
        let author_seed = keys.join("author.seed");
        support::ok(
            &b.run(
                &keys,
                &["key", "generate", "--out", author_seed.to_str().unwrap()],
            ),
            "generate author",
        );
        let env = [
            ("PRIKK_MAINTAINER_SEED_FILE", seed.to_str().unwrap()),
            ("PRIKK_AUTHOR_SEED_FILE", author_seed.to_str().unwrap()),
        ];
        let bundle = distinct_history(&b, &env, "keyids-c6m-b3");
        let generated_id = b.status_field(&keys, &env, "maintainer", "key_id");
        let generated_public = {
            let out = b.run(
                &keys,
                &["key", "public", "--seed-file", seed.to_str().unwrap()],
            );
            text(&out)
                .trim()
                .strip_prefix("public key: ")
                .unwrap()
                .to_string()
        };
        support::ok(
            &a.run(
                &repo_a,
                &[
                    "trust",
                    "maintainer",
                    "add",
                    "--key-id",
                    &generated_id,
                    "--public-key",
                    &generated_public,
                ],
            ),
            "adopt the generated key under its id",
        );
        support::ok(
            &import(&a, &repo_a, &bundle),
            "import generated-key history",
        );
        let verify = a.run(&repo_a, &["verify"]);
        assert_eq!(verify.status.code(), Some(0), "{}", text(&verify));
    }
}

/// Control 6, the author refusal at import: history under `author` from another key cannot enter; the
/// route — new history under a distinct id — imports.
#[test]
fn control6_the_author_refusal_at_import_names_a_route_that_runs() {
    let (a, repo_a, b, repo_b) = two_legacy("keyids-c6a");
    std::fs::write(repo_a.join("a.txt"), "a\n").unwrap();
    support::ok(&a.run(&repo_a, &["commit", "-m", "a"]), "A commits");
    let legacy_bundle = b.commit_seal_export(&repo_b, &[], "b.txt");
    let refused = import(&a, &repo_a, &legacy_bundle);
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused));
    let message = text(&refused);
    for named in [
        "legacy default key id `author`",
        "cannot enter this repository",
        "PRIKK_AUTHOR_KEY_ID",
        "prikk key generate --out",
        "PRIKK_AUTHOR_SEED_FILE",
    ] {
        assert!(
            message.contains(named),
            "the refusal names {named}: {message}"
        );
    }
    assert!(!message.contains("key-rotation attempt"), "{message}");

    let env = [("PRIKK_AUTHOR_KEY_ID", "b-author")];
    let bundle = distinct_history(&b, &env, "keyids-c6a-b2");
    support::ok(
        &import(&a, &repo_a, &bundle),
        "new history under a distinct id imports",
    );

    #[cfg(unix)]
    {
        let keys = support::unique_repo("keyids-c6a-keys");
        let author_seed = keys.join("author.seed");
        support::ok(
            &b.run(
                &keys,
                &["key", "generate", "--out", author_seed.to_str().unwrap()],
            ),
            "generate",
        );
        let env = [("PRIKK_AUTHOR_SEED_FILE", author_seed.to_str().unwrap())];
        let bundle = distinct_history(&b, &env, "keyids-c6a-b3");
        support::ok(
            &import(&a, &repo_a, &bundle),
            "generated-key history imports",
        );
    }
}

/// Control 6, the author refusal at commit: after an import bound `author` to the other key, `commit`
/// refuses with the same advice, and signing under a distinct id commits.
#[test]
fn control6_the_author_refusal_at_commit_names_a_route_that_runs() {
    let (a, repo_a, b, repo_b) = two_legacy("keyids-c6c");
    let legacy_bundle = b.commit_seal_export(&repo_b, &[], "b.txt");
    support::ok(
        &import(&a, &repo_a, &legacy_bundle),
        "A imports first, never having committed",
    );
    std::fs::write(repo_a.join("a.txt"), "a\n").unwrap();
    let refused = a.run(&repo_a, &["commit", "-m", "a"]);
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused));
    let message = text(&refused);
    assert!(
        message.contains("legacy default key id `author`")
            && message.contains("PRIKK_AUTHOR_KEY_ID"),
        "{message}"
    );

    support::ok(
        &a.run_with(
            &repo_a,
            &[("PRIKK_AUTHOR_KEY_ID", "a-author")],
            &["commit", "-m", "a"],
        ),
        "commit under a distinct id",
    );

    #[cfg(unix)]
    {
        let keys = support::unique_repo("keyids-c6c-keys");
        let author_seed = keys.join("author.seed");
        support::ok(
            &a.run(
                &keys,
                &["key", "generate", "--out", author_seed.to_str().unwrap()],
            ),
            "generate",
        );
        std::fs::write(repo_a.join("a2.txt"), "a2\n").unwrap();
        support::ok(
            &a.run_with(
                &repo_a,
                &[("PRIKK_AUTHOR_SEED_FILE", author_seed.to_str().unwrap())],
                &["commit", "-m", "a2"],
            ),
            "commit with a generated key",
        );
    }
}

/// Commit and seal one file, and return the AUTHOR key id on the sealed patch.
fn signed_author_id(
    installation: &Installation,
    repo: &Path,
    env: &[(&str, &str)],
    name: &str,
) -> String {
    std::fs::write(repo.join(name), format!("{name}\n")).unwrap();
    let commit = installation.run_with(repo, env, &["commit", "-m", name]);
    support::ok(&commit, "commit");
    support::ok(
        &installation.run_with(repo, env, &["seal", "--allow-no-audit"]),
        "seal",
    );
    let patch_id: ObjectId = text(&commit)
        .lines()
        .find_map(|line| line.strip_prefix("patch id: "))
        .expect("commit prints a patch id")
        .trim()
        .parse()
        .unwrap();
    let envelope = FileObjectStore::new(RepositoryLayout::open(repo.to_path_buf()).unwrap())
        .read_object(patch_id)
        .unwrap()
        .expect("the sealed patch is stored");
    envelope
        .signatures
        .iter()
        .find(|signature| signature.signer_role == SignerRole::Author)
        .expect("an AUTHOR signature")
        .key_id
        .clone()
}

/// Control 7: resolution is one function — the id `key status` reports is the id `commit` signs under,
/// in every state: a key-id file, the legacy default, and the environment.
#[test]
fn control7_key_status_reports_the_id_commit_signs_under() {
    let fresh = Installation::new("keyids-c7-fresh");
    let repo = fresh.setup("keyids-c7-fresh-repo");
    let reported = fresh.status_field(&repo, &[], "author", "key_id");
    assert_eq!(
        fresh.status_field(&repo, &[], "author", "key_id_source"),
        "key-file"
    );
    assert_eq!(signed_author_id(&fresh, &repo, &[], "file.txt"), reported);

    let env = [("PRIKK_AUTHOR_KEY_ID", "c7-environment")];
    assert_eq!(
        fresh.status_field(&repo, &env, "author", "key_id"),
        "c7-environment"
    );
    assert_eq!(
        signed_author_id(&fresh, &repo, &env, "env.txt"),
        "c7-environment"
    );

    let (legacy, legacy_repo, _, _) = two_legacy("keyids-c7-legacy");
    let reported = legacy.status_field(&legacy_repo, &[], "author", "key_id");
    assert_eq!(reported, "author");
    assert_eq!(
        signed_author_id(&legacy, &legacy_repo, &[], "legacy.txt"),
        reported
    );
}

/// Addendum 2, F3: `key generate` without `--out` prints instructions that, followed literally, give a key
/// on its own id — both files saved as printed, `key status` reports `key-file` with the printed id, and a
/// seal adopted under the printed id verifies.
#[test]
fn f3_key_generate_without_out_prints_instructions_that_keep_the_distinct_id() {
    let installation = Installation::new("keyids-f3");
    let dir = support::unique_repo("keyids-f3-dir");
    let out = installation.run(&dir, &["key", "generate"]);
    support::ok(&out, "key generate");
    let printed = text(&out);
    let field = |prefix: &str| -> String {
        printed
            .lines()
            .find_map(|line| line.trim().strip_prefix(prefix))
            .unwrap_or_else(|| panic!("no {prefix:?} in {printed}"))
            .trim()
            .to_string()
    };
    let seed_hex = field("seed: ");
    let key_id = field("key id: ");
    assert!(is_derived(&key_id), "{printed}");
    assert!(
        printed.contains("recommended: re-run with --out <path>"),
        "{printed}"
    );
    assert!(!printed.contains("--key-id maintainer "), "{printed}");
    let trust_line = field("prikk trust maintainer add ");

    // The two hand-saved files, exactly as printed.
    let mut saved = Vec::new();
    for line in printed.lines() {
        let line = line.trim();
        if let Some((path, what)) = line.split_once("  (") {
            if what.starts_with("the seed above") {
                saved.push((PathBuf::from(path), format!("{seed_hex}\n")));
            } else if what.starts_with(&format!("exactly {key_id}")) {
                saved.push((PathBuf::from(path), format!("{key_id}\n")));
            }
        }
    }
    assert_eq!(saved.len(), 2, "both files are named: {printed}");
    std::fs::create_dir_all(installation.key_dir()).unwrap();
    for (path, contents) in &saved {
        write_private(path, contents);
    }
    // The AUTHOR note, followed literally: the same two files under author.* names.
    for (path, contents) in &saved {
        let name = path.file_name().unwrap().to_str().unwrap();
        let author = path.with_file_name(name.replacen("maintainer", "author", 1));
        write_private(&author, contents);
    }

    assert_eq!(
        installation.status_field(&dir, &[], "maintainer", "key_id_source"),
        "key-file"
    );
    assert_eq!(
        installation.status_field(&dir, &[], "maintainer", "key_id"),
        key_id
    );
    assert_eq!(
        installation.status_field(&dir, &[], "maintainer", "usable"),
        "true"
    );

    let repo = support::unique_repo("keyids-f3-repo");
    support::ok(&installation.run(&repo, &["init"]), "init");
    let mut trust_args = vec!["trust", "maintainer", "add"];
    trust_args.extend(trust_line.split_whitespace());
    support::ok(
        &installation.run(&repo, &trust_args),
        "the printed trust line",
    );
    installation.commit_seal_export(&repo, &[], "f3.txt");
    let verify = installation.run(&repo, &["verify"]);
    assert_eq!(verify.status.code(), Some(0), "{}", text(&verify));
}

/// Addendum 3: a seed write that refuses leaves no key-id file. `key generate --out` into `.prikk/` refuses,
/// and neither the seed nor its key-id file exists after; a retry at a valid path then succeeds with nothing
/// left over to refuse on. The `.prikk` directory exists, so a key-id file written before the refusal
/// would really be there.
#[test]
fn addendum3_a_refused_seed_write_leaves_no_key_id_file() {
    let installation = Installation::new("keyids-a3");
    let dir = support::unique_repo("keyids-a3-dir");
    std::fs::create_dir_all(dir.join(".prikk")).unwrap();
    let refused_seed = dir.join(".prikk").join("inside.seed");
    let refused = installation.run(
        &dir,
        &["key", "generate", "--out", refused_seed.to_str().unwrap()],
    );
    assert_eq!(refused.status.code(), Some(2), "{}", text(&refused));
    assert!(text(&refused).contains(".prikk"), "{}", text(&refused));
    assert!(!refused_seed.exists(), "no seed");
    assert!(
        !dir.join(".prikk").join("inside.seed.key-id").exists(),
        "no key-id file left by the refusal"
    );

    // Unix: `key generate --out` refuses outright on Windows, so the valid retry is Unix-only.
    #[cfg(unix)]
    {
        let seed = dir.join("outside.seed");
        support::ok(
            &installation.run(&dir, &["key", "generate", "--out", seed.to_str().unwrap()]),
            "retry at a valid path",
        );
        assert!(seed.exists() && dir.join("outside.seed.key-id").exists());
    }
}

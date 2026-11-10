#![allow(clippy::unwrap_used)]

use super::generate;

#[test]
fn writes_both_scripts_with_the_placeholder_substituted() {
    let temporary = tempfile::tempdir().unwrap();
    generate(temporary.path()).unwrap();

    let install = std::fs::read_to_string(temporary.path().join("install.sh")).unwrap();
    let uninstall = std::fs::read_to_string(temporary.path().join("uninstall.sh")).unwrap();

    assert!(install.starts_with("#!/bin/sh"));
    assert!(uninstall.starts_with("#!/bin/sh"));
    assert!(install.contains("prikk-vcs/prikk"));
    assert!(uninstall.contains("prikk-vcs/prikk"));
    assert!(
        !install.contains("REPO_SLUG"),
        "the placeholder must not survive substitution"
    );
    assert!(
        !uninstall.contains("REPO_SLUG"),
        "the placeholder must not survive substitution"
    );
}

// Windows installer/uninstaller handoff v1 Part 6a: the same coverage as the `.sh` pair above,
// for the new `.ps1` pair -- placeholder substitution is the one thing `generate()` actually does
// to either template's text, so it is the one thing worth asserting per pair.
#[test]
fn writes_both_powershell_scripts_with_the_placeholder_substituted() {
    let temporary = tempfile::tempdir().unwrap();
    generate(temporary.path()).unwrap();

    let install = std::fs::read_to_string(temporary.path().join("install.ps1")).unwrap();
    let uninstall = std::fs::read_to_string(temporary.path().join("uninstall.ps1")).unwrap();

    assert!(install.starts_with("# Prikk installer"));
    assert!(uninstall.starts_with("# Prikk uninstaller"));
    assert!(install.contains("prikk-vcs/prikk"));
    assert!(uninstall.contains("prikk-vcs/prikk"));
    assert!(
        !install.contains("REPO_SLUG"),
        "the placeholder must not survive substitution"
    );
    assert!(
        !uninstall.contains("REPO_SLUG"),
        "the placeholder must not survive substitution"
    );
}

#[cfg(unix)]
#[test]
fn generated_scripts_are_executable() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().unwrap();
    generate(temporary.path()).unwrap();

    for name in ["install.sh", "uninstall.sh"] {
        let mode = std::fs::metadata(temporary.path().join(name))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o111,
            0o111,
            "{name} must be executable by owner, group, and other"
        );
    }
}

// Windows installer/uninstaller handoff v1 Part 6a: the `.ps1` pair is never marked executable by
// this tool (Windows has no equivalent of the Unix execute bit for `mark_executable` to set, and
// neither `irm ... | iex` nor `powershell -File` needs one) -- asserted on every platform this
// workspace runs tests on, unlike `generated_scripts_are_executable` above which is Unix-only
// because it reads a Unix-only permission bit.
#[test]
fn generated_powershell_scripts_carry_no_unix_permission_bits() {
    let temporary = tempfile::tempdir().unwrap();
    generate(temporary.path()).unwrap();

    for name in ["install.ps1", "uninstall.ps1"] {
        let path = temporary.path().join(name);
        assert!(path.is_file(), "{name} must exist in dist_dir");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o111,
                0,
                "{name} must not be marked executable -- mark_executable is never called for it"
            );
        }
    }
}

#[test]
fn creates_the_output_directory_if_missing() {
    let temporary = tempfile::tempdir().unwrap();
    let nested = temporary.path().join("does-not-exist-yet");
    generate(&nested).unwrap();
    assert!(nested.join("install.sh").is_file());
    assert!(nested.join("install.ps1").is_file());
}

// 0.56.0 step U: uninstall.sh never read its own arguments at all before this round -- `--prefix`
// was silently ignored and the default-location binary was removed instead (found by an external
// consumer, "stikk"; see CHANGELOG.md's own 0.56.0 "Fixed" entry). Unlike every test above, which
// only checks the generated *text*, this measures it the way a user meets it: generate the real
// uninstall.sh, run it with `sh` against real scratch directories standing in for "installed"
// locations, and check which files survive. Unix-only, like `generated_scripts_are_executable`
// above: uninstall.sh is a POSIX shell script that refuses outright on Windows's own shells, and
// `sh` is not reliably on PATH there either; `#[cfg(unix)]` covers Linux and macOS, both of which
// already run `cargo test --workspace` in CI (DC-81), while never adding this crate's tests to the
// windows-mutation job's own surface.
#[cfg(unix)]
mod uninstall_sh_execution {
    use super::generate;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    /// Generate the real uninstall.sh into a fresh temp `dist_dir` and return it together with
    /// the `TempDir` guard (drop it only once the caller is done with the path).
    fn generated_uninstall_sh() -> (tempfile::TempDir, PathBuf) {
        let dist = tempfile::tempdir().unwrap();
        generate(dist.path()).unwrap();
        let path = dist.path().join("uninstall.sh");
        (dist, path)
    }

    /// A fake "installed" prikk binary. uninstall.sh only ever checks `[ -f "$binary_path" ]`
    /// before removing it -- it never execs or inspects the file's own contents -- so any regular
    /// file stands in for a real binary here.
    fn write_fake_binary(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join("prikk");
        fs::write(&path, b"fake prikk binary").unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
    }

    /// Run the generated uninstall.sh with `sh`, in a subprocess whose `$HOME` is a test-scoped
    /// temp directory -- never the real one -- and whose `PRIKK_INSTALL_DIR` is either set to the
    /// given value or removed outright (never inherited from this test process's own environment).
    fn run_uninstall(
        script: &Path,
        args: &[&str],
        home: &Path,
        install_dir_env: Option<&Path>,
    ) -> Output {
        let mut command = Command::new("sh");
        command.arg(script).args(args).env("HOME", home);
        match install_dir_env {
            Some(value) => command.env("PRIKK_INSTALL_DIR", value),
            None => command.env_remove("PRIKK_INSTALL_DIR"),
        };
        command.output().unwrap()
    }

    /// **The case stikk measured, "the way a user meets it."** A decoy binary sits at the default
    /// location ($HOME/.local/bin) and the real target sits at a custom --prefix, with
    /// PRIKK_INSTALL_DIR unset. Before this round, --prefix was silently ignored: the decoy was
    /// removed and the real target was left behind -- exactly backwards. The decoy must survive,
    /// and the named prefix's copy must go.
    #[test]
    fn prefix_flag_removes_the_named_prefix_and_leaves_the_default_location_alone() {
        let (_dist, script) = generated_uninstall_sh();
        let home = tempfile::tempdir().unwrap();
        let custom = tempfile::tempdir().unwrap();
        let decoy_dir = home.path().join(".local/bin");
        write_fake_binary(&decoy_dir);
        write_fake_binary(custom.path());

        let output = run_uninstall(
            &script,
            &["--prefix", custom.path().to_str().unwrap()],
            home.path(),
            None,
        );

        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !custom.path().join("prikk").exists(),
            "the named --prefix binary must be gone"
        );
        assert!(
            decoy_dir.join("prikk").exists(),
            "the decoy at the default location must survive"
        );
    }

    /// An argument this script does not recognize is refused outright, before anything is removed.
    #[test]
    fn unrecognized_argument_refuses_and_removes_nothing() {
        let (_dist, script) = generated_uninstall_sh();
        let home = tempfile::tempdir().unwrap();
        let decoy_dir = home.path().join(".local/bin");
        write_fake_binary(&decoy_dir);

        let output = run_uninstall(&script, &["--nonsense"], home.path(), None);

        assert!(
            !output.status.success(),
            "an unrecognized argument must exit non-zero"
        );
        assert!(
            decoy_dir.join("prikk").exists(),
            "nothing should be removed when the script refuses"
        );
    }

    /// `--prefix` as the script's own last argument, with nothing after it to be its value. Before
    /// 0.56.0 step U2 this aborted only via `set -u`'s raw "unbound variable" error when `$2` did
    /// not exist; it must now refuse with this script's own message instead, before anything is
    /// removed.
    #[test]
    fn prefix_with_missing_value_refuses_and_removes_nothing() {
        let (_dist, script) = generated_uninstall_sh();
        let home = tempfile::tempdir().unwrap();
        let decoy_dir = home.path().join(".local/bin");
        write_fake_binary(&decoy_dir);

        let output = run_uninstall(&script, &["--prefix"], home.path(), None);

        assert!(
            !output.status.success(),
            "a missing --prefix value must exit non-zero"
        );
        assert!(
            decoy_dir.join("prikk").exists(),
            "nothing should be removed when the script refuses"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("--prefix needs a directory"),
            "stderr should name the reason, not a raw shell error: {stderr}"
        );
    }

    /// `--prefix ""` -- the actual 0.56.0 step U2 defect. Before the fix this set `install_dir` to
    /// the empty string, which made the script act on "/prikk" (a path nobody named) and still
    /// print "done". It must now refuse the same way the missing-value case does.
    #[test]
    fn prefix_with_empty_value_refuses_and_removes_nothing() {
        let (_dist, script) = generated_uninstall_sh();
        let home = tempfile::tempdir().unwrap();
        let decoy_dir = home.path().join(".local/bin");
        write_fake_binary(&decoy_dir);

        let output = run_uninstall(&script, &["--prefix", ""], home.path(), None);

        assert!(
            !output.status.success(),
            "an empty --prefix value must exit non-zero"
        );
        assert!(
            decoy_dir.join("prikk").exists(),
            "nothing should be removed when the script refuses"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("--prefix needs a directory"),
            "stderr: {stderr}"
        );
    }

    /// `--prefix` and `$PRIKK_INSTALL_DIR` naming two different directories is refused outright,
    /// before anything is removed from either location.
    #[test]
    fn disagreeing_prefix_and_env_var_refuses_and_removes_nothing() {
        let (_dist, script) = generated_uninstall_sh();
        let home = tempfile::tempdir().unwrap();
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        write_fake_binary(dir_a.path());
        write_fake_binary(dir_b.path());

        let output = run_uninstall(
            &script,
            &["--prefix", dir_a.path().to_str().unwrap()],
            home.path(),
            Some(dir_b.path()),
        );

        assert!(
            !output.status.success(),
            "a disagreeing --prefix and PRIKK_INSTALL_DIR must exit non-zero"
        );
        assert!(
            dir_a.path().join("prikk").exists(),
            "dir_a's binary must survive the refusal"
        );
        assert!(
            dir_b.path().join("prikk").exists(),
            "dir_b's binary must survive the refusal"
        );
    }

    /// Unchanged, already-working case: `PRIKK_INSTALL_DIR` alone (no `--prefix`) still removes
    /// from the env-var location.
    #[test]
    fn env_var_alone_still_removes_from_its_location() {
        let (_dist, script) = generated_uninstall_sh();
        let home = tempfile::tempdir().unwrap();
        let custom = tempfile::tempdir().unwrap();
        write_fake_binary(custom.path());

        let output = run_uninstall(&script, &[], home.path(), Some(custom.path()));

        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!custom.path().join("prikk").exists());
    }

    /// Unchanged, already-working case: neither `--prefix` nor `PRIKK_INSTALL_DIR` given still
    /// removes from the default location.
    #[test]
    fn neither_given_removes_from_the_default_location() {
        let (_dist, script) = generated_uninstall_sh();
        let home = tempfile::tempdir().unwrap();
        let decoy_dir = home.path().join(".local/bin");
        write_fake_binary(&decoy_dir);

        let output = run_uninstall(&script, &[], home.path(), None);

        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!decoy_dir.join("prikk").exists());
    }

    /// `--prefix X` with `PRIKK_INSTALL_DIR=X` (equal, not disagreeing) removes from `X` without
    /// refusing -- idempotent agreement is not a refusal.
    #[test]
    fn agreeing_prefix_and_env_var_removes_from_the_shared_location_without_refusing() {
        let (_dist, script) = generated_uninstall_sh();
        let home = tempfile::tempdir().unwrap();
        let shared = tempfile::tempdir().unwrap();
        write_fake_binary(shared.path());

        let output = run_uninstall(
            &script,
            &["--prefix", shared.path().to_str().unwrap()],
            home.path(),
            Some(shared.path()),
        );

        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!shared.path().join("prikk").exists());
    }
}

// 0.56.0 step U: uninstall.ps1 has no param() block and no --prefix flag of its own to disagree
// with anything (like install.ps1, it is env-var-only by design -- the one-line `irm | iex` form
// cannot forward CLI arguments). The fix here is a refusal of any argument at all. No pwsh/
// powershell runtime exists in this environment (confirmed: `which pwsh` finds nothing), and
// `tests.rs` has no execution harness for .ps1 the way the `sh`-based module above does for
// uninstall.sh -- so this can only check the generated *text* contains the refusal, the same
// limited coverage every other `.ps1` test in this file already has. True execution-based
// coverage, parallel to `uninstall_sh_execution` above, could not be added in this sitting; see
// this round's report for why, and for the architect's own independent confirmation this flags.
#[test]
fn uninstall_powershell_refuses_any_argument_in_its_generated_text() {
    let temporary = tempfile::tempdir().unwrap();
    generate(temporary.path()).unwrap();
    let uninstall = std::fs::read_to_string(temporary.path().join("uninstall.ps1")).unwrap();
    assert!(
        uninstall.contains("if ($args.Count -gt 0)"),
        "uninstall.ps1 must refuse any argument"
    );
}

// 0.57.0 items 5b and 5c: install.sh and uninstall.sh carry the licence files, and install followed by uninstall leaves the file
// system as it was. Like the uninstall tests above, these run the real generated scripts with `sh` against scratch directories --
// install.sh against a fixture release served from a `file://` URL (its own `PRIKK_INSTALL_BASE_URL` seam), with a real archive and
// the checksum line it verifies. Unix-only, for the same reason.
#[cfg(unix)]
mod licence_files_execution {
    use super::generate;
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    const FILES: [&str; 2] = ["LICENSE", "THIRD-PARTY-NOTICES"];

    /// The target the script itself would pick on this machine, or `None` where it ships no binary (then the tests have nothing to run).
    fn host_target() -> Option<&'static str> {
        let text = |flag: &str| {
            let output = Command::new("uname").arg(flag).output().unwrap();
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        match (text("-s").as_str(), text("-m").as_str()) {
            ("Linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
            ("Linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
            ("Darwin", "arm64") => Some("aarch64-apple-darwin"),
            _ => None,
        }
    }

    fn curl_exists() -> bool {
        Command::new("curl").arg("--version").output().is_ok()
    }

    /// A fixture release directory: the archive for this host (holding `prikk` and the named licence files) and its checksum line.
    fn fixture_release(target: &str, licence_files: &[&str]) -> tempfile::TempDir {
        let release = tempfile::tempdir().unwrap();
        let stage = tempfile::tempdir().unwrap();
        fs::write(stage.path().join("prikk"), b"#!/bin/sh\necho fake prikk\n").unwrap();
        for name in licence_files {
            fs::write(stage.path().join(name), format!("text of {name}\n")).unwrap();
        }
        let asset = format!("prikk-{target}.tar.gz");
        let mut members = vec!["prikk"];
        members.extend(licence_files.iter().copied());
        let status = Command::new("tar")
            .arg("-C")
            .arg(stage.path())
            .arg("-czf")
            .arg(release.path().join(&asset))
            .args(&members)
            .status()
            .unwrap();
        assert!(status.success());
        let digest = Sha256::digest(fs::read(release.path().join(&asset)).unwrap());
        let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        fs::write(
            release.path().join(format!("{asset}.sha256")),
            format!("{hex}  {asset}\n"),
        )
        .unwrap();
        release
    }

    fn generated(name: &str) -> (tempfile::TempDir, PathBuf) {
        let dist = tempfile::tempdir().unwrap();
        generate(dist.path()).unwrap();
        let path = dist.path().join(name);
        (dist, path)
    }

    /// `prefix` of `None` is the default (`$HOME/.local/bin`): neither the flag nor the variable is given.
    fn run_install(script: &Path, release: &Path, home: &Path, prefix: Option<&Path>) -> Output {
        let mut command = Command::new("sh");
        command.arg(script);
        if let Some(prefix) = prefix {
            command.arg("--prefix").arg(prefix);
        }
        command
            .env("HOME", home)
            .env_remove("SHELL")
            .env_remove("PRIKK_INSTALL_DIR")
            .env(
                "PRIKK_INSTALL_BASE_URL",
                format!("file://{}", release.display()),
            )
            .output()
            .unwrap()
    }

    fn run_uninstall(script: &Path, home: &Path, prefix: Option<&Path>) -> Output {
        let mut command = Command::new("sh");
        command.arg(script);
        if let Some(prefix) = prefix {
            command.arg("--prefix").arg(prefix);
        }
        command
            .env("HOME", home)
            .env_remove("PRIKK_INSTALL_DIR")
            .output()
            .unwrap()
    }

    fn stdout(output: &Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }

    /// Every path under `root` (directories marked), sorted: the file system as a comparison sees it.
    fn tree(root: &Path) -> Vec<String> {
        fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
            let mut entries: Vec<_> = fs::read_dir(dir).unwrap().flatten().collect();
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for entry in entries {
                let path = entry.path();
                let relative = path.strip_prefix(root).unwrap().display().to_string();
                if entry.file_type().unwrap().is_dir() {
                    out.push(format!("{relative}/"));
                    walk(&path, root, out);
                } else {
                    out.push(relative);
                }
            }
        }
        let mut out = Vec::new();
        walk(root, root, &mut out);
        out
    }

    /// **The 5c property**: install, then uninstall, leaves the tree as it was. `build` makes the directories the user already had
    /// under the scratch root `home`, and returns the prefix to use (`None` for the default).
    fn install_then_uninstall_leaves_the_tree(
        build: impl Fn(&Path) -> Option<PathBuf>,
        what: &str,
    ) {
        let Some(target) = host_target().filter(|_| curl_exists()) else {
            return;
        };
        let release = fixture_release(target, &FILES);
        let (_i, install) = generated("install.sh");
        let (_u, uninstall) = generated("uninstall.sh");
        let home = tempfile::tempdir().unwrap();
        let prefix = build(home.path());
        let before = tree(home.path());

        let output = run_install(&install, release.path(), home.path(), prefix.as_deref());
        assert!(output.status.success(), "{what}: {}", stdout(&output));
        let during = tree(home.path());
        assert!(
            during
                .iter()
                .any(|path| path.ends_with("licenses/prikk/LICENSE")),
            "{what}: the licences are installed: {during:?}"
        );
        let output = run_uninstall(&uninstall, home.path(), prefix.as_deref());
        assert!(output.status.success(), "{what}: {}", stdout(&output));
        assert_eq!(
            tree(home.path()),
            before,
            "{what}: install then uninstall leaves the file system as it was"
        );
    }

    #[test]
    fn install_then_uninstall_leaves_the_tree_as_it_was_for_every_prefix_shape() {
        // The default, on an ordinary system: ~/.local/share exists (licenses does not), so the licences go beside the binary directory.
        install_then_uninstall_leaves_the_tree(
            |home| {
                fs::create_dir_all(home.join(".local/bin")).unwrap();
                fs::create_dir_all(home.join(".local/share")).unwrap();
                None
            },
            "default with ~/.local/share",
        );
        // The default where there is no share/: the licences stay inside ~/.local/bin, and ~/.local/share is never made.
        install_then_uninstall_leaves_the_tree(
            |home| {
                fs::create_dir_all(home.join(".local/bin")).unwrap();
                None
            },
            "default without share",
        );
        // A bin prefix whose parent has share/ and already has a licenses/ holding another program's.
        install_then_uninstall_leaves_the_tree(
            |home| {
                fs::create_dir_all(home.join("opt/bin")).unwrap();
                fs::create_dir_all(home.join("opt/share/licenses/other")).unwrap();
                fs::write(home.join("opt/share/licenses/other/LICENSE"), "other").unwrap();
                Some(home.join("opt/bin"))
            },
            "bin prefix, parent has share/licenses",
        );
        // `~/bin`: the parent (the home directory) has no share/, and ~/share must not appear.
        install_then_uninstall_leaves_the_tree(
            |home| {
                fs::create_dir_all(home.join("bin")).unwrap();
                Some(home.join("bin"))
            },
            "~/bin",
        );
        // A prefix that is not named bin.
        install_then_uninstall_leaves_the_tree(
            |home| {
                fs::create_dir_all(home.join("tools")).unwrap();
                Some(home.join("tools"))
            },
            "non-bin prefix",
        );
        // A bin prefix whose parent has share/ but no licenses/: `licenses` is made for the install and removed with it.
        install_then_uninstall_leaves_the_tree(
            |home| {
                fs::create_dir_all(home.join("usr/bin")).unwrap();
                fs::create_dir_all(home.join("usr/share/man")).unwrap();
                Some(home.join("usr/bin"))
            },
            "bin prefix, parent has share without licenses",
        );
    }

    /// Where the licences land, and that a file nobody named, and another program's licences, are left alone.
    #[test]
    fn the_licences_land_where_the_rule_says_and_uninstall_removes_exactly_them() {
        let Some(target) = host_target().filter(|_| curl_exists()) else {
            return;
        };
        let release = fixture_release(target, &FILES);
        let (_i, install) = generated("install.sh");
        let (_u, uninstall) = generated("uninstall.sh");
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        let prefix = root.join("bin");
        fs::create_dir_all(&prefix).unwrap();
        fs::create_dir_all(root.join("share")).unwrap();

        // Beside: <parent>/share exists.
        let output = run_install(&install, release.path(), home.path(), Some(&prefix));
        assert!(output.status.success(), "{}", stdout(&output));
        let licences = root.join("share/licenses/prikk");
        for name in FILES {
            assert_eq!(
                fs::read_to_string(licences.join(name)).unwrap(),
                format!("text of {name}\n")
            );
        }
        assert_eq!(
            fs::read_dir(&prefix).unwrap().count(),
            1,
            "the binary directory holds the binary only"
        );
        fs::write(licences.join("NOTES"), "mine").unwrap();
        let sibling = root.join("share/licenses/other");
        fs::create_dir_all(&sibling).unwrap();
        fs::write(sibling.join("LICENSE"), "other").unwrap();
        let output = run_uninstall(&uninstall, home.path(), Some(&prefix));
        assert!(output.status.success(), "{}", stdout(&output));
        assert!(!prefix.join("prikk").exists());
        for name in FILES {
            assert!(!licences.join(name).exists(), "{name} is removed");
        }
        assert!(
            licences.join("NOTES").is_file(),
            "a file it did not name stays, and keeps its directory"
        );
        assert!(
            sibling.join("LICENSE").is_file(),
            "another program's licences are untouched"
        );
        assert!(root.join("share/licenses").is_dir());

        // Inside: the parent has no share/, so the named directory holds its own.
        let other_root = home.path().join("other-root");
        let other_prefix = other_root.join("bin");
        fs::create_dir_all(&other_prefix).unwrap();
        let output = run_install(&install, release.path(), home.path(), Some(&other_prefix));
        assert!(output.status.success(), "{}", stdout(&output));
        for name in FILES {
            assert!(
                other_prefix
                    .join("share/licenses/prikk")
                    .join(name)
                    .is_file()
            );
        }
        assert!(
            !other_root.join("share").exists(),
            "nothing is written outside the named directory"
        );
    }

    /// An archive from before 0.57.0 carries neither file: the install still succeeds and says so.
    #[test]
    fn an_archive_without_the_licence_files_installs_the_binary_and_says_so() {
        let Some(target) = host_target().filter(|_| curl_exists()) else {
            return;
        };
        let release = fixture_release(target, &[]);
        let (_i, install) = generated("install.sh");
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        let prefix = root.join("bin");
        fs::create_dir_all(&prefix).unwrap();
        let before = tree(home.path());
        let output = run_install(&install, release.path(), home.path(), Some(&prefix));
        assert!(output.status.success(), "{}", stdout(&output));
        assert!(prefix.join("prikk").is_file());
        let after = tree(home.path());
        assert_eq!(
            after.len(),
            before.len() + 1,
            "only the binary is added: {after:?}"
        );
        assert!(
            stdout(&output).contains("this archive has no LICENSE"),
            "{}",
            stdout(&output)
        );
    }

    /// Controls: the tree-equality property fails on scripts with the cleanup taken out, and the placement on scripts without it.
    #[test]
    fn the_controls_fail_on_scripts_without_the_licence_steps() {
        let Some(target) = host_target().filter(|_| curl_exists()) else {
            return;
        };
        let release = fixture_release(target, &FILES);
        let home = tempfile::tempdir().unwrap();
        let anchor = "for licence_file in LICENSE THIRD-PARTY-NOTICES; do";

        // install.sh without the licence loop installs none.
        let (dir, install) = generated("install.sh");
        let text = fs::read_to_string(&install).unwrap();
        assert!(text.contains(anchor));
        let perturbed = dir.path().join("install-perturbed.sh");
        fs::write(&perturbed, text.replace(anchor, "for licence_file in; do")).unwrap();
        let prefix = home.path().join("root/bin");
        fs::create_dir_all(&prefix).unwrap();
        let output = run_install(&perturbed, release.path(), home.path(), Some(&prefix));
        assert!(output.status.success(), "{}", stdout(&output));
        assert!(
            !prefix.join("share").exists() && !home.path().join("root/share").exists(),
            "RED: the perturbed install.sh installs no licence file"
        );

        // uninstall.sh without the removal leaves them.
        let (_i, install) = generated("install.sh");
        let output = run_install(&install, release.path(), home.path(), Some(&prefix));
        assert!(output.status.success(), "{}", stdout(&output));
        let (udir, uninstall) = generated("uninstall.sh");
        let text = fs::read_to_string(&uninstall).unwrap();
        assert!(text.contains(anchor));
        let perturbed = udir.path().join("uninstall-perturbed.sh");
        fs::write(&perturbed, text.replace(anchor, "for licence_file in; do")).unwrap();
        let output = run_uninstall(&perturbed, home.path(), Some(&prefix));
        assert!(output.status.success(), "{}", stdout(&output));
        assert!(
            prefix
                .join("share/licenses/prikk/THIRD-PARTY-NOTICES")
                .is_file(),
            "RED: the perturbed uninstall.sh leaves the licence files"
        );
    }
}

/// 0.57.0 item 5b: the Windows installer test (run in CI on a Windows runner, never here) perturbs `uninstall.ps1` by replacing one
/// exact line, and fails with "the template changed shape" at run time if it is gone. This holds both ends together on every OS: the
/// generated script has the line, and the test script names the same text, so a rename in either shows here first.
#[test]
fn the_windows_test_names_the_licence_loop_the_generated_uninstall_script_has() {
    let temporary = tempfile::tempdir().unwrap();
    generate(temporary.path()).unwrap();
    let anchor = r#"foreach ($licenceName in @("LICENSE", "THIRD-PARTY-NOTICES")) {"#;
    for script in ["install.ps1", "uninstall.ps1"] {
        let text = std::fs::read_to_string(temporary.path().join(script)).unwrap();
        assert!(text.contains(anchor), "{script} has the licence loop");
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap();
    let windows_test =
        std::fs::read_to_string(root.join("scripts/windows-installer-test.ps1")).unwrap();
    assert!(
        windows_test.contains(anchor),
        "scripts/windows-installer-test.ps1 perturbs the same line"
    );
}

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

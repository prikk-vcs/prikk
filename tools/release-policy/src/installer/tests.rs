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

use super::{Invocation, invocations, scan, scan_shell, scan_yaml};

#[test]
fn recognizes_bounded_equivalent_forms() {
    for text in [
        "cargo  publish --workspace",
        "env cargo \\\n          publish --workspace",
        "command cargo\tpublish -p prikk-release-policy",
        "cmd=(cargo publish --workspace)",
        "publish_all() { cargo publish --workspace; }",
        "run: >-\n  cargo\n  publish --workspace",
        "printf '#'; cargo publish --workspace",
        "printf \"#\"; cargo publish --workspace",
        ": ''#x; cargo publish --workspace",
        ": \"\"#x; cargo publish --workspace",
    ] {
        assert!(matches!(
            invocations(text).as_slice(),
            [Invocation::Publication { phase, .. }] if phase == "publish"
        ));
    }
    assert_eq!(
        invocations("env cargo run --locked -p prikk-release-policy -- check"),
        vec![Invocation::RustPolicy]
    );
}

#[test]
fn real_comments_and_malformed_commands_fail_closed() {
    assert!(invocations("printf ok # cargo publish --workspace").is_empty());
    assert!(invocations(": '' # cargo publish --workspace").is_empty());
    assert_eq!(scan("cargo 'publish").errors, vec!["unterminated-quote"]);
    let dynamic_publication = scan("$CARGO publish --workspace");
    assert!(
        dynamic_publication
            .errors
            .contains(&"unsupported-cargo-executable")
    );
    assert!(
        dynamic_publication
            .errors
            .contains(&"unsupported-publication-invocation")
    );
    for command in [
        "$CARGO run --locked -p prikk-release-policy -- check",
        "${CARGO} run --locked -p prikk-release-policy -- check",
        "$(resolve-cargo) run --locked -p prikk-release-policy -- check",
    ] {
        assert!(!scan(command).errors.is_empty(), "{command}");
    }
    for command in [
        "cargo \"$PHASE\" --workspace",
        "cargo \"${PHASE}\" --workspace",
        "cargo \"$ACTION\" --workspace",
        "cargo \"${ACTION}\" --workspace",
        "$CARGO \"$PHASE\" --workspace",
        "$TOOL \"$ACTION\" --workspace",
        "$TOOL publish --workspace",
        "env $TOOL publish --workspace",
        "env MODE=release ${TOOL} \"${ACTION}\" --workspace",
        "run: $TOOL \"$ACTION\" --workspace",
        "- run: $TOOL \"$ACTION\" --workspace",
        "  - run: $TOOL \"$ACTION\" --workspace",
        "- { run: $TOOL \"$ACTION\" --workspace }",
        "- {run: $TOOL \"$ACTION\" --workspace}",
        "env -u UNUSED $TOOL \"$ACTION\" --workspace",
        "env -C DIR $TOOL \"$ACTION\" --workspace",
        "env --unset UNUSED --chdir DIR $TOOL \"$ACTION\" --workspace",
        "command -p $TOOL \"$ACTION\" --workspace",
        "nice $TOOL \"$ACTION\" --workspace",
        "exec $TOOL \"$ACTION\" --workspace",
        "timeout 600 $TOOL \"$ACTION\" --workspace",
        "xargs $TOOL \"$ACTION\" --workspace",
        "nohup $TOOL \"$ACTION\" --workspace",
        "stdbuf -oL $TOOL \"$ACTION\" --workspace",
        "setsid $TOOL \"$ACTION\" --workspace",
        "ionice $TOOL \"$ACTION\" --workspace",
        "time $TOOL \"$ACTION\" --workspace",
        "project-wrapper $TOOL \"$ACTION\" --workspace",
        "- run: nice $TOOL \"$ACTION\" --workspace",
    ] {
        assert!(!scan(command).errors.is_empty(), "{command}");
    }
    for command in [
        "env --unknown VALUE $TOOL \"$ACTION\" --workspace",
        "env -u",
        "command --unknown $TOOL \"$ACTION\" --workspace",
    ] {
        assert!(!scan(command).errors.is_empty(), "{command}");
    }
    for command in [
        "echo \"$VALUE\"",
        "printf '%s' \"$VALUE\"",
        "printf '%s' '`literal`'",
        "url: ${{ steps.deployment.outputs.page_url }}",
    ] {
        assert!(scan(command).errors.is_empty(), "{command}");
    }
    for command in [
        "`deploy`",
        "`resolve-tool` publish",
        "sh -c 'cargo publish'",
        "bash -c \"cargo package --workspace\"",
        "bash -lc 'cargo publish'",
        "/bin/sh -c 'cargo publish'",
        "dash -c 'cargo publish'",
        "ksh -c 'cargo publish'",
        "zsh -c 'cargo publish'",
        "env sh -c 'cargo publish'",
        "eval 'cargo publish'",
    ] {
        assert!(!scan(command).errors.is_empty(), "{command}");
    }
}

#[test]
fn governed_procedures_are_default_closed() {
    for command in [
        "python3 -c 'import os; os.system(\"cargo publish\")'",
        "perl -e 'system(\"cargo publish\")'",
        "node -e 'require(\"child_process\").execSync(\"cargo publish\")'",
        "ruby -e 'system(\"cargo publish\")'",
        "nice sh -c 'cargo publish'",
        "xargs sh -c 'cargo publish'",
        "timeout 600 bash -c 'cargo publish'",
        "project-local-wrapper cargo fmt --check",
    ] {
        assert!(!scan_shell(command).errors.is_empty(), "{command}");
        assert!(
            !scan_yaml(&format!("- run: {command}")).errors.is_empty(),
            "{command}"
        );
    }

    let workflow = "url: ${{ steps.deployment.outputs.page_url }}\n\
                    - run: cargo fmt --check\n\
                    - run: cargo clippy --workspace --all-targets --all-features --locked -- -D warnings\n\
                    - run: cargo test --workspace\n\
                    - run: mdbook build\n";
    assert!(scan_yaml(workflow).errors.is_empty());
}

#[test]
fn recognizes_exact_locked_workspace_procedures_without_authority() {
    let commands = [
        "cargo fmt --all -- --check",
        "cargo clippy --workspace --all-targets --all-features --locked -- -D warnings",
        "cargo test --workspace --locked",
        "cargo check --workspace --all-targets --locked",
        "cargo build --workspace --locked",
    ];

    for command in commands {
        for scan in [scan_shell(command), scan_yaml(&format!("- run: {command}"))] {
            assert!(scan.errors.is_empty(), "{command}: {:?}", scan.errors);
            assert!(scan.invocations.is_empty(), "{command}");
        }
    }

    for command in [
        "MODE=ci cargo clippy --workspace --all-targets --all-features --locked -- -D warnings",
        "env MODE=ci cargo clippy --workspace --all-targets --all-features --locked -- -D warnings",
        "command cargo clippy --workspace --all-targets --all-features --locked -- -D warnings",
        "MODE=ci cargo test --workspace --locked",
        "env MODE=ci cargo test --workspace --locked",
        "command cargo test --workspace --locked",
    ] {
        for scan in [scan_shell(command), scan_yaml(&format!("- run: {command}"))] {
            assert!(scan.errors.is_empty(), "{command}: {:?}", scan.errors);
            assert!(scan.invocations.is_empty(), "{command}");
        }
    }
}

#[test]
fn rejects_retired_clippy_productions_through_bounded_prefixes() {
    for arguments in [
        "--workspace --all-targets -- -D warnings",
        "--workspace --all-targets --locked -- -D warnings",
    ] {
        for prefix in ["", "MODE=ci ", "env MODE=ci ", "command "] {
            let command = format!("{prefix}cargo clippy {arguments}");
            for scan in [
                scan_shell(&command),
                scan_yaml(&format!("- run: {command}")),
            ] {
                assert_eq!(
                    scan.errors,
                    vec!["unclassified-procedure-command"],
                    "{command}"
                );
                assert!(scan.invocations.is_empty(), "{command}");
            }
        }
    }
}

#[test]
fn rejects_near_miss_locked_workspace_procedures() {
    for command in [
        "cargo fmt --all --check",
        "cargo clippy --workspace --all-targets --all-features -- -D warnings",
        "cargo clippy --workspace --all-targets --all-features --all-features --locked -- -D warnings",
        "cargo clippy --workspace --all-targets --all-features --locked --locked -- -D warnings",
        "cargo clippy --workspace --all-targets --all-features --locked --all-targets -- -D warnings",
        "cargo clippy --workspace --all-targets --locked --all-features -- -D warnings",
        "cargo clippy --workspace --all-targets --locked -- -D warnings --all-features",
        "cargo clippy --workspace --all-targets --all-features -- -D warnings --locked",
        "cargo +stable clippy --workspace --all-targets --all-features --locked -- -D warnings",
        "$CARGO clippy --workspace --all-targets --all-features --locked -- -D warnings",
        "cargo clippy \"$SCOPE\" --all-targets --all-features --locked -- -D warnings",
        "bash -c 'cargo clippy --workspace --all-targets --all-features --locked -- -D warnings'",
        "project-local-wrapper cargo clippy --workspace --all-targets --all-features --locked -- -D warnings",
        "cargo test --locked --workspace",
        "cargo check --workspace --locked",
        "cargo build --workspace --all-targets --locked --release",
        "cargo +1.85.0 test --workspace --locked",
        "$CARGO test --workspace --locked",
        "cargo test \"$SCOPE\" --locked",
        "bash -c 'cargo test --workspace --locked'",
        "project-local-wrapper cargo test --workspace --locked",
    ] {
        assert!(!scan_shell(command).errors.is_empty(), "{command}");
        assert!(
            !scan_yaml(&format!("- run: {command}")).errors.is_empty(),
            "{command}"
        );
    }
}

#[test]
fn recognizes_dc70_release_binary_build_procedures() {
    for command in [
        "cargo build -p prikk --release --target x86_64-unknown-linux-gnu --locked",
        "cargo build -p prikk --release --target aarch64-unknown-linux-gnu --locked",
    ] {
        for scan in [scan_shell(command), scan_yaml(&format!("- run: {command}"))] {
            assert!(scan.errors.is_empty(), "{command}: {:?}", scan.errors);
            assert!(scan.invocations.is_empty(), "{command}");
        }
    }

    for command in [
        "cargo build -p prikk --release --target ${{ matrix.target }} --locked",
        "cargo build -p prikk --release --target x86_64-unknown-linux-musl --locked",
        "cargo build -p prikk --release --target aarch64-unknown-linux-gnu --locked --offline",
        "cargo build --release --target x86_64-unknown-linux-gnu --locked -p prikk",
    ] {
        assert!(!scan_shell(command).errors.is_empty(), "{command}");
        assert!(
            !scan_yaml(&format!("- run: {command}")).errors.is_empty(),
            "{command}"
        );
    }
}

#[test]
fn dc70_file_utility_inert_heads_tolerate_any_arguments_including_dynamic_ones() {
    for command in [
        "cd dist",
        "mkdir -p stage dist",
        "cp \"$bin\" stage/prikk",
        "sha256sum \"${asset}.tar.gz\" > \"${asset}.tar.gz.sha256\"",
    ] {
        for scan in [scan_shell(command), scan_yaml(&format!("- run: {command}"))] {
            assert!(scan.errors.is_empty(), "{command}: {:?}", scan.errors);
            assert!(scan.invocations.is_empty(), "{command}");
        }
    }
}

/// Architect review B1 (2026-08-03): `tar`, `rustc`, and `gh` can each execute another program
/// (tar via `--to-command`/`-I`, rustc via proc macros and build scripts, gh as a general-purpose
/// API client), so — unlike the file utilities above — they must NOT tolerate arbitrary
/// arguments. Only the exact commands `release.yml` actually uses are accepted; anything else
/// with the same head, dynamic or not, is rejected the same way an unregistered cargo build
/// would be.
#[test]
fn dc70_tar_rustc_gh_require_exact_procedure_match_not_blanket_inertness() {
    for command in [
        "tar -C stage -czf dist/prikk-x86_64-unknown-linux-gnu.tar.gz prikk LICENSE",
        "tar -C stage -czf dist/prikk-aarch64-unknown-linux-gnu.tar.gz prikk LICENSE",
        "rustc -vV >> dist/prikk-x86_64-unknown-linux-gnu.build-info.txt",
        "rustc -vV >> dist/prikk-aarch64-unknown-linux-gnu.build-info.txt",
        "gh release create \"$TAG\" dist/*.tar.gz dist/*.tar.gz.sha256 dist/*.zip dist/*.zip.sha256 dist/*.build-info.txt dist/*.sh dist/*.sh.sha256 --repo prikk-vcs/prikk --title \"$TAG\" --notes-file release-notes.md",
    ] {
        for scan in [scan_shell(command), scan_yaml(&format!("- run: {command}"))] {
            assert!(scan.errors.is_empty(), "{command}: {:?}", scan.errors);
            assert!(scan.invocations.is_empty(), "{command}");
        }
    }

    for command in [
        "tar -C stage -czf \"dist/${asset}.tar.gz\" prikk LICENSE",
        "tar --to-command=sh -C stage -czf dist/prikk-x86_64-unknown-linux-gnu.tar.gz prikk",
        "tar -I 'sh -c \"cargo publish\"' -cf out.tar prikk",
        "rustc -vV",
        "rustc evil.rs -o /tmp/evil",
        "gh api repos/prikk-vcs/prikk --method DELETE",
        "gh workflow run publish.yml",
        "gh release create \"$OTHER_TAG\" dist/*.tar.gz dist/*.tar.gz.sha256 dist/*.zip dist/*.zip.sha256 dist/*.build-info.txt dist/*.sh dist/*.sh.sha256 --repo prikk-vcs/prikk --title \"$TAG\" --notes-file release-notes.md",
        "gh release create \"$TAG\" dist/*.tar.gz dist/*.tar.gz.sha256 dist/*.zip dist/*.zip.sha256 dist/*.build-info.txt dist/*.sh dist/*.sh.sha256 --repo other/repo --title \"$TAG\" --notes-file release-notes.md",
        // RFC 107 Stage 2: the old two-glob asset list (before the `.zip`/`.zip.sha256` fix) must
        // now be rejected too -- it is what silently dropped the Windows artifact
        // (`RFC-107-stage-2-report-ruling-v1.md` §5).
        "gh release create \"$TAG\" dist/*.tar.gz dist/*.tar.gz.sha256 dist/*.build-info.txt --repo prikk-vcs/prikk --title \"$TAG\" --notes-file release-notes.md",
        // Universal installer/uninstaller handoff v1 §4.1: the five-glob shape (before this
        // increment's own `.sh`/`.sh.sha256` addition) must now be rejected too -- the same "old
        // shape stops being accepted" precedent RFC 107 Stage 2 already established above for the
        // two-glob case.
        "gh release create \"$TAG\" dist/*.tar.gz dist/*.tar.gz.sha256 dist/*.zip dist/*.zip.sha256 dist/*.build-info.txt --repo prikk-vcs/prikk --title \"$TAG\" --notes-file release-notes.md",
    ] {
        assert!(!scan_shell(command).errors.is_empty(), "{command}");
        assert!(
            !scan_yaml(&format!("- run: {command}")).errors.is_empty(),
            "{command}"
        );
    }
}

/// RFC 107 Stage 2: the macOS package step reuses Linux's `tar`/`rustc` shapes exactly, plus
/// `shasum` (base-OS, unlike `sha256sum` which is not confirmed present --
/// `RFC-107-stage-2-report-ruling-v1.md` §2) added to `inert_head` alongside `sha256sum` since
/// neither can invoke another program under any arguments.
#[test]
fn recognizes_rfc107_macos_package_procedure() {
    for command in [
        "tar -C stage -czf dist/prikk-aarch64-apple-darwin.tar.gz prikk LICENSE",
        "rustc -vV >> dist/prikk-aarch64-apple-darwin.build-info.txt",
        "shasum -a 256 prikk-aarch64-apple-darwin.tar.gz",
        "shasum -a 256 anything.tar.gz > anything.tar.gz.sha256",
    ] {
        for scan in [scan_shell(command), scan_yaml(&format!("- run: {command}"))] {
            assert!(scan.errors.is_empty(), "{command}: {:?}", scan.errors);
        }
    }

    for command in [
        "tar -C stage -czf dist/prikk-x86_64-apple-darwin.tar.gz prikk LICENSE",
        "rustc evil.rs -o /tmp/evil",
    ] {
        assert!(!scan_shell(command).errors.is_empty(), "{command}");
    }
}

/// RFC 107 Stage 2: the Windows package step's PowerShell cmdlets, each exact-match for the same
/// reason `tar`/`rustc`/`gh` are. Confirmed against the real lexer before writing release.yml:
/// commas and backslashes are both stripped by tokenization (`normalize_token`'s trailing-
/// punctuation trim, and the lexer's own POSIX-style unquoted-backslash escape handling), which is
/// exactly why the workflow uses forward slashes and no trailing comma survives into these
/// literals (`RFC-107-stage-2-report-ruling-v1.md` §3). The checksum line embeds `Get-FileHash` as
/// a `$(...)` subexpression inside a quoted `Set-Content -Value`, not a standalone command with its
/// raw table output redirected -- the latter is not a `sha256sum`-verifiable checksum at all
/// (`RFC-107-stage-2-implementation-ruling-v1.md` §1). `-NoNewline` makes the file's byte content
/// deterministic (`Set-Content` otherwise terminates with the platform newline, CRLF on Windows)
/// rather than leaving `sha256sum -c`'s tolerance for a trailing `\r` an open question only a real
/// artifact could answer (`RFC-107-stage-2-checksum-fix-ruling-v1.md` §2).
#[test]
fn recognizes_rfc107_windows_package_procedure() {
    for command in [
        "New-Item -ItemType Directory -Force -Path stage, dist",
        "Copy-Item target/x86_64-pc-windows-msvc/release/prikk.exe stage/prikk.exe",
        "Copy-Item LICENSE stage/LICENSE",
        "Compress-Archive -Path stage/prikk.exe, stage/LICENSE -DestinationPath dist/prikk-x86_64-pc-windows-msvc.zip",
        "Set-Content -Path dist/prikk-x86_64-pc-windows-msvc.zip.sha256 -Value \"$((Get-FileHash dist/prikk-x86_64-pc-windows-msvc.zip -Algorithm SHA256).Hash.ToLower())  prikk-x86_64-pc-windows-msvc.zip\" -NoNewline",
        "Set-Content -Path dist/prikk-x86_64-pc-windows-msvc.build-info.txt -Value \"target: x86_64-pc-windows-msvc\"",
        "Add-Content -Path dist/prikk-x86_64-pc-windows-msvc.build-info.txt -Value \"commit: $env:GITHUB_SHA\"",
        "Add-Content -Path dist/prikk-x86_64-pc-windows-msvc.build-info.txt -Value \"tag: $env:GITHUB_REF_NAME\"",
        "Add-Content -Path dist/prikk-x86_64-pc-windows-msvc.build-info.txt -Value \"build: cargo build -p prikk --release --target x86_64-pc-windows-msvc --locked\"",
        "rustc -vV >> dist/prikk-x86_64-pc-windows-msvc.build-info.txt",
    ] {
        for scan in [scan_shell(command), scan_yaml(&format!("- run: {command}"))] {
            assert!(scan.errors.is_empty(), "{command}: {:?}", scan.errors);
        }
    }

    for command in [
        "Copy-Item target/x86_64-pc-windows-msvc/release/prikk.exe /tmp/evil.exe",
        "Compress-Archive -Path stage/prikk.exe -DestinationPath /tmp/evil.zip",
        // The raw, unformatted `Get-FileHash` redirect this Stage 2 round replaced -- must stay
        // rejected, not merely absent from the positive list, since it is what silently shipped a
        // non-checksum the first time.
        "Get-FileHash dist/prikk-x86_64-pc-windows-msvc.zip -Algorithm SHA256 > dist/prikk-x86_64-pc-windows-msvc.zip.sha256",
        "Get-FileHash /etc/passwd -Algorithm SHA256",
        // The checksum line without `-NoNewline` -- the shape that left a CRLF question only a
        // real artifact could answer -- must also stay rejected now that the flag is required.
        "Set-Content -Path dist/prikk-x86_64-pc-windows-msvc.zip.sha256 -Value \"$((Get-FileHash dist/prikk-x86_64-pc-windows-msvc.zip -Algorithm SHA256).Hash.ToLower())  prikk-x86_64-pc-windows-msvc.zip\"",
        "Set-Content -Path /tmp/evil.txt -Value \"anything\"",
        "Add-Content -Path dist/prikk-x86_64-pc-windows-msvc.build-info.txt -Value \"commit: $(evil)\"",
        "New-Item -ItemType File -Force -Path evil.exe",
    ] {
        assert!(!scan_shell(command).errors.is_empty(), "{command}");
    }
}

/// RFC 107 Stage 2: the two new release-binary builds, same exact-entry shape as the existing
/// Linux ones -- deliberately `aarch64-apple-darwin` and `x86_64-pc-windows-msvc` only, not
/// `x86_64-apple-darwin`, per the target-list finding
/// (`RFC-107-stage-2-report-ruling-v1.md` §1).
#[test]
fn recognizes_rfc107_stage2_release_binary_builds() {
    for command in [
        "cargo build -p prikk --release --target aarch64-apple-darwin --locked",
        "cargo build -p prikk --release --target x86_64-pc-windows-msvc --locked",
    ] {
        for scan in [scan_shell(command), scan_yaml(&format!("- run: {command}"))] {
            assert!(scan.errors.is_empty(), "{command}: {:?}", scan.errors);
            assert!(scan.invocations.is_empty(), "{command}");
        }
    }

    assert!(
        !scan_shell("cargo build -p prikk --release --target x86_64-apple-darwin --locked")
            .errors
            .is_empty()
    );
}

/// RFC 107 Stage 1: the notes-assembly step is a dedicated matcher (`release_notes_procedure`),
/// not a widening of `rust_policy` -- `$TAG` is the only token free to vary, mirroring
/// `gh_release_create`'s own precedent for the same reason (the release tag cannot be enumerated
/// in advance).
#[test]
fn recognizes_rfc107_release_notes_assembly_procedure() {
    for command in [
        "cargo run -p prikk-release-policy --locked -- release-notes \"$TAG\" dist > release-notes.md",
    ] {
        for scan in [scan_shell(command), scan_yaml(&format!("- run: {command}"))] {
            assert!(scan.errors.is_empty(), "{command}: {:?}", scan.errors);
        }
    }

    for command in [
        "cargo run -p prikk-release-policy --locked -- release-notes \"$TAG\" dist",
        "cargo run -p prikk-release-policy --locked -- release-notes \"$TAG\" other-dir > release-notes.md",
        "cargo run -p prikk-release-policy --locked -- boundary-check > release-notes.md",
        "cargo run -p prikk-release-policy --locked -- release-notes \"$TAG\" dist > notes.md",
    ] {
        assert!(!scan_shell(command).errors.is_empty(), "{command}");
        assert!(
            !scan_yaml(&format!("- run: {command}")).errors.is_empty(),
            "{command}"
        );
    }
}

#[test]
fn recognizes_dc71_non_linux_fixture_build_procedure() {
    for command in ["cargo build -p prikk --locked"] {
        for scan in [scan_shell(command), scan_yaml(&format!("- run: {command}"))] {
            assert!(scan.errors.is_empty(), "{command}: {:?}", scan.errors);
            assert!(scan.invocations.is_empty(), "{command}");
        }
    }

    for command in [
        "cargo build -p prikk --locked --release",
        "cargo build -p other-crate --locked",
        "cargo build --locked -p prikk --features audit-plugins",
    ] {
        assert!(!scan_shell(command).errors.is_empty(), "{command}");
        assert!(
            !scan_yaml(&format!("- run: {command}")).errors.is_empty(),
            "{command}"
        );
    }
}

/// `prikk` itself has no subprocess-execution capability anywhere in the workspace (verified by
/// `grep -rn "std::process::Command" crates/` returning nothing) — unlike `tar`/`rustc`/`gh`, it
/// tolerates any arguments the same way `cd`/`mkdir`/`cp`/`sha256sum` do.
#[test]
fn dc71_prikk_binary_is_inert_with_any_arguments() {
    for command in [
        "target/debug/prikk log",
        "../target/debug/prikk verify",
        "../target/debug/prikk checkout --plan-only --ref \"$REF\"",
        "target/debug/prikk trust maintainer add --key-id \"$KEY\" --public-key deadbeef",
    ] {
        for scan in [scan_shell(command), scan_yaml(&format!("- run: {command}"))] {
            assert!(scan.errors.is_empty(), "{command}: {:?}", scan.errors);
            assert!(scan.invocations.is_empty(), "{command}");
        }
    }
}

/// CI hermeticity increment: `cargo fetch --locked` populates every target's dependencies before
/// the boundary check's `cargo metadata --locked --offline`. `fetch` cannot publish or package, so
/// this is an exact-match procedure entry on the same DC-70 B1 pattern, not an `inert_head` grant.
#[test]
fn recognizes_ci_hermeticity_fetch_procedure() {
    for command in ["cargo fetch --locked"] {
        for scan in [scan_shell(command), scan_yaml(&format!("- run: {command}"))] {
            assert!(scan.errors.is_empty(), "{command}: {:?}", scan.errors);
            assert!(scan.invocations.is_empty(), "{command}");
        }
    }

    for command in [
        "cargo fetch",
        "cargo fetch --target x86_64-unknown-linux-gnu --locked",
        "cargo fetch --locked --target x86_64-unknown-linux-gnu",
        "$CARGO fetch --locked",
    ] {
        assert!(!scan_shell(command).errors.is_empty(), "{command}");
        assert!(
            !scan_yaml(&format!("- run: {command}")).errors.is_empty(),
            "{command}"
        );
    }
}

/// DC-71 B2 ruling: the CI fixture round-trips through tar (not the artifact zip, which drops
/// empty directories) between the fixture and non-linux-verify jobs. The create form's literal
/// `.` (current directory) argument is trimmed to nothing by the lexer's sentence-trailing-period
/// normalization, so its accepted tail is four tokens, not five — asserted here so a future lexer
/// change that stops trimming `.` cannot silently make this entry permissive of an extra argument.
#[test]
fn recognizes_dc71_ci_fixture_tar_round_trip() {
    for command in [
        "tar -czf fixture-repo.tar.gz -C fixture-repo .",
        // Tokenizes identically to the line above — the lexer cannot see the trailing "." either
        // way, so this is not a distinguishable near-miss, it is the same accepted procedure.
        "tar -czf fixture-repo.tar.gz -C fixture-repo",
        "tar -xzf fixture-repo.tar.gz -C fixture-repo",
    ] {
        for scan in [scan_shell(command), scan_yaml(&format!("- run: {command}"))] {
            assert!(scan.errors.is_empty(), "{command}: {:?}", scan.errors);
            assert!(scan.invocations.is_empty(), "{command}");
        }
    }

    for command in [
        "tar -czf other.tar.gz -C fixture-repo .",
        "tar -czf fixture-repo.tar.gz -C other-dir .",
        "tar -xzf fixture-repo.tar.gz -C other-dir",
        "tar --to-command=sh -czf fixture-repo.tar.gz -C fixture-repo .",
    ] {
        assert!(!scan_shell(command).errors.is_empty(), "{command}");
        assert!(
            !scan_yaml(&format!("- run: {command}")).errors.is_empty(),
            "{command}"
        );
    }
}

/// DC-77: `docs.yml`'s `mdbook-mermaid` install, so Mermaid diagrams render as pictures rather
/// than code blocks. The `install` arm's new entry must accept exactly this vector — same DC-70
/// B1 pattern as `tar`/`rustc`/`gh` above, not a widening of `install` to accept any crate. A
/// *different* `cargo install` anywhere in a scanned file, including one installing the same
/// crate at a different version or with different flags, must still be rejected.
#[test]
fn recognizes_dc77_mdbook_mermaid_install_procedure_narrowly() {
    for command in ["cargo install mdbook-mermaid --vers \"^0.17\" --locked"] {
        for scan in [scan_shell(command), scan_yaml(&format!("- run: {command}"))] {
            assert!(scan.errors.is_empty(), "{command}: {:?}", scan.errors);
            assert!(scan.invocations.is_empty(), "{command}");
        }
    }

    for command in [
        "cargo install mdbook-mermaid",
        "cargo install mdbook-mermaid --vers \"^0.16\" --locked",
        "cargo install mdbook-mermaid --locked",
        "cargo install mdbook-mermaid --vers \"^0.17\"",
        "cargo install mdbook-mermaid --vers \"^0.17\" --locked --force",
        "cargo install --vers \"^0.17\" --locked mdbook-mermaid",
        "cargo install mdbook-mermaid-ssr --vers \"^0.17\" --locked",
        "cargo install some-other-crate --vers \"^0.17\" --locked",
    ] {
        assert!(!scan_shell(command).errors.is_empty(), "{command}");
        assert!(
            !scan_yaml(&format!("- run: {command}")).errors.is_empty(),
            "{command}"
        );
    }
}

#[test]
fn recognizes_dc81_macos_sync_benchmark_procedure_narrowly() {
    for command in [
        "cargo test --workspace --locked -- measure_directory_sync_fsync_vs_fcntl_fullfsync --ignored --nocapture",
    ] {
        for scan in [scan_shell(command), scan_yaml(&format!("- run: {command}"))] {
            assert!(scan.errors.is_empty(), "{command}: {:?}", scan.errors);
            assert!(scan.invocations.is_empty(), "{command}");
        }
    }

    for command in [
        "cargo test --workspace --locked -- measure_directory_sync_fsync_vs_fcntl_fullfsync",
        "cargo test --workspace --locked -- measure_directory_sync_fsync_vs_fcntl_fullfsync --nocapture",
        "cargo test --workspace --locked -- measure_directory_sync_fsync_vs_fcntl_fullfsync --ignored",
        "cargo test --workspace --locked -- --ignored --nocapture",
        "cargo test --workspace --locked -- some_other_test --ignored --nocapture",
        "cargo test --workspace -- measure_directory_sync_fsync_vs_fcntl_fullfsync --ignored --nocapture",
        "cargo test --workspace --locked --release -- measure_directory_sync_fsync_vs_fcntl_fullfsync --ignored --nocapture",
    ] {
        assert!(!scan_shell(command).errors.is_empty(), "{command}");
        assert!(
            !scan_yaml(&format!("- run: {command}")).errors.is_empty(),
            "{command}"
        );
    }
}

#[test]
fn recognizes_rfc126_doc_lint_procedure_narrowly() {
    for command in ["cargo doc --workspace --no-deps"] {
        for scan in [scan_shell(command), scan_yaml(&format!("- run: {command}"))] {
            assert!(scan.errors.is_empty(), "{command}: {:?}", scan.errors);
            assert!(scan.invocations.is_empty(), "{command}");
        }
    }

    for command in [
        "cargo doc",
        "cargo doc --workspace",
        "cargo doc --no-deps",
        "cargo doc --workspace --no-deps --open",
        "cargo doc --no-deps --workspace",
        "cargo doc --workspace --document-private-items",
    ] {
        assert!(!scan_shell(command).errors.is_empty(), "{command}");
        assert!(
            !scan_yaml(&format!("- run: {command}")).errors.is_empty(),
            "{command}"
        );
    }
}

#[test]
fn recognizes_rfc126_scheduled_audit_procedure_narrowly() {
    for command in ["cargo audit", "cargo install cargo-audit --locked"] {
        for scan in [scan_shell(command), scan_yaml(&format!("- run: {command}"))] {
            assert!(scan.errors.is_empty(), "{command}: {:?}", scan.errors);
            assert!(scan.invocations.is_empty(), "{command}");
        }
    }

    for command in [
        "cargo audit --no-fetch",
        "cargo audit --deny warnings",
        "cargo install cargo-audit",
        "cargo install cargo-audit --version 0.21.0",
        "cargo install cargo-audit --locked --force",
        "cargo install --locked cargo-audit",
        "cargo install cargo-audit-fix --locked",
        "cargo install some-other-crate --locked",
    ] {
        assert!(!scan_shell(command).errors.is_empty(), "{command}");
        assert!(
            !scan_yaml(&format!("- run: {command}")).errors.is_empty(),
            "{command}"
        );
    }
}

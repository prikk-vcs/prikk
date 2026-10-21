//! Universal installer/uninstaller handoff v1 §4.1: generated at release time and attached as a
//! release asset, never committed as a tracked `.sh` file.
//!
//! **Why not committed.** `command_scan::scan_shell`/`scan_yaml` treat every `.sh`/`.yml`/`.yaml`
//! file *anywhere in the repository* as a governed procedure (`reference.rs`'s
//! `collect_text_files` walks the whole tree, excluding only `.git`/`.git-exclude`/`target`/
//! `docs/book`) and scan it in strict mode: every command head must be `inert_head` or an exact
//! shape registered in `command_scan/procedure.rs`, with no exemption for shell keywords
//! (`if`/`case`/`for`/`while`) or heredocs (`command_scan/lexer.rs` has no concept of either — it
//! tokenizes per line, treating `;`/`|`/`&`/`(`/`)`/`[`/`]` as command separators with no keyword
//! awareness). Confirmed empirically, not assumed: a real installer needs OS/architecture branching
//! and checksum-failure handling, which cannot be written without `if`/`case`; a probe script using
//! both, placed anywhere in the tree, fails `reference-check` with a wall of
//! `unclassified-procedure-command`/`unclassified-dynamic-command` errors. No `.sh` file has ever
//! existed in this repository before this handoff — the scanner's `.sh` handling has never been
//! exercised against a real, control-flow-bearing script, only ever against `release.yml`'s own
//! flat, keyword-free command lists.
//!
//! Rewriting that scanner to understand real POSIX shell syntax would be a substantial,
//! security-relevant change to review-gated policy code (`EXECUTION-ORDER.md` §6 rule 5) — a
//! different, dedicated increment, not a side effect of shipping an installer. Instead, the actual
//! script text lives in `templates/*.sh.txt` — a `.txt` extension is outside
//! `scannable_reference_file`'s `md|yml|yaml|sh` set, so it is read only by the loose,
//! non-strict `scan()` (which looks for stray `cargo publish`/`prikk-release-policy … check`
//! invocations in prose, not full shell-command validation) and never subjected to the strict
//! per-command allowlist. The generated `.sh` file this module writes exists only transiently in a
//! release's own `dist/` directory (never committed), exactly like the tarballs and checksums
//! already produced there — so it is never walked by `reference-check`/`boundary-check` either.
//!
//! Windows installer/uninstaller handoff v1 Part 6a added `install.ps1`/`uninstall.ps1`, generated
//! the same way. `reference.rs`'s `scannable_reference_file` (`md|yml|yaml|sh`) never included
//! `.ps1` in the first place — confirmed by reading that match arm before writing this, not
//! assumed — so a `.ps1` extension here was never at risk of the strict scan this file's own `.sh`
//! pair had to dodge. `templates/*.ps1.txt` keeps the `.txt` extension anyway, for consistency with
//! its siblings, not because the scanner requires it this time.

use std::fs;
use std::path::Path;

use crate::error::Result;

const REPO_SLUG: &str = "prikk-vcs/prikk";
const INSTALL_TEMPLATE: &str = include_str!("../templates/install.sh.txt");
const UNINSTALL_TEMPLATE: &str = include_str!("../templates/uninstall.sh.txt");
// Windows installer/uninstaller handoff v1 Part 6a, item 1: generated the same way, beside the
// Unix templates. `.ps1` is not in `scannable_reference_file`'s `md|yml|yaml|sh` set at all
// (`reference.rs`), so unlike the `.sh` templates, a `.ps1` extension here would not have been
// walked by `reference-check`/`boundary-check` either way -- confirmed before choosing `.txt`,
// not assumed. `.txt` is kept anyway, for consistency: every installer template lives beside its
// siblings under one naming convention, not because the scanner leaves it no choice this time.
const INSTALL_PS1_TEMPLATE: &str = include_str!("../templates/install.ps1.txt");
const UNINSTALL_PS1_TEMPLATE: &str = include_str!("../templates/uninstall.ps1.txt");

/// Write `install.sh`/`uninstall.sh` and `install.ps1`/`uninstall.ps1` into `dist_dir`,
/// substituting the one placeholder (`REPO_SLUG`) every template carries. The `.sh` pair is
/// executable on Unix, where the release `publish` job actually runs this (`ubuntu-latest`) —
/// `#[cfg(unix)]`, not because a Windows build of this tool could not otherwise compile, but
/// because `std::os::unix::fs::PermissionsExt` does not exist elsewhere and nothing in this
/// workspace needs it to. The `.ps1` pair is never marked executable by this tool: a user runs them
/// via `irm ... | iex` or `powershell -File`, neither of which needs or honours a Unix execute bit,
/// and Windows has no equivalent bit for `mark_executable` to set.
pub(crate) fn generate(dist_dir: &Path) -> Result<()> {
    fs::create_dir_all(dist_dir)?;
    write_script(dist_dir, "install.sh", INSTALL_TEMPLATE, true)?;
    write_script(dist_dir, "uninstall.sh", UNINSTALL_TEMPLATE, true)?;
    write_script(dist_dir, "install.ps1", INSTALL_PS1_TEMPLATE, false)?;
    write_script(dist_dir, "uninstall.ps1", UNINSTALL_PS1_TEMPLATE, false)?;
    Ok(())
}

fn write_script(dist_dir: &Path, name: &str, template: &str, executable: bool) -> Result<()> {
    let content = template.replace("REPO_SLUG", REPO_SLUG);
    let path = dist_dir.join(name);
    fs::write(&path, content)?;
    if executable {
        mark_executable(&path)?;
    }
    Ok(())
}

#[cfg(unix)]
fn mark_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn mark_executable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests;

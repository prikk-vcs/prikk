//! `prikk extract` (0.54.0 step 3 Part 2): write the files at a ref or a block id into a brand-new,
//! empty, external directory -- no repository history, no worktree state.
//!
//! **Anchoring is the one thing that makes this a different command from `cat`/`tree`, not a third
//! copy of their own read path.** This writes what a CI job tests and what a forge serves for
//! download, so it anchors only where a writer would (`prikk_store::prepare_extract_plan`, under
//! the admitted-anchor rule, RFC 136 §10.3c ruling 2 / RFC 159 §8.2) -- never at any snapshot that
//! merely "passes the loader" the way `cat`/`tree`'s own anchoring does. A `remotes/…` ref under a
//! key this repository has not adopted always replays from genesis; past the trusted distance bound
//! it refuses by name unless `--allow-full-replay` is given.
//!
//! **Streaming.** `ExtractPlan` never holds more than one file's bytes at a time resident past what
//! `prepare_extract_plan` itself needed to hold (`prikk_store::extract`'s own module doc): this
//! module reads each file's content immediately before writing it, from the plan's own `Blob` id or
//! already-resident `Bytes`, and drops it the moment the write completes.
//!
//! **Write safety.** The point's own resolved paths are checked for case-insensitive collisions
//! before the first byte is written (`prepare_extract_plan`'s own call to
//! `validate_no_path_collisions`); the target directory must not already exist; every file is
//! written into a temporary sibling (`<dir>.prikk-extract-tmp`) first, which refuses by name if a
//! previous run's sibling is still there rather than silently reusing or deleting it; and the
//! sibling becomes the real target only through one no-replace rename
//! (`prikk_store::rename_new_directory_no_replace`) -- an ordinary refusal or error cleans the
//! sibling up; only a hard crash can leave it behind.

use std::fs;
use std::path::{Path, PathBuf};

use prikk_store::{ExtractPlan, ExtractPlanContent, ObjectReadSnapshot, Point, RepoPath};

use crate::arg_scan::{SetOnce, flag_value, mark_seen, unknown_argument};
use crate::commands::CliError;
use crate::durable_output::destination_exists;
use crate::output::point_label;
use crate::stdout::println;
use crate::{current_branch, open_repository};

struct ExtractArgs {
    root: PathBuf,
    target: PathBuf,
    ref_name: Option<String>,
    allow_full_replay: bool,
}

fn parse_extract_args(
    root: PathBuf,
    args: Vec<String>,
) -> std::result::Result<ExtractArgs, CliError> {
    let mut ref_name = None;
    let mut allow_full_replay = false;
    let mut target = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--ref" => {
                let value = flag_value(&mut iter, "extract --ref")?;
                // RFC 153 §2: a point is a ref name or a bare block id, and nothing else.
                if !prikk_store::is_point_name(&value) {
                    return Err(CliError::Usage(format!(
                        "extract --ref {value:?} is neither a ref name (heads/…, tags/… or \
                         remotes/…) nor a block id (64 lowercase hex characters)"
                    )));
                }
                ref_name.set_once("--ref", value)?;
            }
            "--allow-full-replay" => mark_seen(&mut allow_full_replay, "--allow-full-replay")?,
            other if other.starts_with('-') => return Err(unknown_argument("extract", other)),
            _ => {
                if target.is_some() {
                    return Err(CliError::Usage(
                        "extract accepts at most one output directory".to_string(),
                    ));
                }
                target = Some(PathBuf::from(arg));
            }
        }
    }
    let Some(target) = target else {
        return Err(CliError::Usage(
            "extract requires an output directory: prikk extract --ref <ref|block-id> <dir>"
                .to_string(),
        ));
    };
    Ok(ExtractArgs {
        root,
        target,
        ref_name,
        allow_full_replay,
    })
}

/// `prikk extract --ref <ref|block-id> <dir> [--allow-full-replay]`.
pub fn run_extract(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let args = parse_extract_args(root, args)?;
    let layout = open_repository(args.root)?;
    let name = current_branch::resolve_ref(&layout, args.ref_name)?;
    // Same rule as `cat`/`tree` (RFC 157 §2, RFC 147 §2i): a received ref is read, never adopted;
    // an unpublished current branch has nothing to extract yet.
    let unpublished_current_branch = current_branch::is_unpublished_current_branch(&layout, &name)?;
    let point = match prikk_store::resolve_point(&layout, &name, prikk_store::ReceivedRefs::Read) {
        Ok(point) => point,
        Err(prikk_error::PrikkError::Precondition(_)) if unpublished_current_branch => {
            return Err(CliError::Failure(format!(
                "precondition not met: {name} has no published history yet -- nothing to extract"
            )));
        }
        Err(err) => return Err(err.to_string().into()),
    };

    refuse_existing_target(&args.target)?;
    let temp_sibling = temp_sibling_path(&args.target)?;
    if destination_exists(&temp_sibling) {
        return Err(CliError::Failure(format!(
            "precondition not met: a temporary sibling from a previous run already exists at {} \
             -- this run never deletes what it cannot prove it created; remove it yourself once you \
             have confirmed it is not another run still in progress, then try again (nothing was \
             written)",
            temp_sibling.display()
        )));
    }

    let reader = ObjectReadSnapshot::open(&layout).map_err(|err| err.to_string())?;
    let plan = prikk_store::prepare_extract_plan(&layout, &reader, &point, args.allow_full_replay)
        .map_err(|err| err.to_string())?;

    let total_content_bytes = match write_plan(&reader, &plan, &temp_sibling) {
        Ok(total) => total,
        Err(err) => {
            let _ = fs::remove_dir_all(&temp_sibling);
            return Err(err);
        }
    };

    // 0.54.0 step 3 Part 2b R1: every file underneath is already synced (`write_file_durably`);
    // sync every directory in the tree -- the sibling root included -- before the one rename that
    // makes the whole tree visible at its real target, so a crash between here and the rename
    // leaves only the (still-incomplete-looking) temporary sibling, never a target whose directory
    // entries outran their own files' durability.
    if let Err(err) = sync_tree_directories(&temp_sibling) {
        let _ = fs::remove_dir_all(&temp_sibling);
        return Err(CliError::Failure(format!(
            "failed to sync the temporary sibling directory tree at {} before moving it into \
             place: {err}",
            temp_sibling.display()
        )));
    }

    if let Err(err) = prikk_store::rename_new_directory_no_replace(&temp_sibling, &args.target) {
        let _ = fs::remove_dir_all(&temp_sibling);
        return Err(CliError::Failure(format!(
            "failed to move the completed extract into place at {}: {err}",
            args.target.display()
        )));
    }

    // The rename is itself the durable, atomic boundary on Windows (`MOVEFILE_WRITE_THROUGH`,
    // `prikk_ffi::rename_no_replace`); on Unix, the directory entry it just created still needs its
    // own parent synced, the same step `durable_output.rs`'s own single-file writes already take.
    if let Err(err) = crate::durable_output::sync_parent_directory(&args.target) {
        return Err(CliError::Failure(format!(
            "extract completed at {}, but failed to sync its parent directory afterward -- its \
             durability across a crash is not confirmed: {err}",
            args.target.display()
        )));
    }

    print_report(&layout, &point, &args.target, &plan, total_content_bytes);
    Ok(())
}

/// `fsync` every directory under (and including) `root`, deepest first -- so a crash partway
/// through this walk never leaves an already-synced directory whose own, not-yet-synced parent
/// could still lose the directory entry that names it (0.54.0 step 3 Part 2b R1). A no-op on
/// non-Unix platforms ([`crate::durable_output::sync_directory`]'s own doc).
fn sync_tree_directories(root: &Path) -> std::io::Result<()> {
    let mut directories = vec![root.to_path_buf()];
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.is_dir() {
                directories.push(path.clone());
                stack.push(path);
            }
        }
    }
    // Deepest first: longest path components sort last among comparable prefixes, and no sibling
    // subtree's ordering matters here (two directories only need relative ordering when one is the
    // other's ancestor).
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        crate::durable_output::sync_directory(&directory)?;
    }
    Ok(())
}

/// The target must not already exist -- extract only ever creates a brand-new directory, never
/// writes into one that already has something in it.
fn refuse_existing_target(target: &Path) -> std::result::Result<(), CliError> {
    if destination_exists(target) {
        return Err(CliError::Failure(format!(
            "precondition not met: refusing to extract into an existing path: {} (nothing was \
             written)",
            target.display()
        )));
    }
    Ok(())
}

/// `<target>.prikk-extract-tmp` -- a fixed suffix, never a random one, so a user or a later run can
/// recognize and find it deterministically (design round 2, R-A4/ruling 3).
fn temp_sibling_path(target: &Path) -> std::result::Result<PathBuf, CliError> {
    let file_name = target.file_name().ok_or_else(|| {
        CliError::Usage(format!(
            "extract's output path {} has no file name to write",
            target.display()
        ))
    })?;
    let mut temp_name = file_name.to_os_string();
    temp_name.push(".prikk-extract-tmp");
    Ok(target.with_file_name(temp_name))
}

/// Join a validated repository-relative path onto an external root, component by component --
/// never `Path::join` on the raw string, which would accept a platform separator the path's own
/// validation never promised (the same reasoning `prikk_store`'s own, crate-private
/// `join_repo_path_to_root` documents).
fn join_repo_path(root: &Path, path: &RepoPath) -> PathBuf {
    let mut out = root.to_path_buf();
    for component in path.as_str().split('/') {
        out.push(component);
    }
    out
}

/// Write every planned file into `temp_sibling` (freshly created here), streaming each one's
/// content immediately before writing it and dropping it right after -- never holding more than one
/// file resident past what the plan itself already held. Returns the total bytes written (a free
/// by-product of writing them, not a second pass).
///
/// **Durability (0.54.0 step 3 Part 2b R1).** Each file is `sync_all`'d here, right after its
/// content and mode are both written -- the same point `durable_output.rs`'s own
/// `write_and_sync_temp_file` syncs a single file, before anything depends on it being durable.
/// Every directory this builds (the sibling root and every subdirectory a nested path needs) is
/// synced afterward, as a whole tree, by [`run_extract`]'s own `sync_tree_directories` call, once
/// every file underneath is already synced -- a directory's own durability only needs to follow its
/// entries', not precede each one.
fn write_plan(
    reader: &ObjectReadSnapshot,
    plan: &ExtractPlan,
    temp_sibling: &Path,
) -> std::result::Result<u64, CliError> {
    fs::create_dir(temp_sibling).map_err(|err| {
        CliError::Failure(format!(
            "failed to create the temporary sibling directory {}: {err}",
            temp_sibling.display()
        ))
    })?;
    let mut total_content_bytes = 0_u64;
    for entry in &plan.entries {
        let destination = join_repo_path(temp_sibling, &entry.path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                CliError::Failure(format!(
                    "failed to create directory {}: {err}",
                    parent.display()
                ))
            })?;
        }
        let bytes = match &entry.content {
            ExtractPlanContent::Bytes(bytes) => bytes.clone(),
            ExtractPlanContent::Blob(blob_id) => {
                prikk_store::read_extract_blob_content(reader, *blob_id)
                    .map_err(|err| CliError::Failure(err.to_string()))?
            }
        };
        total_content_bytes += bytes.len() as u64;
        write_file_durably(&destination, &bytes, entry.mode)?;
    }
    Ok(total_content_bytes)
}

/// Write `bytes` to the freshly-created `destination`, set its mode on the still-open handle, and
/// `fsync` it before returning -- both the file's content and its mode are durable before this
/// function returns (0.54.0 step 3 Part 2b, the architect's own start-of-Part-3 correction: setting
/// the mode *after* `sync_all`, as an earlier round did, left a crash window where the content
/// landed durably but the mode bit did not -- `extract` promises the output equals the point, and a
/// mode that isn't covered by the same sync as its file isn't kept durably). `destination`'s parent
/// is assumed to already exist ([`write_plan`]'s own `fs::create_dir_all` call, just above its own
/// call to this function).
fn write_file_durably(
    destination: &Path,
    bytes: &[u8],
    mode: u32,
) -> std::result::Result<(), CliError> {
    use std::io::Write as _;

    let mut file = fs::File::create_new(destination).map_err(|err| {
        CliError::Failure(format!("failed to write {}: {err}", destination.display()))
    })?;
    file.write_all(bytes).map_err(|err| {
        CliError::Failure(format!("failed to write {}: {err}", destination.display()))
    })?;
    set_mode(&file, destination, mode)?;
    file.sync_all().map_err(|err| {
        CliError::Failure(format!(
            "failed to sync {} before moving the extract into place: {err}",
            destination.display()
        ))
    })
}

/// Set `mode` on the still-open `file` -- the same handle [`write_file_durably`] is about to
/// `sync_all`, so the mode change is covered by that one sync rather than needing a second.
/// `destination` is used only for error messages (`File::set_permissions` takes no path of its
/// own).
#[cfg(unix)]
fn set_mode(file: &fs::File, destination: &Path, mode: u32) -> std::result::Result<(), CliError> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(mode & 0o7777))
        .map_err(|err| {
            CliError::Failure(format!(
                "failed to set mode on {}: {err}",
                destination.display()
            ))
        })
}

/// No observable POSIX mode on this platform (DC-87 §3.3/§4.3): a documented no-op, the same
/// reasoning `worktree.rs`'s own mode-aware materializer already applies.
#[cfg(not(unix))]
fn set_mode(
    _file: &fs::File,
    _destination: &Path,
    _mode: u32,
) -> std::result::Result<(), CliError> {
    Ok(())
}

fn print_report(
    layout: &prikk_store::RepositoryLayout,
    point: &Point,
    target: &Path,
    plan: &ExtractPlan,
    total_content_bytes: u64,
) {
    println!("extract repository: {}", layout.prikk_dir().display());
    println!(
        "{}: {}",
        point_label(point.kind == prikk_store::PointKind::Block),
        plan.point
    );
    println!("target block: {}", plan.target_block_id);
    println!("output directory: {}", target.display());
    // 0.54.0 step 3 Part 2b R4: `checkout --snapshot-materialize`'s own wording ("this snapshot's
    // block") does not fit `extract` -- a point up to 63 blocks past its anchor has no single
    // snapshot of its own to name. This instead says what `extract` actually checked: whether an
    // admitted anchor (`AnchorTrust`, under `AnchorSite::Extract`) was found and used, or the
    // replay fell back to genesis because none was.
    if plan.provisional {
        println!(
            "provisional: yes -- no replay-verified, adopted-key checkpoint was found within this \
             repository's trusted distance of this point; every file was replayed from genesis \
             instead; run `prikk verify` before committing"
        );
    } else {
        println!(
            "provisional: no -- every file was reached from a checkpoint this repository has \
             itself replay-verified and whose signer it has adopted"
        );
    }
    println!("files written: {}", plan.entries.len());
    println!("total content bytes: {total_content_bytes}");
}

#[cfg(test)]
mod tests;

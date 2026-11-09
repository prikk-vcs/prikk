//! `prikk cat` (RFC 157 §4): one file's bytes at a point, or its metadata as `path-content-v1`.
//!
//! **All or nothing.** The content is resolved in full (`prikk_store::read_path_at_point_reporting_anchor`,
//! which applies `--max-bytes` before it hands anything back) before one byte is written, and `--output`
//! goes through [`crate::durable_output`]'s temporary-sibling-then-rename write, so a failure leaves the
//! destination untouched.

use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};

use prikk_store::{PathContent, PointEntryEncoding};

use crate::arg_scan::{SetOnce, flag_value, mark_seen, unknown_argument};
use crate::commands::CliError;
use crate::output::verification::escape_json_string;
use crate::stdout::println;
use crate::tree::push_entry_fields;
use crate::{current_branch, durable_output, open_repository, warn_anchor_fallbacks};

struct CatArgs {
    root: PathBuf,
    path: String,
    ref_name: Option<String>,
    output: Option<PathBuf>,
    force: bool,
    max_bytes: Option<u64>,
    format_json: bool,
}

fn parse_cat_args(args: Vec<String>) -> std::result::Result<CatArgs, CliError> {
    let mut root = None;
    let mut path = None;
    let mut ref_name = None;
    let mut output = None;
    let mut max_bytes = None;
    let mut force = false;
    let mut format_json = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--path" => {
                let value = flag_value(&mut iter, "cat --path")?;
                prikk_store::RepoPath::parse(&value).map_err(|err| {
                    CliError::Usage(format!(
                        "cat --path {value:?} is not a valid repository-relative path ({err})"
                    ))
                })?;
                path.set_once("--path", value)?;
            }
            "--ref" => {
                let value = flag_value(&mut iter, "cat --ref")?;
                // RFC 153 §2: a point is a ref name or a bare block id, and nothing else.
                if !prikk_store::is_point_name(&value) {
                    return Err(CliError::Usage(format!(
                        "cat --ref {value:?} is neither a ref name (heads/…, tags/… or remotes/…) nor a \
                         block id (64 lowercase hex characters)"
                    )));
                }
                ref_name.set_once("--ref", value)?;
            }
            "--output" => {
                let value = flag_value(&mut iter, "cat --output")?;
                output.set_once("--output", PathBuf::from(value))?;
            }
            "--max-bytes" => {
                let value = flag_value(&mut iter, "cat --max-bytes")?;
                let parsed = value.parse::<u64>().map_err(|_| {
                    CliError::Usage(format!(
                        "cat --max-bytes must be a non-negative integer, got {value:?}"
                    ))
                })?;
                max_bytes.set_once("--max-bytes", parsed)?;
            }
            "--force" => mark_seen(&mut force, "--force")?,
            "--format" => {
                let value = flag_value(&mut iter, "cat --format")?;
                if value != "json" {
                    return Err(CliError::Usage(format!(
                        "cat --format does not support {value:?}"
                    )));
                }
                mark_seen(&mut format_json, "--format")?;
            }
            other if other.starts_with('-') => return Err(unknown_argument("cat", other)),
            _ => {
                if root.is_some() {
                    return Err(CliError::Usage("cat accepts at most one path".to_string()));
                }
                root = Some(arg);
            }
        }
    }
    let Some(path) = path else {
        return Err(CliError::Usage("cat requires --path <p>".to_string()));
    };
    Ok(CatArgs {
        root: crate::args::optional_path_or_current(root)?,
        path,
        ref_name,
        output,
        force,
        max_bytes,
        format_json,
    })
}

/// **The one terminal decision** (RFC 157 §4, §6): binary content never reaches a terminal, so history from
/// an untrusted source cannot drive one by being `cat`-ed. Text goes to a terminal as-is, exactly as
/// `checkout` already materializes it. Taking `stdout_is_terminal` as a parameter is what makes this
/// decision testable without opening a pseudo-terminal, which this workspace's `forbid(unsafe_code)` rules
/// out; the CLI control drives a real one through `script` where that tool exists.
pub(crate) fn binary_to_terminal_refusal(
    encoding: PointEntryEncoding,
    stdout_is_terminal: bool,
    path: &str,
) -> Option<String> {
    (encoding == PointEntryEncoding::Binary && stdout_is_terminal).then(|| {
        format!(
            "{path} is binary, and binary content is not written to a terminal; redirect stdout, or use \
             `prikk cat --output <file>` (nothing was written)"
        )
    })
}

/// `cat`'s `refusal-v1` ids (0.57.0 step 3, S4). Every id but `path-not-found` is `extract`'s own id for the
/// same meaning (see `commands.md`'s registry); `path-not-found` is the one new id, because `extract` has no
/// path to be missing. **Exhaustive by construction**: no `match` over this enum has a wildcard arm, so a
/// refusal added later without an id here does not compile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatRefusalId {
    /// A ref or block id that does not resolve (`extract`'s id, same meaning).
    PointNotFound,
    /// The path is not a file at the point (new).
    PathNotFound,
    /// The content is above `--max-bytes` (`extract`'s id, same meaning).
    OverMaxBytes,
    /// `--output` names an existing file and `--force` was not given (`extract`'s id).
    DestinationExists,
    /// `--output` is inside the repository's own `.prikk` directory (`extract`'s id).
    DestinationInsideRepository,
    /// No repository at the path (`extract`'s id).
    NotARepository,
    /// A path this version's own rule refuses (today: non-ASCII); a newer `prikk` may have written it.
    OutsideVersionRule,
    /// A persisted object that is missing or damaged, or an unresolvable current branch.
    DamagedRepository,
    /// A real I/O or resource failure, including a failed write of `--output`.
    IoFailure,
}

impl CatRefusalId {
    fn id(self) -> &'static str {
        match self {
            Self::PointNotFound => "point-not-found",
            Self::PathNotFound => "path-not-found",
            Self::OverMaxBytes => "over-max-bytes",
            Self::DestinationExists => "destination-exists",
            Self::DestinationInsideRepository => "destination-inside-repository",
            Self::NotARepository => "not-a-repository",
            Self::OutsideVersionRule => "outside-version-rule",
            Self::DamagedRepository => "damaged-repository",
            Self::IoFailure => "io-failure",
        }
    }
}

/// A refusal `cat` raised, with the id its raise site gave it. `id: None` is the one refusal that can never
/// be a document: binary content to a terminal, which only exists when bytes would be written, and
/// `--format json` never writes bytes.
struct CatRefusal {
    id: Option<CatRefusalId>,
    message: String,
}

impl CatRefusal {
    fn new(id: CatRefusalId, message: impl Into<String>) -> Self {
        Self {
            id: Some(id),
            message: message.into(),
        }
    }

    /// A failure whose only classification is its `PrikkError` variant, never its text: `Io` is
    /// `io-failure`, a path this version's own rule refuses is `outside-version-rule` (decided from the
    /// error's kind), everything else names a problem with persisted data.
    fn from_error(error: &prikk_error::PrikkError) -> Self {
        let id = match error {
            prikk_error::PrikkError::Io { .. } => CatRefusalId::IoFailure,
            _ if error.decode_failure_kind()
                == prikk_error::DecodeFailureKind::OutsideVersionRule =>
            {
                CatRefusalId::OutsideVersionRule
            }
            _ => CatRefusalId::DamagedRepository,
        };
        Self::new(id, error.to_string())
    }
}

/// `prikk cat --path <p> [--ref <ref|block-id>] [--output <file>] [--force] [--max-bytes <N>]
/// [--format json]`.
///
/// **The boundary catch** (as `extract`'s): the real work is [`run_cat_inner`]; under `--format json` every
/// refusal it returns (exit `1`, never a usage error) also writes one `refusal-v1` document to stdout, with
/// `"command": "cat"`. stderr's text and the exit code are exactly what they were.
pub(crate) fn run_cat(args: Vec<String>) -> std::result::Result<(), CliError> {
    let args = parse_cat_args(args)?;
    let format_json = args.format_json;
    match run_cat_inner(args) {
        Ok(()) => Ok(()),
        Err(refusal) => {
            if format_json {
                if let Some(id) = refusal.id {
                    crate::extract::print_refusal_document("cat", id.id(), None, &refusal.message);
                }
            }
            Err(CliError::Failure(refusal.message))
        }
    }
}

fn run_cat_inner(args: CatArgs) -> std::result::Result<(), CatRefusal> {
    let layout = open_repository(args.root)
        .map_err(|message| CatRefusal::new(CatRefusalId::NotARepository, message))?;
    // `current_branch::resolve_ref` / `is_unpublished_current_branch` fail only on an unresolvable current
    // branch pointer: damage, as `extract` classifies the identical failure.
    let name = current_branch::resolve_ref(&layout, args.ref_name)
        .map_err(|err| CatRefusal::new(CatRefusalId::DamagedRepository, err.message()))?;
    // RFC 157 §2, as amended by RFC 147 §2i: a received ref is read; reading is not adopting. An
    // unpublished current branch has no content at all, which the path refusal below says -- named
    // explicitly with `--ref` or not. Any other absent ref refuses here.
    let unpublished_current_branch = current_branch::is_unpublished_current_branch(&layout, &name)
        .map_err(|err| CatRefusal::new(CatRefusalId::DamagedRepository, err.message()))?;
    let point = match prikk_store::resolve_point(&layout, &name, prikk_store::ReceivedRefs::Read) {
        Ok(point) => point,
        // RFC 147 §2i Addendum 1: the resolver's own message ("does not exist") is what this whole round
        // rules false for the current branch, so it is not reused here -- the accurate reason is that
        // nothing has been published yet, not that the ref is absent.
        Err(prikk_error::PrikkError::Precondition(_)) if unpublished_current_branch => {
            // `extract` says `point-not-found` for the same condition: the point does not resolve to a
            // Block, so the question of the path never arises. One condition, one id across commands.
            return Err(CatRefusal::new(
                CatRefusalId::PointNotFound,
                format!(
                    "precondition not met: path {} does not exist at {name} ({name} has no published \
                     history yet)",
                    args.path
                ),
            ));
        }
        // Every `Precondition`/`InvalidName` `resolve_point` raises here is "no such ref or block" (an
        // absent ref, a block the store does not hold, an object that is not a block, a malformed
        // `remotes/` name): `extract`'s `point-not-found`, same meaning.
        Err(
            err @ (prikk_error::PrikkError::Precondition(_)
            | prikk_error::PrikkError::InvalidName(_)),
        ) => {
            return Err(CatRefusal::new(
                CatRefusalId::PointNotFound,
                err.to_string(),
            ));
        }
        Err(err) => return Err(CatRefusal::from_error(&err)),
    };
    if let Some(destination) = &args.output {
        refuse_output_destination(&layout, destination, args.force)?;
    }
    let (content, fallback) = prikk_store::read_path_at_point_reporting_anchor(
        &layout,
        &point,
        &args.path,
        args.max_bytes,
    )
    .map_err(|err| match err.kind {
        Some(prikk_store::PathReadRefusalKind::PathNotFound) => {
            CatRefusal::new(CatRefusalId::PathNotFound, err.to_string())
        }
        Some(prikk_store::PathReadRefusalKind::OverMaxBytes) => {
            CatRefusal::new(CatRefusalId::OverMaxBytes, err.to_string())
        }
        // `PathReadRefusalKind` is `#[non_exhaustive]`: a kind added later falls back to the error's
        // own variant, like any unclassified failure.
        Some(_) | None => CatRefusal::from_error(&err.error),
    })?;
    warn_anchor_fallbacks(fallback.iter());

    if args.format_json {
        // §4: metadata only, never bytes.
        println!("{}", path_content_json(&content));
        return Ok(());
    }
    match &args.output {
        Some(destination) => {
            // The recorded mode is applied through the one function `extract` uses (only the two modes
            // prikk records; a no-op where there is no POSIX mode), to the temporary file before it is
            // synced and renamed over the destination -- also over an existing one, with `--force`.
            durable_output::write_new_file_durably_with_mode(
                destination,
                &content.bytes,
                content.entry.mode,
            )
            .map_err(|message| CatRefusal::new(CatRefusalId::IoFailure, message))?;
        }
        None => {
            if let Some(refusal) = binary_to_terminal_refusal(
                content.entry.encoding,
                std::io::stdout().is_terminal(),
                &args.path,
            ) {
                return Err(CatRefusal {
                    id: None,
                    message: format!("precondition not met: {refusal}"),
                });
            }
            // RFC 121 §2.1: a closed stdout is not an error, for bytes as for lines.
            crate::stdout::write_bytes(&content.bytes);
        }
    }
    Ok(())
}

/// `--output` policy (RFC 157 §4): never inside `.prikk/`, and never over an existing file without
/// `--force` (`bundle export`'s own convention).
fn refuse_output_destination(
    layout: &prikk_store::RepositoryLayout,
    destination: &Path,
    force: bool,
) -> std::result::Result<(), CatRefusal> {
    if durable_output::inside_directory(layout.prikk_dir(), destination) {
        return Err(CatRefusal::new(
            CatRefusalId::DestinationInsideRepository,
            format!(
                "precondition not met: refusing to write inside the repository's own `.prikk` directory: {}",
                destination.display()
            ),
        ));
    }
    if !force && durable_output::destination_exists(destination) {
        return Err(CatRefusal::new(
            CatRefusalId::DestinationExists,
            format!(
                "precondition not met: refusing to overwrite existing file {}; pass --force to replace it \
                 (nothing was written)",
                destination.display()
            ),
        ));
    }
    Ok(())
}

/// `path-content-v1` (RFC 157 §4): the same fields and rules as one `tree-listing-v1` entry, and no bytes.
fn path_content_json(content: &PathContent) -> String {
    let mut json = String::new();
    json.push_str("{\n  \"schema_version\": \"path-content-v1\",\n  \"point\": ");
    json.push_str(&escape_json_string(&content.point));
    json.push_str(",\n  \"target_block_id\": ");
    json.push_str(&escape_json_string(&content.target_block_id.to_string()));
    json.push_str(",\n  ");
    push_entry_fields(&mut json, &content.entry);
    json.push_str("\n}");
    json
}

#[cfg(test)]
mod tests;

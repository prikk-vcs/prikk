//! `prikk merge` command implementation.

use prikk_store::{MaintainerSigner, MergeExecutionReport, execute_merge};

use crate::args::MergeExecuteArgs;
use crate::commands::CliError;
use crate::open_repository;

/// Run `prikk merge` against already-parsed arguments. AUD-10: parsing is the caller's job now
/// (`parse_merge_execute_args`, called from `main.rs` before the signer is built), so that a bad
/// argument is refused before an unrelated missing-signer environment is ever consulted.
pub(crate) fn run_merge(
    args: MergeExecuteArgs,
    signer: &impl MaintainerSigner,
) -> std::result::Result<MergeExecutionReport, CliError> {
    let layout = open_repository(args.root)?;
    // 0.56.0 step W3b (R3): resolved here, the one place the CLI reads the current-branch pointer
    // (RFC 151 §2.2), and threaded down as a plain `Option<&str>` -- `branch_switch.rs`'s own idiom
    // for `switch_branch`'s `from` parameter, never read from inside the store crate.
    let current_branch = crate::current_branch::displayed_current_branch(&layout);
    execute_merge(
        &layout,
        args.baseline_block_id,
        &args.into_ref,
        &args.from_ref,
        signer,
        current_branch.as_deref(),
    )
    .map_err(|err| CliError::from(err.to_string()))
}

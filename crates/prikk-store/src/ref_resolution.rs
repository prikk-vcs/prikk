//! The one resolver for a ref a command consumes (RFC 132 refusal sweep, Addendum 2).
//!
//! **Consumers** -- commands whose operation reads an existing ref's state -- call
//! [`require_existing_ref`] before anything else they check. An absent name is a `Precondition`, never
//! damage; a received name is read, refused as received, or left to the command's own name validation,
//! per [`ReceivedRefs`]. Not consumers, and not callers: `sync have`, `sync build` and `bundle preview`,
//! which answer "none of it" as a state, and the creators (`commit --ref`, `seal --ref`, `branch create`'s
//! own name).
//!
//! An upper-layer module (RFC 149): it reads both the local ref store and the received-ref index.

use prikk_error::{PrikkError, Result};

use crate::foundation::layout::RepositoryLayout;
use crate::refs::{RefStore, validate_local_branch_ref, validate_local_tag_ref};

/// How a command treats a received ref (`remotes/…`) it is given (RFC 132 refusal sweep, Addendum 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReceivedRefs {
    /// The command reads received refs (`log`, `merge --from`, `merge-evidence`, `merge-plan`): a
    /// received name must exist as a received pointer.
    Read,
    /// The command cannot use a received ref, and today's answer for one is false: refuse it as
    /// received, naming the commands that do read received refs.
    Refused,
    /// The command's own name validation already refuses a received name truly (`ref namespace is
    /// reserved`); leave it to that.
    LeftToNameValidation,
}

/// The commands that read a received ref, and the one that takes it into a local branch. A factual list,
/// quoted by both refusals below.
const RECEIVED_REF_READERS: &str = "received refs are read by `prikk log`, `prikk merge-evidence`, \
     `prikk merge-plan` and `prikk bundle preview`, and taken into a local branch by `prikk merge --from`";

/// **The one resolver for a ref a command consumes** (RFC 132 refusal sweep, rule 1): refuse a name that
/// does not exist in this repository with `Precondition`, before anything else the command checks.
///
/// - A local name (`heads/…`, `tags/…`) exists when its pointer is published. A missing pointer whose log
///   still holds history is damage, not absence: that is left to the caller's own `Integrity` report.
/// - When the local name is absent but `remotes/<name>` exists, the refusal says so.
/// - A received name is treated per [`ReceivedRefs`].
/// - A name no validator accepts is left to the caller's own name validation.
///
/// # Errors
///
/// `Precondition` for an absent or (under [`ReceivedRefs::Refused`]) received ref; any read error.
pub fn require_existing_ref(
    layout: &RepositoryLayout,
    ref_name: &str,
    received_refs: ReceivedRefs,
) -> Result<()> {
    if ref_name.starts_with("remotes/") {
        if received_refs == ReceivedRefs::LeftToNameValidation
            || crate::received::validate_received_ref(ref_name).is_err()
        {
            return Ok(());
        }
        let exists = crate::received::read_received_pointer(layout, ref_name)?.is_some();
        return match (exists, received_refs) {
            (false, _) => Err(PrikkError::Precondition(format!(
                "ref {ref_name} does not exist in this repository"
            ))),
            (true, ReceivedRefs::Refused) => Err(PrikkError::Precondition(format!(
                "{ref_name} is a received ref, and this command does not accept received refs; \
                 {RECEIVED_REF_READERS}"
            ))),
            (true, _) => Ok(()),
        };
    }
    if validate_local_branch_ref(ref_name).is_err() && validate_local_tag_ref(ref_name).is_err() {
        return Ok(());
    }
    let ref_store = RefStore::new(layout.clone());
    if ref_store.read_current_ref_state_id(ref_name)?.is_some() {
        return Ok(());
    }
    let log = ref_store.replay_log(ref_name)?;
    if !log.records.is_empty() || log.trailing_partial_bytes != 0 || log.has_item_failure() {
        return Ok(());
    }
    let received = format!("remotes/{ref_name}");
    let received_note = if crate::received::read_received_pointer(layout, &received)?.is_some() {
        format!("; {received} exists: {RECEIVED_REF_READERS}")
    } else {
        String::new()
    };
    Err(PrikkError::Precondition(format!(
        "ref {ref_name} does not exist in this repository{received_note}"
    )))
}

//! The one resolver for a ref a command consumes (RFC 132 refusal sweep, Addendum 2).
//!
//! **Consumers** -- commands whose operation reads an existing ref's state -- call
//! [`require_existing_ref`] before anything else they check. An absent name is a `Precondition`, never
//! damage; a received name is read, refused as received, or left to the command's own name validation,
//! per [`ReceivedRefs`]. Not consumers, and not callers: `sync have`, `sync build` and `bundle preview`,
//! which answer "none of it" as a state, and the creators (`commit --ref`, `seal --ref`, `branch create`'s
//! own name).
//!
//! **Points** (RFC 153 §2 and §7.1, RFC 157 §2): a read that takes a point -- a ref or a bare block id --
//! calls [`resolve_point`], which decides a ref's existence through the same function
//! [`require_existing_ref`] is, and a block id's through [`resolve_point`]'s own block arm. Reading a
//! block is not adopting it: a block the store holds resolves whether or not any ref reaches it.
//!
//! An upper-layer module (RFC 149): it reads both the local ref store and the received-ref index.

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectId, ObjectType, RefStatePayload};

use crate::foundation::layout::RepositoryLayout;
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::point::{Point, PointKind, is_bare_block_id};
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
    decide_ref_existence(layout, ref_name, received_refs).map(|_| ())
}

/// What [`decide_ref_existence`] found for a name that did not refuse.
enum RefExistence {
    /// The name exists as this kind of ref (or, for a local name, its log holds history: damage the
    /// reader reports).
    Exists(PointKind),
    /// Left to the caller: a received name under [`ReceivedRefs::LeftToNameValidation`], or a name no
    /// validator accepts.
    LeftToCaller,
}

/// **The one decision on whether a named ref exists** -- [`require_existing_ref`] and [`resolve_point`]
/// both call it.
fn decide_ref_existence(
    layout: &RepositoryLayout,
    ref_name: &str,
    received_refs: ReceivedRefs,
) -> Result<RefExistence> {
    if ref_name.starts_with("remotes/") {
        if received_refs == ReceivedRefs::LeftToNameValidation
            || crate::received::validate_received_ref(ref_name).is_err()
        {
            return Ok(RefExistence::LeftToCaller);
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
            (true, _) => Ok(RefExistence::Exists(PointKind::ReceivedRef)),
        };
    }
    let kind = if validate_local_branch_ref(ref_name).is_ok() {
        PointKind::LocalBranch
    } else if validate_local_tag_ref(ref_name).is_ok() {
        PointKind::Tag
    } else {
        return Ok(RefExistence::LeftToCaller);
    };
    let ref_store = RefStore::new(layout.clone());
    if ref_store.read_current_ref_state_id(ref_name)?.is_some() {
        return Ok(RefExistence::Exists(kind));
    }
    let log = ref_store.replay_log(ref_name)?;
    if !log.records.is_empty() || log.trailing_partial_bytes != 0 || log.has_item_failure() {
        return Ok(RefExistence::Exists(kind));
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

/// **Resolve a point** (RFC 153 §7.1): a ref name to the Block its tip names, or a bare block id to that
/// Block, whether or not any ref reaches it.
///
/// - A ref's existence is decided exactly as [`require_existing_ref`] decides it, with the same
///   refusals; its tip is then read as every ref-addressed reader reads it.
/// - A block id the store does not hold: `Precondition`, "block <id> is not in this repository".
/// - A block id the store holds as another type: `Precondition`, "object <id> is a patch, not a block".
///
/// # Errors
///
/// `Precondition` for an absent ref, a received ref under [`ReceivedRefs::Refused`], or a block id that
/// names no block; `InvalidName` for a name that is not a point, or a received name left to name
/// validation; `Integrity` for a ref whose publication does not resolve; any read error.
pub fn resolve_point(
    layout: &RepositoryLayout,
    name: &str,
    received_refs: ReceivedRefs,
) -> Result<Point> {
    let object_store = ObjectReadSnapshot::open(layout)?;
    if is_bare_block_id(name) {
        let block_id: ObjectId = name.parse()?;
        return match object_store.read_object(block_id)? {
            None => Err(PrikkError::Precondition(format!(
                "block {block_id} is not in this repository"
            ))),
            Some(envelope) if envelope.object_type != ObjectType::Block => {
                let type_name = envelope.object_type.name();
                let article = if type_name.starts_with(['a', 'e', 'i', 'o', 'u']) {
                    "an"
                } else {
                    "a"
                };
                Err(PrikkError::Precondition(format!(
                    "object {block_id} is {article} {type_name}, not a block"
                )))
            }
            Some(_) => Ok(Point {
                name: name.to_string(),
                kind: PointKind::Block,
                block_id,
            }),
        };
    }
    let kind = match decide_ref_existence(layout, name, received_refs)? {
        RefExistence::Exists(kind) => kind,
        RefExistence::LeftToCaller => {
            return Err(PrikkError::InvalidName(format!(
                "{name} is not a point this command reads: expected a ref name (heads/…, tags/… or \
                 remotes/…) or a block id (64 lowercase hex characters)"
            )));
        }
    };
    let block_id = if kind == PointKind::ReceivedRef {
        received_tip_block(layout, &object_store, name)?
    } else {
        crate::refs::read_current_ref_tip_block(layout, &object_store, name)?
    };
    Ok(Point {
        name: name.to_string(),
        kind,
        block_id,
    })
}

/// The Block a received ref's tip names. A received RefState carries the origin's own name, so there is
/// no name check here (DC-85 §3A), unlike a local ref's.
fn received_tip_block(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    name: &str,
) -> Result<ObjectId> {
    let pointer = crate::received::read_received_pointer(layout, name)?.ok_or_else(|| {
        PrikkError::Precondition(format!("ref {name} does not exist in this repository"))
    })?;
    let envelope = object_store
        .read_typed(pointer.ref_state_id, ObjectType::RefState)?
        .ok_or_else(|| {
            PrikkError::Integrity(format!(
                "received ref {name} points to missing RefState {}",
                pointer.ref_state_id
            ))
        })?;
    let ref_state =
        RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)?;
    Ok(crate::refs::resolve_ref_tip_block(object_store, &ref_state)?.0)
}

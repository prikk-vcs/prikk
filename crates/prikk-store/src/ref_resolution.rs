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

use std::collections::BTreeSet;

use prikk_error::{PrikkError, Result};
use prikk_object::{BlockPayload, ObjectId, ObjectType, RefStatePayload};

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

/// Whether `ref_name` is a local branch that has never been published: no `RefState`, and a log that
/// holds no history at all. **Never** true for a name that [`require_existing_ref`] would refuse (a
/// typo, a tag, a received ref, a block id), and never true for a **damaged** branch -- a pointer gone
/// while its log still holds real history, which stays `Integrity`, distinct from this.
///
/// This does **not** decide whether `ref_name` is the repository's current branch: RFC 151 §2.2 reserves
/// reading that pointer to the CLI's own resolution module and `doctor`, so this store-layer function
/// never touches it. A caller that already resolved the current branch through that one reader combines
/// the two facts itself -- `ref_name == <the current branch>` and this -- to give an explicit `--ref`
/// naming it the same answer its implicit (no `--ref`) path already gives (RFC 147 §2i), instead of the
/// resolver's ordinary block-requiring path.
///
/// # Errors
///
/// Any read error.
pub fn is_unpublished_local_branch(layout: &RepositoryLayout, ref_name: &str) -> Result<bool> {
    if validate_local_branch_ref(ref_name).is_err() {
        return Ok(false);
    }
    let ref_store = RefStore::new(layout.clone());
    if ref_store.read_current_ref_state_id(ref_name)?.is_some() {
        return Ok(false);
    }
    let log = ref_store.replay_log(ref_name)?;
    Ok(log.records.is_empty() && log.trailing_partial_bytes == 0 && !log.has_item_failure())
}

/// **Resolve a point** (RFC 153 §7.1): a ref name to the Block its tip names, or a bare block id to that
/// Block, whether or not any ref reaches it.
///
/// - A ref's existence is decided exactly as [`require_existing_ref`] decides it, with the same
///   refusals; its tip is then read as every ref-addressed reader reads it.
/// - A block id the store does not hold: `Precondition`, `block <id> is not in this repository`.
/// - A block id the store holds as another type: `Precondition`, `object <id> is a patch, not a block`.
///
/// This never special-cases the current branch (RFC 147 §2i): a caller whose own answer for an
/// unpublished current branch is not a refusal (a bare `tree`, `diff`'s worktree form, `checkout
/// --plan-only`) checks [`is_unpublished_local_branch`] against the current branch first and never
/// reaches this call for that name; every other caller keeps today's refusal for it unchanged.
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
        require_block_given(&object_store, block_id)?;
        return Ok(Point {
            name: name.to_string(),
            kind: PointKind::Block,
            block_id,
        });
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

/// **The one decision on whether a block id a caller gave names a block** -- [`resolve_point`]'s block arm,
/// and `merge-evidence`/`merge-plan`/`merge`'s `--baseline-block` (RFC 153 point-resolver handoff, Addendum 2).
/// A block id the user typed that names nothing is a caller precondition, not damage. A block reached **by
/// walking history** is not this: a missing parent found there stays the walker's own `Integrity`.
///
/// # Errors
///
/// `Precondition` when the store holds no object with this id, or holds one of another type; any read
/// error.
pub(crate) fn require_block_given(
    object_store: &impl ObjectReader,
    block_id: ObjectId,
) -> Result<()> {
    match object_store.read_object(block_id)? {
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
        Some(_) => Ok(()),
    }
}

/// Where a block stands among this repository's refs (0.57.0 step 3, S1): the question `branch create
/// --from <block id>` asks before it publishes a local branch at a block.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BlockStanding {
    /// A local branch or tag reaches it (the block is in the tip's ancestry). The ref names, sorted;
    /// at most the first few are listed.
    ReachedLocally(Vec<String>),
    /// No local branch or tag reaches it, but a received ref does: held only through what was
    /// received, which `archive import --adopt` takes in with its trust checks and nothing else does.
    ReceivedOnly(Vec<String>),
    /// The store holds the block and no ref of any kind reaches it (after `received remove`, say).
    Unreached,
}

/// How many ref names a [`BlockStanding`] lists.
const STANDING_NAMES_LISTED: usize = 3;

/// **Where `block_id` stands among the refs** (S1): which local branches or tags reach it, else which
/// received refs do, else neither. "Reaches" is the full-DAG ancestor closure of a tip (every parent
/// followed, as the merge reachability primitive does), inclusive of the tip itself.
///
/// A local ref's tip is read as every ref-addressed reader reads it (a tag through its Tag object), so
/// a tag reaches exactly what the Block it names reaches. The walk
/// over local refs shares one visited set: a block seen from an earlier tip has had its whole
/// ancestry searched already, so a repository of many branches walks each block once. Received refs
/// are walked one by one, because the answer lists them (and only a refusal asks).
///
/// # Errors
///
/// Any read error, including `Integrity` for a block in an ancestry the store cannot read: a damaged
/// history is reported, not read as "unreached".
pub fn block_standing(layout: &RepositoryLayout, block_id: ObjectId) -> Result<BlockStanding> {
    let object_store = ObjectReadSnapshot::open(layout)?;
    let ref_store = RefStore::new(layout.clone());
    let mut local = Vec::new();
    let mut seen = BTreeSet::new();
    let mut names: Vec<String> = ref_store
        .list_ref_pointers()?
        .into_iter()
        .map(|pointer| pointer.ref_name)
        .filter(|name| name.starts_with("heads/") || name.starts_with("tags/"))
        .collect();
    names.sort();
    for name in names {
        let tip = crate::refs::read_current_ref_tip_block(layout, &object_store, &name)?;
        if reaches(&object_store, tip, block_id, &mut seen)? {
            local.push(name);
            if local.len() == STANDING_NAMES_LISTED {
                break;
            }
        }
    }
    if !local.is_empty() {
        return Ok(BlockStanding::ReachedLocally(local));
    }
    let mut received = Vec::new();
    for pointer in crate::received::list_received_pointers(layout)? {
        let tip = received_tip_block(layout, &object_store, &pointer.ref_name)?;
        if reaches(&object_store, tip, block_id, &mut BTreeSet::new())? {
            received.push(pointer.ref_name);
            if received.len() == STANDING_NAMES_LISTED {
                break;
            }
        }
    }
    if received.is_empty() {
        Ok(BlockStanding::Unreached)
    } else {
        Ok(BlockStanding::ReceivedOnly(received))
    }
}

/// **The block `branch create <new_branch> --from <block id>` publishes at** (0.57.0 step 3, S1): the id
/// through [`resolve_point`] (a block the store holds; an absent id or one of another kind refuses
/// there, saying what it is), then the one question this command adds -- does a local branch or tag
/// reach it? Publishing a branch at a block only a received ref (or nothing) reaches would take in
/// history without the trust checks `archive import --adopt` makes, so it is refused by name, before
/// anything is written.
///
/// # Errors
///
/// `Precondition` as above; any read error.
pub fn resolve_branch_source_block(
    layout: &RepositoryLayout,
    new_branch: &str,
    block_id_text: &str,
) -> Result<ObjectId> {
    let point = resolve_point(layout, block_id_text, ReceivedRefs::Refused)?;
    let prefix = format!("branch create {new_branch} --from {block_id_text}");
    match block_standing(layout, point.block_id)? {
        BlockStanding::ReachedLocally(_) => Ok(point.block_id),
        BlockStanding::ReceivedOnly(refs) => Err(PrikkError::Precondition(format!(
            "{prefix}: block {block_id_text} is not reached by any local branch or tag; it is held \
             only through the received ref(s) {}. Publishing it as a local branch would bypass the \
             trust checks `prikk archive import --adopt` makes -- refusing, nothing was written. To \
             bring that history in with those checks, run `prikk archive import <the archive that \
             carried it> --adopt` (it adopts a branch whose name is free here; where a local branch \
             of that name exists the delivery stays under remotes/), then run this command again",
            refs.join(", ")
        ))),
        BlockStanding::Unreached => Err(PrikkError::Precondition(format!(
            "{prefix}: block {block_id_text} is in this repository but no ref reaches it (not a \
             local branch, a tag or a received ref; `prikk received remove` leaves blocks like \
             this), so there is no published history to branch from -- refusing, nothing was \
             written. Bring the history that carried it back with `prikk archive import <the \
             archive that carried it> --adopt`, then run this command again"
        ))),
    }
}

/// Whether `target` is `tip` or one of its ancestors. `seen` holds blocks already searched from an
/// earlier tip without finding `target`; a block in it is skipped.
fn reaches(
    object_store: &impl ObjectReader,
    tip: ObjectId,
    target: ObjectId,
    seen: &mut BTreeSet<ObjectId>,
) -> Result<bool> {
    let mut stack = vec![tip];
    while let Some(current) = stack.pop() {
        if current == target {
            return Ok(true);
        }
        if !seen.insert(current) {
            continue;
        }
        let envelope = object_store
            .read_typed(current, ObjectType::Block)?
            .ok_or_else(|| PrikkError::Integrity(format!("missing Block {current}")))?;
        let block = BlockPayload::decode_canonical(&envelope.canonical_payload)?;
        stack.extend(block.parent_block_ids.iter().copied());
    }
    Ok(false)
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

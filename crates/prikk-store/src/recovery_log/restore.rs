//! RFC 168 §3.2 and A1: planning and running the restore of a run, in undo order. Every step's conditions are checked against
//! the state each earlier undo step leaves, before anything is written; a step that already holds its result is done.

use super::*;

std::thread_local! {
    /// The kind the open run was begun as ([`begin_run_as`]): stamped on every entry it appends, as the entry's label.
    static CURRENT_KIND: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

/// The kind of a run `prikk uncommit` writes. Recorded in each entry's label, so a restore tells the run apart by what it recorded.
pub(crate) const UNCOMMIT_RUN_KIND: &str = "uncommit";

/// [`begin_run`], recording what kind of run it is: every entry it appends carries `kind` as its label (0.57.0 step 3, S5c).
pub(crate) fn begin_run_as(kind: &'static str) -> RunScope {
    let scope = begin_run();
    if scope.owned {
        CURRENT_KIND.with(|current| current.set(Some(kind)));
    }
    scope
}

pub(super) fn run_kind() -> Option<&'static str> {
    CURRENT_KIND.with(std::cell::Cell::get)
}

pub(super) fn clear_run_kind() {
    CURRENT_KIND.with(|current| current.set(None));
}

/// Whether run `id`'s entries recorded the `uncommit` kind.
pub(super) fn run_is_uncommit(layout: &RepositoryLayout, id: &str) -> Result<bool> {
    Ok(list(layout.repository_mutation_root())?
        .entries
        .iter()
        .any(|listed| listed.id == id && listed.entry.label == UNCOMMIT_RUN_KIND))
}

/// One restore condition: the fact it found, in the words the plan prints, and whether it holds (RFC 168 F2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Condition {
    pub(crate) text: String,
    pub(crate) holds: bool,
}

/// One step of a run's restore, in the order it is undone (A1 item 3: the reverse of the repair's own order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Step {
    pub(crate) source: String,
    pub(crate) kind: Kind,
    pub(crate) offset: u64,
    /// Already holds what this step would write: a restore run again skips it (A1 item 5).
    pub(crate) done: bool,
    pub(crate) conditions: Vec<Condition>,
    /// The bytes this step writes.
    pub(crate) bytes: usize,
}

/// What a run's restore would do (RFC 168 §3.2, A1). `refusal` is set when the run cannot run at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestorePlan {
    pub(crate) id: String,
    /// The first step's source and offset: the whole plan for a run of one step.
    pub(crate) source: String,
    pub(crate) offset: u64,
    /// The steps, in undo order.
    pub(crate) steps: Vec<Step>,
    /// Every condition of every step that is not done, flattened.
    pub(crate) conditions: Vec<Condition>,
    /// The bytes the whole run would write.
    pub(crate) would_write: usize,
    pub(crate) refusal: Option<String>,
    pub(crate) written: bool,
}

impl RestorePlan {
    /// Whether the run can run: no refusal, and every step that is not done has all its conditions.
    pub(crate) fn can_restore(&self) -> bool {
        self.refusal.is_none()
            && self
                .steps
                .iter()
                .all(|step| step.done || step.conditions.iter().all(|condition| condition.holds))
    }
}

/// An id is 16 hex characters, compared ignoring case (RFC 168 F9). Returns the lowercase form.
fn normalize_id(id: &str) -> Option<String> {
    (id.len() == ID_LEN && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| id.to_ascii_lowercase())
}

fn refused_plan(id: &str, reason: String) -> RestorePlan {
    RestorePlan {
        id: id.to_string(),
        source: String::new(),
        offset: 0,
        steps: Vec::new(),
        conditions: Vec::new(),
        would_write: 0,
        refusal: Some(reason),
        written: false,
    }
}

/// Whether a cut has been undone already: the file is exactly its state before the cut, the removed bytes after the offset.
fn cut_undone(current: Option<&[u8]>, entry: &Entry) -> bool {
    let Some(bytes) = current else { return false };
    let Ok(offset) = usize::try_from(entry.offset) else {
        return false;
    };
    bytes.len() == offset + entry.removed.len()
        && bytes
            .get(..offset)
            .is_some_and(|prefix| sha(prefix) == entry.prefix_hash)
        && bytes.get(offset..) == Some(entry.removed.as_slice())
}

/// How many of an append's removed bytes an interrupted restore already put back (0.59.0 K2): the file is longer than the cut's offset and shorter
/// than offset + removed, the bytes before the offset are the bytes the repair left, and the bytes after it are a prefix of the removed bytes. `None`
/// for any other state, which a restore refuses as it always did. A kill inside one `write` of more than a page can leave this state, and so can a
/// power loss; a kill at a syscall boundary cannot.
fn cut_partly_restored(current: Option<&[u8]>, entry: &Entry) -> Option<usize> {
    let bytes = current?;
    let offset = usize::try_from(entry.offset).ok()?;
    let have = bytes.len().checked_sub(offset).filter(|have| *have > 0)?;
    (have < entry.removed.len()
        && sha(bytes.get(..offset)?) == entry.prefix_hash
        && entry.removed.starts_with(bytes.get(offset..)?))
    .then_some(have)
}

/// Whether a file holds a strict prefix of the bytes a repair removed, the empty prefix included (0.59.0 K2): the state a restore's own interrupted
/// recreation of a removed file leaves (the file is created, and its bytes are written after). It is this step's own write, so the restore writes the
/// whole file again from offset 0.
fn remove_partly_restored(current: Option<&[u8]>, entry: &Entry) -> bool {
    current
        .is_some_and(|bytes| bytes.len() < entry.removed.len() && entry.removed.starts_with(bytes))
}

/// What a step writes, decided from the state it found.
enum Act {
    /// Append bytes to the file (a cut's undo, whole or the rest of an interrupted one).
    Append(Vec<u8>),
    /// Create a file that is absent.
    Create(Vec<u8>),
    /// Write the whole file again from offset 0, over an interrupted recreation of it.
    Rewrite(Vec<u8>),
    /// Overwrite at 0, then truncate to the bytes' length (a replace's undo).
    Replace(Vec<u8>),
}

/// The state each file would be in after the steps already undone (A1 item 3). Read from the disk once, then overlaid, so every
/// condition of a run is checked against the state each earlier step leaves, before anything is written.
struct Overlay<'a> {
    root: &'a MutationRoot,
    files: std::collections::BTreeMap<String, Option<Vec<u8>>>,
}

impl Overlay<'_> {
    fn get(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        if let Some(bytes) = self.files.get(path) {
            return Ok(bytes.clone());
        }
        let bytes = read_file_if_exists(self.root, Path::new(path))?;
        self.files.insert(path.to_string(), bytes.clone());
        Ok(bytes)
    }

    fn set(&mut self, path: &str, bytes: Option<Vec<u8>>) {
        self.files.insert(path.to_string(), bytes);
    }
}

/// Plan the restore of a run, and unless `plan_only`, write it when the plan can run (RFC 168 §3.2, F2, F8, F9, A1). Every step's
/// conditions are checked before any byte is written; the steps then run in undo order. A step that already holds its result is
/// done, so a run interrupted part-way finishes when it is run again. The caller holds the repair's locks.
pub(crate) fn restore(layout: &RepositoryLayout, id: &str, plan_only: bool) -> Result<RestorePlan> {
    #[cfg(test)]
    let _whole_read_scope =
        crate::foundation::fsutil::whole_read_guard::declare("recovery-log-identity");
    let root = layout.repository_mutation_root();
    let Some(run) = normalize_id(id) else {
        return Ok(refused_plan(
            id,
            format!(
                "a run id is {ID_LEN} hex characters; {id:?} is not one (`prikk doctor --recovery-list` prints the ids)"
            ),
        ));
    };
    let listing = list(root)?;
    let members: Vec<usize> = listing
        .entries
        .iter()
        .enumerate()
        .filter(|(_, listed)| listed.id == run)
        .map(|(index, _)| index)
        .collect();
    let Some(&first) = members.first() else {
        return Ok(refused_plan(
            id,
            format!("no recovery run has id {run}; `prikk doctor --recovery-list` prints the ids"),
        ));
    };
    let repair_sources = repairable_sources(layout)?;
    for &index in &members {
        let Some(listed) = listing.entries.get(index) else {
            continue;
        };
        let entry = &listed.entry;
        let candidate_remove =
            entry.kind == Kind::Remove && super::is_ref_candidate_source(&entry.source);
        if !candidate_remove && !repair_sources.contains(&entry.source) {
            let mut plan = refused_plan(
                &run,
                format!(
                    "a restore does not write {}: it is not on the repair list (the WAL, the pointer index, the ref log, the trust and \
                     received files, their generation logs, the commit witness and ref-name, the rename declarations and the worktree's record), so this run cannot be restored",
                    entry.source
                ),
            );
            plan.id.clone_from(&run);
            return Ok(plan);
        }
        let expected = meaning_paths_for(layout, &entry.source)?;
        let named: Vec<String> = entry
            .meaning
            .iter()
            .map(|meaning| meaning.path.clone())
            .collect();
        if expected != named {
            return Ok(refused_plan(
                &run,
                format!(
                    "the entry names the meaning files {named:?}, but the repair table gives {} the meaning files {expected:?}; the \
                     entry does not match the table, so it cannot be restored",
                    entry.source
                ),
            ));
        }
    }

    let Some(first_listed) = listing.entries.get(first) else {
        return Ok(refused_plan(id, "no recovery run matches".to_string()));
    };
    let first_entry = &first_listed.entry;
    let mut plan = RestorePlan {
        id: run.clone(),
        source: first_entry.source.clone(),
        offset: first_entry.offset,
        steps: Vec::new(),
        conditions: Vec::new(),
        would_write: 0,
        refusal: None,
        written: false,
    };
    let mut overlay = Overlay {
        root,
        files: std::collections::BTreeMap::new(),
    };
    // (source, what to write, done) for each step in undo order, so the writes repeat exactly what the checks saw.
    let mut writes: Vec<(String, Act, bool)> = Vec::new();
    let mut overlap: Option<(String, String)> = None;
    for &index in members.iter().rev() {
        let Some(listed) = listing.entries.get(index) else {
            continue;
        };
        let entry = &listed.entry;
        let current = overlay.get(&entry.source)?;
        let was_absent = current.is_none();
        let (done, mut bytes) = match entry.kind {
            Kind::Cut => (cut_undone(current.as_deref(), entry), entry.removed.clone()),
            Kind::Replace => (
                current
                    .as_deref()
                    .is_some_and(|bytes| sha(bytes) == entry.prefix_hash),
                entry.removed.clone(),
            ),
            // The file is back as the repair found it.
            Kind::Remove => (
                current
                    .as_deref()
                    .is_some_and(|bytes| sha(bytes) == entry.prefix_hash),
                entry.removed.clone(),
            ),
        };
        if done {
            plan.steps.push(Step {
                source: entry.source.clone(),
                kind: entry.kind,
                offset: entry.offset,
                done: true,
                conditions: Vec::new(),
                bytes: 0,
            });
            writes.push((entry.source.clone(), Act::Append(Vec::new()), true));
            // The file already is in its undone state: nothing to overlay.
            continue;
        }
        let mut conditions = Vec::new();
        let next = match entry.kind {
            Kind::Cut => {
                let offset = entry.offset;
                let partly = cut_partly_restored(current.as_deref(), entry);
                let length_ok = partly.is_some()
                    || current
                        .as_ref()
                        .is_some_and(|bytes| bytes.len() as u64 == offset);
                conditions.push(Condition {
                    text: match (partly, current.as_ref().map(|bytes| bytes.len() as u64)) {
                        (Some(have), _) => format!(
                            "the source is {} bytes: {have} of the {} removed bytes are already back, from an interrupted restore of this step, and the rest is appended",
                            offset + have as u64,
                            entry.removed.len()
                        ),
                        (None, Some(len)) if len == offset => {
                            format!("the source is {offset} bytes, the length the repair left")
                        }
                        (None, Some(len)) => {
                            format!("the source is {len} bytes; the repair left {offset}")
                        }
                        (None, None) => {
                            format!("the source is absent; the repair left {offset} bytes")
                        }
                    },
                    holds: length_ok,
                });
                let prefix_ok = length_ok
                    && current
                        .as_ref()
                        .and_then(|bytes| {
                            bytes.get(..usize::try_from(offset).unwrap_or(usize::MAX))
                        })
                        .is_some_and(|prefix| sha(prefix) == entry.prefix_hash);
                conditions.push(Condition {
                    text: if !length_ok {
                        "the bytes before the offset are not compared: the length differs"
                            .to_string()
                    } else if prefix_ok {
                        "the bytes before the offset are the bytes the repair left".to_string()
                    } else {
                        "the bytes before the offset have changed since the repair".to_string()
                    },
                    holds: prefix_ok,
                });
                if let Some(have) = partly {
                    bytes = entry.removed.get(have..).unwrap_or_default().to_vec();
                }
                // Undone, the file is its state before the cut: the bytes it had, then the removed bytes appended.
                let mut after = current.unwrap_or_default();
                after.truncate(usize::try_from(offset).unwrap_or(usize::MAX));
                if length_ok {
                    after.extend_from_slice(&entry.removed);
                }
                Some(after)
            }
            Kind::Remove => {
                // Recreated where nothing is, or finished where this step's own interrupted recreation left a strict prefix of the removed bytes
                // (the file is created, then written: a kill between leaves it empty or part-written). A file with any other bytes is not the
                // repair's to overwrite.
                let partly = remove_partly_restored(current.as_deref(), entry);
                let holds = current.is_none() || partly;
                conditions.push(Condition {
                    text: if current.is_none() {
                        format!("{} is absent, as the repair left it", entry.source)
                    } else if partly {
                        let have = current.as_ref().map_or(0, Vec::len);
                        if have == 0 {
                            format!(
                                "{} exists and is empty, from an interrupted restore of this step (the file is created, then written), and is written whole",
                                entry.source
                            )
                        } else {
                            format!(
                                "{} holds the first {have} of the {} removed bytes, from an interrupted restore of this step, and is written whole",
                                entry.source,
                                entry.removed.len()
                            )
                        }
                    } else {
                        format!(
                            "{} exists again, with other bytes than the repair removed",
                            entry.source
                        )
                    },
                    holds,
                });
                Some(entry.removed.clone())
            }
            Kind::Replace => {
                let holds = current
                    .as_ref()
                    .is_some_and(|bytes| sha(bytes) == entry.new_hash);
                conditions.push(Condition {
                    text: if holds {
                        format!("{} holds the bytes the repair wrote", entry.source)
                    } else {
                        format!(
                            "{} has changed since the repair wrote its bytes",
                            entry.source
                        )
                    },
                    holds,
                });
                Some(entry.removed.clone())
            }
        };
        for meaning in &entry.meaning {
            let now = overlay.get(&meaning.path)?.map(|bytes| sha(&bytes));
            let holds = now == meaning.hash;
            conditions.push(Condition {
                text: if holds {
                    format!("{} is unchanged since the repair", meaning.path)
                } else {
                    format!("{} has changed since the repair", meaning.path)
                },
                holds,
            });
        }
        if overlap.is_none() && conditions.iter().any(|condition| !condition.holds) {
            // A later run that touches this file is why the state is not what this step left (A1 item 6).
            overlap = listing
                .entries
                .iter()
                .skip(index + 1)
                .find(|listed| listed.id != run && listed.entry.source == entry.source)
                .map(|listed| (listed.id.clone(), entry.source.clone()));
        }
        overlay.set(&entry.source, next);
        plan.steps.push(Step {
            source: entry.source.clone(),
            kind: entry.kind,
            offset: entry.offset,
            done: false,
            conditions,
            bytes: bytes.len(),
        });
        let act = match entry.kind {
            Kind::Cut => Act::Append(bytes),
            Kind::Remove if was_absent => Act::Create(bytes),
            Kind::Remove => Act::Rewrite(bytes),
            Kind::Replace => Act::Replace(bytes),
        };
        writes.push((entry.source.clone(), act, false));
    }

    plan.conditions = plan
        .steps
        .iter()
        .filter(|step| !step.done)
        .flat_map(|step| step.conditions.iter().cloned())
        .collect();
    plan.would_write = plan.steps.iter().map(|step| step.bytes).sum();
    if let Some((later, source)) = overlap.filter(|_| !plan.can_restore()) {
        plan.refusal = Some(format!(
            "restore run {later} first: a later repair changed {source} after this run did, so this run's files are not as it left them"
        ));
    }
    if plan.can_restore() && !plan_only {
        for (source, act, done) in &writes {
            if *done {
                continue;
            }
            let relative = Path::new(source);
            match act {
                Act::Append(bytes) => append_file_required(root, relative, bytes)?,
                Act::Create(bytes) => {
                    ensure_directory_required(
                        root,
                        relative.parent().unwrap_or_else(|| Path::new("")),
                    )?;
                    create_new_file_required(root, relative, bytes).map_err(|error| {
                        PrikkError::MalformedData(format!("recreating {source}: {error}"))
                    })?;
                }
                Act::Rewrite(bytes) => overwrite_in_place_required(root, relative, 0, bytes)?,
                Act::Replace(bytes) => {
                    overwrite_in_place_required(root, relative, 0, bytes)?;
                    truncate_existing_file_required(root, relative, bytes.len() as u64)?;
                }
            }
        }
        plan.written = true;
    }
    Ok(plan)
}

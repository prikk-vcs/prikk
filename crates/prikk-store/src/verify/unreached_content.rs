//! 0.57.0 S5d: the one count line for unreached **content** -- the Patches and Blobs that nothing reaches.
//!
//! Almost every `prikk uncommit` leaves one (a text `create-file` writes a blob too), and `received remove` and
//! `doctor --discard-damaged-commits` leave them as well. Nothing was *published* under a Patch or a Blob, so there is no trust
//! question to warn about, and a warning per object buried the ones that matter. A Block or a RefState keeps its per-object
//! warning (its publication trust was not checked); `unreached_objects` in `verify --format json` still lists every object with
//! its type, so this only moves the detail to where a tool reads it. `verify` and `doctor` print the same sentence, built here.

use prikk_object::ObjectType;

use super::UnreachedObject;

/// How many unreached Patches and Blobs a repository holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnreachedContent {
    /// Unreached Patches.
    pub patches: usize,
    /// Unreached Blobs.
    pub blobs: usize,
}

impl UnreachedContent {
    /// The count of unreached content among `unreached`, or `None` when there is none (so no line is printed at all).
    #[must_use]
    pub fn of(unreached: &[UnreachedObject]) -> Option<Self> {
        let count = |wanted: ObjectType| {
            unreached
                .iter()
                .filter(|object| object.object_type == wanted)
                .count()
        };
        let content = Self {
            patches: count(ObjectType::Patch),
            blobs: count(ObjectType::Blob),
        };
        (content.total() > 0).then_some(content)
    }

    /// Patches plus Blobs.
    #[must_use]
    pub const fn total(&self) -> usize {
        self.patches + self.blobs
    }

    /// Whether an object of this type is counted here rather than warned about one by one.
    #[must_use]
    pub const fn counts(object_type: ObjectType) -> bool {
        matches!(object_type, ObjectType::Patch | ObjectType::Blob)
    }

    /// The sentence both `verify` and `doctor` print: what these objects are, that they are harmless, that nothing removes them
    /// yet, and where each one is listed.
    #[must_use]
    pub fn sentence(&self) -> String {
        format!(
            "unreached content: {total} object(s) ({patches} patch(es), {blobs} blob(s)) that nothing reaches -- content kept from \
             commits taken back (`prikk uncommit`), from received refs removed, or from discarded commits. Nothing was published \
             under them, so there is no trust to check; they are harmless, and no command removes them yet. \
             `prikk verify --format json` lists each one under `unreached_objects`",
            total = self.total(),
            patches = self.patches,
            blobs = self.blobs,
        )
    }
}

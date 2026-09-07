//! `WindowsAuthority`'s own module (DC-96 Windows Anchor Identity). Moved out of `directory.rs`
//! so its fields are genuinely private -- it used to share that module with the read-side walker
//! (`open_existing_windows_directory_for_read`), and field privacy binds at module granularity, so
//! privacy bought nothing while both lived there. **This module exposes no way to obtain a base
//! path from an authority without first re-deriving and verifying it.** Every resolver below
//! (`resolve_existing`/`resolve_prepared`/`resolve_existing_for_read`) and both `PlatformAuthority`
//! walks (`ensure_child`/`open_child`) call [`WindowsAuthority::verified_anchor_path`] before
//! touching anything -- there is no path accessor a future fourth walker could reach for instead.
//! That is what makes this a control rather than a convention (the distinction DC-90's own module
//! doc draws, applied one layer down): a second call site could be forgotten; a missing accessor
//! cannot compile around.
//!
//! **Prevention, not detection** (`.git-exclude/reviewed/DC-96-implementation-ruling-v1.md` §4,
//! correcting this module's first version, which stored a path string and verified identity
//! against it -- detection only, and wrong: the acceptance tests require operations to keep
//! working correctly against the retained directory after a replacement, not merely refuse). This
//! authority instead **retains the directory handle it was bound to.** Windows has no `openat` --
//! no primitive resolves a child by name against an already-open directory handle the way Linux
//! and macOS do -- but a retained handle still follows its object across a rename
//! (`GetFinalPathNameByHandle`, `prikk-ffi::current_path_of`). So every walk re-derives the
//! anchor's *current* path from the handle first, confirms the object found there is still the one
//! that was bound (`GetFileInformationByHandle`, `prikk-ffi::identity_of` -- the check-then-open
//! race closer, not the sole mechanism now), and only then walks forward. What this closes:
//! **anchor replacement between operations, and the walk continues correctly against the retained
//! object.** What it does not: a replacement racing the single verify-then-open pair on the anchor
//! itself (the window is narrowed, not closed), and G1's already-documented mid-walk reparse-point
//! race among the *relative* components of a walk (`windows.rs`'s own module doc), which this
//! module does not touch.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use prikk_error::{PrikkError, Result};

use super::directory::{PlatformAuthority, relative_components};
use super::{failpoints, windows};

/// The retained handle is the mechanism; `identity` is the confirmation that the object found at
/// the handle's re-derived current path is still the one `bind` (or the last walk) validated --
/// not the sole check, since `GetFinalPathNameByHandle` itself already follows the right object
/// regardless of what identity says. Not `Clone` directly (a `File` cannot be cheaply cloned);
/// always used behind `Arc`, matching Linux/macOS's `Arc<AnchoredDirectory>` shape exactly, for the
/// same reason -- `MutationRoot` itself is `Clone` and needs its authority to be too.
pub(super) struct WindowsAuthority {
    handle: std::fs::File,
    identity: prikk_ffi::FileIdentity,
}

fn io_error(path: &Path, error: std::io::Error) -> PrikkError {
    PrikkError::Io {
        kind: None,
        context: format!("{}: {error}", path.display()),
    }
}

impl WindowsAuthority {
    /// Re-derive this authority's current path from the retained handle, confirm the object found
    /// there is still the one that was bound, and return the path to walk from. Called before
    /// every walk in this module, **before** any component loop -- a relative path with zero
    /// components (a file directly in the anchor) must still be checked, or the empty-relative
    /// case -- the originally failing tests' own exact shape -- would stay unverified.
    fn verified_anchor_path(&self) -> Result<PathBuf> {
        let current_path = prikk_ffi::current_path_of(&self.handle)
            .map_err(|error| io_error(Path::new("<retained Windows anchor handle>"), error))?;
        // RFC 106: the only window this function's identity comparison guards -- a replacement
        // installed at `current_path` after it is captured above but before it is opened below.
        // No-op outside test builds, matching `failpoints::wait_at_directory_create` exactly.
        failpoints::wait_at_anchor_verification();
        let file = windows::open_directory_no_follow(&current_path)?;
        let identity = windows::identity_no_follow(&file, &current_path)?;
        // DC-99 Stage 2 found this comparison real and correct but unproven -- neutralizing it
        // (`if identity != self.identity` replaced with `if false`) left the full suite green,
        // 936/936, because neither DC-96 acceptance test constructs the race it guards: one returns
        // at a refused rename and never walks, the other completes its rename before anything else
        // runs, so `current_path_of`'s re-derivation alone already finds the retained directory and
        // this check confirms trivially. RFC 106 built the mechanism -- `failpoints::wait_at_-
        // anchor_verification` above holds this exact window open -- and constructed the race
        // directly: `windows_authority/tests.rs::a_replacement_installed_between_path_re_derivation-
        // _and_open_is_refused` installs a replacement in the window and asserts the refusal, and
        // passed on real Windows CI (run `32002318009`). With the comparison neutralized the same
        // way DC-99 did, that one test failed and was the only failure in the suite (run
        // `32002319494`) -- so this comparison now has a control that depends on it. What remains
        // unclosed is the window itself: Windows offers no `openat`-equivalent, so a replacement
        // racing the verify-then-open pair on the anchor is narrowed, not closed
        // (`platform-support.md`'s anchor-replacement section).
        if identity != self.identity {
            return Err(PrikkError::Integrity(format!(
                "Windows anchor replaced: {} no longer identifies the directory this authority \
                 was bound to",
                current_path.display()
            )));
        }
        Ok(current_path)
    }

    /// Resolve `relative` (creating any missing component) against this authority, verified
    /// first. Mirrors `prepare_directory_required`'s guarantee on the Unix side.
    pub(super) fn resolve_prepared(&self, relative: &Path) -> Result<PathBuf> {
        let mut current = self.verified_anchor_path()?;
        for component in relative_components(relative)? {
            current.push(component);
            windows::ensure_directory_component_no_follow(&current)?;
        }
        Ok(current)
    }

    /// Resolve `relative` against this authority, requiring every component to already exist,
    /// verified first. Mirrors `open_existing_directory_required`.
    pub(super) fn resolve_existing(&self, relative: &Path) -> Result<PathBuf> {
        let mut current = self.verified_anchor_path()?;
        for component in relative_components(relative)? {
            current.push(component);
            windows::open_directory_no_follow(&current)?;
        }
        Ok(current)
    }

    /// Resolve `relative` against this authority, verified first, returning `None` (not an error)
    /// as soon as any component is absent. Mirrors `open_existing_directory_for_read`. **Keeps
    /// the tolerant contract byte for byte** -- every `WindowsReader` method depends on `None`
    /// meaning "does not exist," not "error" -- this moved module, not meaning; unifying it with
    /// `resolve_existing`'s required contract is explicitly out of this increment's scope.
    pub(super) fn resolve_existing_for_read(&self, relative: &Path) -> Result<Option<PathBuf>> {
        let mut current = self.verified_anchor_path()?;
        for component in relative_components(relative)? {
            current.push(component);
            if windows::stat_directory_no_follow(&current)?.is_none() {
                return Ok(None);
            }
        }
        Ok(Some(current))
    }
}

impl PlatformAuthority for Arc<WindowsAuthority> {
    fn bind(path: &Path) -> Result<Self> {
        // Capture identity from the same handle that is retained -- not a second open, which
        // would be a fresh race inside the constructor.
        let handle = windows::open_directory_no_follow(path)?;
        let identity = windows::identity_no_follow(&handle, path)?;
        Ok(Arc::new(WindowsAuthority { handle, identity }))
    }

    fn same_as(&self, _self_path: &Arc<PathBuf>, other: &Self, _other_path: &Arc<PathBuf>) -> bool {
        // DC-96 prevention-fix-ruling-v1 §5: this authority is `Arc<WindowsAuthority>` now, exactly
        // like Linux/macOS's `Arc<AnchoredDirectory>` -- `Arc::ptr_eq` is available here for the
        // same reason it is there, and using it restores the trait's own stated contract
        // (`PlatformAuthority::same_as`'s doc: "not a new notion of identity"). An earlier version
        // of this compared `identity == identity` instead, which quietly changed what "same
        // authority" means on Windows (same on-disk object, rather than same retained authority)
        // and made `Lock::require_layout`'s cross-repository check rest solely on the 64-bit file
        // index -- exactly the property `platform-support.md` documents as not guaranteed unique
        // on ReFS. Identity is still the right tool for `verified_anchor_path`'s post-open
        // confirmation; it was never the right tool for this.
        Arc::ptr_eq(self, other)
    }

    fn ensure_child(&self, relative: &Path) -> Result<Self> {
        let mut current = self.verified_anchor_path()?;
        // Duplicate the retained handle unconditionally, matching Linux/macOS's own `dup(self)` at
        // the same point (`directory.rs`) -- a genuinely new, independent authority even for the
        // zero-component case, not a second reference to this one. Overwritten by the walk below
        // if `relative` has any components.
        let mut handle = self
            .handle
            .try_clone()
            .map_err(|error| io_error(&current, error))?;
        let mut identity = self.identity;
        for component in relative_components(relative)? {
            current.push(component);
            handle = windows::ensure_directory_component_no_follow(&current)?;
            identity = windows::identity_no_follow(&handle, &current)?;
        }
        Ok(Arc::new(WindowsAuthority { handle, identity }))
    }

    fn open_child(&self, relative: &Path) -> Result<Self> {
        let mut current = self.verified_anchor_path()?;
        let mut handle = self
            .handle
            .try_clone()
            .map_err(|error| io_error(&current, error))?;
        let mut identity = self.identity;
        for component in relative_components(relative)? {
            current.push(component);
            handle = windows::open_directory_no_follow(&current)?;
            identity = windows::identity_no_follow(&handle, &current)?;
        }
        Ok(Arc::new(WindowsAuthority { handle, identity }))
    }
}

#[cfg(test)]
mod tests;

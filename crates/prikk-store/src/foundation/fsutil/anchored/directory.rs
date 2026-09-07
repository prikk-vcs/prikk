//! Root-scoped directory capabilities and synchronization.

use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use prikk_error::{PrikkError, Result};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use rustix::fd::OwnedFd;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use rustix::fs::{self, Mode, OFlags};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use super::failpoints;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use super::io_error;

/// One platform's mutation authority: whatever it retains (or does not) to resolve a relative
/// path against a validated root. DC-87 Stage 1 -- this is the seam a future Windows implementor
/// (Stage 2) plugs into: one more `impl PlatformAuthority for ...` block, gated to its own
/// platform, with no change to `MutationRoot`'s own methods below. `bind`/`same_as`/`ensure_child`/
/// `open_child` are named after what `MutationRoot`'s own four platform-varying methods need, not
/// after any platform's primitive -- the same "guarantee, not syscall" discipline `contract.rs`
/// already holds `DurabilityContract` to.
pub(super) trait PlatformAuthority: Sized {
    /// Bind a fresh authority to `path`, matching `MutationRoot::open`'s own contract.
    fn bind(path: &Path) -> Result<Self>;
    /// True when `self`/`self_path` and `other`/`other_path` name the same retained authority.
    /// Takes both the authority value and its `MutationRoot`'s own path because a platform with no
    /// retained resource of its own (`PathOnlyAuthority`) has nothing else to compare identity on
    /// -- matching exactly what the pre-refactor inline `same_authority` compared on each platform,
    /// not a new notion of identity.
    fn same_as(&self, self_path: &Arc<PathBuf>, other: &Self, other_path: &Arc<PathBuf>) -> bool;
    /// Create (or validate an already-existing) nested authority at `relative`, resolved against
    /// `self`. Matches `MutationRoot::ensure_root`.
    fn ensure_child(&self, relative: &Path) -> Result<Self>;
    /// Bind to an existing nested authority at `relative`, resolved against `self`. Matches
    /// `MutationRoot::open_root`.
    fn open_child(&self, relative: &Path) -> Result<Self>;
}

/// Platform authority payload for `MutationRoot`. A single alias, not a `#[cfg]`'d field: the
/// struct below has one shape on every platform, and the platform difference lives entirely in
/// which concrete type this resolves to and that type's own `PlatformAuthority` impl.
#[cfg(any(target_os = "linux", target_os = "macos"))]
type Authority = Arc<AnchoredDirectory>;
#[cfg(target_os = "windows")]
type Authority = Arc<super::windows_authority::WindowsAuthority>;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
type Authority = PathOnlyAuthority;

/// A validated mutation authority rooted at one retained directory handle (Linux/macOS), at a
/// component-at-a-time-validated path (Windows, `WindowsAuthority` -- design-v1.md §2), or, on
/// every other platform, at a validated path alone with no per-component validation at all
/// (`PathOnlyAuthority`).
#[derive(Clone)]
pub(crate) struct MutationRoot {
    path: Arc<PathBuf>,
    authority: Authority,
}

impl fmt::Debug for MutationRoot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MutationRoot")
            .finish_non_exhaustive()
    }
}

impl MutationRoot {
    pub(crate) fn same_authority(&self, other: &Self) -> bool {
        self.authority
            .same_as(&self.path, &other.authority, &other.path)
    }

    /// Bind mutation authority to an existing no-follow directory handle.
    pub(crate) fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            path: Arc::new(path.to_path_buf()),
            authority: Authority::bind(path)?,
        })
    }

    /// Create and bind a nested root relative to this authority.
    pub(crate) fn ensure_root(&self, relative: &Path) -> Result<Self> {
        Ok(Self {
            path: Arc::new(self.fallback_path(relative)?),
            authority: self.authority.ensure_child(relative)?,
        })
    }

    /// Bind a nested existing root relative to this authority.
    pub(crate) fn open_root(&self, relative: &Path) -> Result<Self> {
        Ok(Self {
            path: Arc::new(self.fallback_path(relative)?),
            authority: self.authority.open_child(relative)?,
        })
    }

    pub(super) fn fallback_path(&self, relative: &Path) -> Result<PathBuf> {
        validate_relative(relative)?;
        Ok(self.path.join(relative))
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub(super) fn duplicate_directory(&self) -> Result<AnchoredDirectory> {
        let fd = rustix::io::dup(&self.authority.fd).map_err(io_error)?;
        Ok(AnchoredDirectory { fd })
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) struct AnchoredDirectory {
    pub(super) fd: OwnedFd,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl PlatformAuthority for Arc<AnchoredDirectory> {
    fn bind(path: &Path) -> Result<Self> {
        Ok(Arc::new(AnchoredDirectory::open(path)?))
    }

    fn same_as(&self, _self_path: &Arc<PathBuf>, other: &Self, _other_path: &Arc<PathBuf>) -> bool {
        Arc::ptr_eq(self, other)
    }

    fn ensure_child(&self, relative: &Path) -> Result<Self> {
        let mut current = dup(self)?;
        for component in relative_components(relative)? {
            current = current.ensure_child(component)?;
        }
        Ok(Arc::new(current))
    }

    fn open_child(&self, relative: &Path) -> Result<Self> {
        let mut current = dup(self)?;
        for component in relative_components(relative)? {
            current = current.open_validated_child(component)?;
        }
        Ok(Arc::new(current))
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn dup(directory: &AnchoredDirectory) -> Result<AnchoredDirectory> {
    let fd = rustix::io::dup(&directory.fd).map_err(io_error)?;
    Ok(AnchoredDirectory { fd })
}

/// Every platform that is neither Linux, macOS, nor Windows: no retained handle and no
/// per-component validation at all, by construction, matching `NoDurability`'s own "genuinely
/// unsupported" stance for these targets. This type exists so `PlatformAuthority` has a concrete
/// implementor for `none::NoDurability` and every test build on such a target; it carries the same
/// "unsupported" behavior `ensure_root`/`open_root` already had inline before Stage 1's refactor --
/// no behavior change, only where the platform difference is expressed.
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
#[derive(Clone)]
pub(super) struct PathOnlyAuthority;

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
impl PlatformAuthority for PathOnlyAuthority {
    fn bind(_path: &Path) -> Result<Self> {
        Ok(Self)
    }

    fn same_as(&self, self_path: &Arc<PathBuf>, _other: &Self, other_path: &Arc<PathBuf>) -> bool {
        // Preserves the pre-refactor non-Unix `same_authority` exactly: `Arc::ptr_eq` on the
        // `MutationRoot`'s own path, since this authority retains no comparable resource of its
        // own.
        Arc::ptr_eq(self_path, other_path)
    }

    fn ensure_child(&self, relative: &Path) -> Result<Self> {
        let _ = relative;
        super::unsupported_mutation()
    }

    fn open_child(&self, relative: &Path) -> Result<Self> {
        let _ = relative;
        Ok(Self)
    }
}

// Windows: no retained handle between steps, by construction, not as a shortcut -- design-v1.md
// §2's own gap statement. `WindowsAuthority` itself now lives in its own module (DC-96,
// `windows_authority.rs`) so its fields are genuinely private and every walk verifies the
// anchor's identity before touching them -- see that module's own doc for why. The three
// functions below are thin wrappers over its resolvers, kept here so `windows.rs`/`read.rs` do
// not need to know the authority moved.

/// Resolve (creating any missing component) `relative` against `root`'s own Windows authority,
/// returning the validated absolute path. Mirrors `prepare_directory_required`.
#[cfg(target_os = "windows")]
pub(super) fn prepare_windows_directory_required(
    root: &MutationRoot,
    relative: &Path,
) -> Result<PathBuf> {
    root.authority.resolve_prepared(relative)
}

/// Resolve `relative` against `root`'s own Windows authority, requiring every component to
/// already exist. Mirrors `open_existing_directory_required`.
#[cfg(target_os = "windows")]
pub(super) fn open_existing_windows_directory_required(
    root: &MutationRoot,
    relative: &Path,
) -> Result<PathBuf> {
    root.authority.resolve_existing(relative)
}

/// Resolve `relative` against `root`'s own Windows authority, returning `None` (not an error) as
/// soon as any component is absent. Mirrors `open_existing_directory_for_read`.
#[cfg(target_os = "windows")]
pub(super) fn open_existing_windows_directory_for_read(
    root: &MutationRoot,
    relative: &Path,
) -> Result<Option<PathBuf>> {
    root.authority.resolve_existing_for_read(relative)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl AnchoredDirectory {
    fn open(path: &Path) -> Result<Self> {
        failpoints::required_open()?;
        let fd = fs::open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io_error)?;
        Ok(Self { fd })
    }

    fn open_child(&self, name: &std::ffi::OsStr) -> Result<Self> {
        failpoints::required_open()?;
        let fd = fs::openat(
            &self.fd,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io_error)?;
        Ok(Self { fd })
    }

    fn open_child_for_read(&self, name: &std::ffi::OsStr) -> Result<Option<Self>> {
        failpoints::required_open()?;
        match fs::openat(
            &self.fd,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => Ok(Some(Self { fd })),
            Err(rustix::io::Errno::NOENT) => Ok(None),
            Err(error) => Err(io_error(error)),
        }
    }

    fn open_validated_child(&self, name: &std::ffi::OsStr) -> Result<Self> {
        let child = self.open_child(name)?;
        failpoints::observed_directory_parent_sync()?;
        self.sync()?;
        Ok(child)
    }

    fn ensure_child(&self, name: &std::ffi::OsStr) -> Result<Self> {
        failpoints::required_open()?;
        match fs::openat(
            &self.fd,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => {
                failpoints::observed_directory_parent_sync()?;
                self.sync()?;
                Ok(Self { fd })
            }
            Err(rustix::io::Errno::NOENT) => {
                failpoints::directory_create()?;
                failpoints::wait_at_directory_create();
                match fs::mkdirat(&self.fd, name, Mode::from_raw_mode(0o755)) {
                    Ok(()) => {
                        failpoints::created_directory_parent_sync()?;
                        self.sync()?;
                        self.open_child(name)
                    }
                    Err(rustix::io::Errno::EXIST) => self.open_validated_child(name),
                    Err(error) => Err(io_error(error)),
                }
            }
            Err(error) => Err(io_error(error)),
        }
    }

    /// Sync this directory's entry-list state — G3's worked example. `fsync` on a directory fd is
    /// Linux's guarantee; macOS documents `fsync` as insufficient (Apple's own `fsync(2)`: the drive
    /// may not have physically written the data, and may reorder it) and provides
    /// `fcntl(fd, F_FULLFSYNC)` as the stronger primitive (DC-76 addendum-1, confirmed against
    /// `rustix` 1.1.4 source: `fcntl_fullfsync` wraps it and is available on `apple`). This is the
    /// one place the two platforms' implementations genuinely diverge — everywhere else, identical
    /// `rustix` calls happen to also be correct on both, because `std::fs::File::sync_all`/
    /// `sync_data` already call `F_FULLFSYNC` internally on Apple targets (`library/std/src/sys/fs/unix.rs`,
    /// `os_fsync`/`os_datasync` under `cfg(target_vendor = "apple")`) — but a directory fd is never a
    /// `std::fs::File`, so this call site needs the platform branch explicitly.
    pub(super) fn sync(&self) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            fs::fsync(&self.fd).map_err(io_error)
        }
        #[cfg(target_os = "macos")]
        {
            fs::fcntl_fullfsync(&self.fd).map_err(io_error)
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn prepare_directory_required(
    root: &MutationRoot,
    relative: &Path,
) -> Result<AnchoredDirectory> {
    let mut current = root.duplicate_directory()?;
    for component in relative_components(relative)? {
        current = current.ensure_child(component)?;
    }
    Ok(current)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn open_existing_directory_required(
    root: &MutationRoot,
    relative: &Path,
) -> Result<AnchoredDirectory> {
    let mut current = root.duplicate_directory()?;
    for component in relative_components(relative)? {
        current = current.open_validated_child(component)?;
    }
    Ok(current)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn open_existing_directory_for_read(
    root: &MutationRoot,
    relative: &Path,
) -> Result<Option<AnchoredDirectory>> {
    let mut current = root.duplicate_directory()?;
    for component in relative_components(relative)? {
        let Some(child) = current.open_child_for_read(component)? else {
            return Ok(None);
        };
        current = child;
    }
    Ok(Some(current))
}

/// Split a validated relative path into its individual normal components. Platform-neutral logic
/// -- shared by the Unix fd-anchored walk and the Windows path-anchored walk (`WindowsAuthority`),
/// each of which resolves one component at a time for the same reason: no single primitive on
/// either platform resolves a whole relative path against a root in one call while refusing a
/// reparse point/symlink at every intermediate component, only at the last.
pub(super) fn relative_components(path: &Path) -> Result<Vec<&std::ffi::OsStr>> {
    validate_relative(path)?;
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => components.push(value),
            Component::RootDir | Component::ParentDir | Component::Prefix(_) => {
                return Err(PrikkError::Io {
                    kind: None,
                    context: "path must be relative to its authority root".to_string(),
                });
            }
        }
    }
    Ok(components)
}

fn validate_relative(path: &Path) -> Result<()> {
    for component in path.components() {
        if matches!(
            component,
            Component::RootDir | Component::ParentDir | Component::Prefix(_)
        ) {
            return Err(PrikkError::Io {
                kind: None,
                context: "path must be relative to its authority root".to_string(),
            });
        }
    }
    Ok(())
}

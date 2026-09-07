//! The macOS implementation of the durability contract (DC-76, ported by DC-81). Every method body
//! here is identical to `linux.rs`'s — the same `rustix` calls, the same failpoint call sites, the
//! same error handling — because DC-76's own prerequisite investigation established that every
//! `rustix` primitive this crate uses is available on `apple` targets, and `std::fs::File::sync_all`/
//! `sync_data` already call `fcntl(fd, F_FULLFSYNC)` on Apple targets internally
//! (`library/std/src/sys/fs/unix.rs`, `os_fsync`/`os_datasync` under `cfg(target_vendor = "apple")`).
//! The one place Linux and macOS genuinely diverge — a directory fd's own sync, which never goes
//! through `std::fs::File` — is handled inside `AnchoredDirectory::sync()` (`directory.rs`), shared
//! by both implementors; nothing below needs to know about it. `Macos` is a second implementor, not
//! a rewrite: DC-76's contract is proven platform-neutral by this file being a duplicate, not a
//! redesign.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use prikk_error::Result;

use super::directory::{
    MutationRoot, open_existing_directory_required, prepare_directory_required,
};
use super::regular::{
    open_append_regular, open_existing_regular, open_new_regular, required_file_name,
    required_parent,
};
use super::{failpoints, io_error, prikk_to_io};
use crate::foundation::fsutil::contract::DurabilityContract;
use crate::foundation::fsutil::temporary_path;

use rustix::fs::{self, OFlags};

/// The macOS durability contract implementation. Zero-sized: every method is stateless, dispatched
/// statically (no `dyn`), matching `LinuxDurability`'s shape exactly.
pub(crate) struct MacosDurability;

impl DurabilityContract for MacosDurability {
    fn atomic_replace(&self, root: &MutationRoot, relative: &Path, bytes: &[u8]) -> Result<()> {
        let parent = required_parent(relative)?;
        let destination = required_file_name(relative)?;
        let directory = prepare_directory_required(root, parent)?;
        let temp_path = temporary_path(relative)?;
        let temp_name = required_file_name(&temp_path)?;
        let fd = open_new_regular(&directory.fd, temp_name).map_err(io_error)?;
        let mut file = File::from(fd);
        file.write_all(bytes)?;
        failpoints::mutable_file_sync()?;
        file.sync_all()?;
        drop(file);
        failpoints::mutable_rename()?;
        fs::renameat(&directory.fd, temp_name, &directory.fd, destination).map_err(io_error)?;
        failpoints::mutable_parent_sync()?;
        directory.sync()
    }

    fn durable_append(&self, root: &MutationRoot, relative: &Path, bytes: &[u8]) -> Result<()> {
        let directory = open_existing_directory_required(root, required_parent(relative)?)?;
        let name = required_file_name(relative)?;
        let fd = open_append_regular(&directory.fd, name)?;
        let mut file = File::from(fd);
        failpoints::append_write()?;
        file.write_all(bytes)?;
        failpoints::required_file_sync()?;
        file.sync_all()?;
        failpoints::required_directory_sync()?;
        directory.sync()
    }

    fn durable_truncate(&self, root: &MutationRoot, relative: &Path, len: u64) -> Result<()> {
        let directory = open_existing_directory_required(root, required_parent(relative)?)?;
        let fd =
            open_existing_regular(&directory.fd, required_file_name(relative)?, OFlags::WRONLY)?;
        let file = File::from(fd);
        failpoints::truncate()?;
        file.set_len(len)?;
        failpoints::required_file_sync()?;
        file.sync_all()?;
        failpoints::required_directory_sync()?;
        directory.sync()
    }

    fn durable_truncate_to_empty(&self, root: &MutationRoot, relative: &Path) -> Result<()> {
        let directory = open_existing_directory_required(root, required_parent(relative)?)?;
        let fd =
            open_existing_regular(&directory.fd, required_file_name(relative)?, OFlags::WRONLY)?;
        let file = File::from(fd);
        failpoints::truncate()?;
        file.set_len(0)?;
        failpoints::required_file_sync()?;
        file.sync_all()?;
        failpoints::required_directory_sync()?;
        directory.sync()
    }

    fn create_exclusive(
        &self,
        root: &MutationRoot,
        relative: &Path,
        bytes: &[u8],
    ) -> std::io::Result<()> {
        let parent = required_parent(relative).map_err(prikk_to_io)?;
        let directory = prepare_directory_required(root, parent).map_err(prikk_to_io)?;
        let fd = open_new_regular(
            &directory.fd,
            required_file_name(relative).map_err(prikk_to_io)?,
        )
        .map_err(std::io::Error::from)?;
        let mut file = File::from(fd);
        file.write_all(bytes)?;
        failpoints::required_file_sync().map_err(prikk_to_io)?;
        file.sync_all()?;
        failpoints::required_directory_sync().map_err(prikk_to_io)?;
        directory.sync().map_err(prikk_to_io)
    }

    fn set_permission_bits(&self, root: &MutationRoot, relative: &Path, mode: u32) -> Result<()> {
        let directory = open_existing_directory_required(root, required_parent(relative)?)?;
        let fd =
            open_existing_regular(&directory.fd, required_file_name(relative)?, OFlags::RDONLY)?;
        // Permission bits only (0o7777): a recorded mode carries the S_IFREG file-type bits
        // (e.g. `0o100_755`), which `fchmod` does not accept. macOS's raw `mode_t` is `u16`
        // (rustix's `RawMode` for the `apple` backend), unlike Linux's `u32` — the mask keeps the
        // value within `0o7777` (<= 4095), so this narrowing always succeeds.
        let permission_bits = u16::try_from(mode & 0o7777).unwrap_or_default();
        fs::fchmod(&fd, fs::Mode::from_raw_mode(permission_bits)).map_err(io_error)
    }

    fn remove_if_present(&self, root: &MutationRoot, relative: &Path) -> Result<bool> {
        let directory = open_existing_directory_required(root, required_parent(relative)?)?;
        failpoints::unlink()?;
        let removed = match fs::unlinkat(
            &directory.fd,
            required_file_name(relative)?,
            fs::AtFlags::empty(),
        ) {
            Ok(()) => true,
            Err(rustix::io::Errno::NOENT) => false,
            Err(error) => return Err(io_error(error)),
        };
        failpoints::cleanup_directory_sync()?;
        directory.sync()?;
        Ok(removed)
    }

    fn ensure_directory(&self, root: &MutationRoot, relative: &Path) -> Result<()> {
        prepare_directory_required(root, relative)?;
        Ok(())
    }

    fn durable_directory_entry(&self, root: &MutationRoot, relative: &Path) -> Result<()> {
        let directory = open_existing_directory_required(root, required_parent(relative)?)?;
        failpoints::required_directory_sync()?;
        directory.sync()
    }
}

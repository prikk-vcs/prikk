// SPDX-License-Identifier: Apache-2.0
// adapted from the external review kit for prikk (reviews 053 and 055): `sctrace.c`, rewritten in Rust on `nix::sys::ptrace`.
//! **The real-kill sweep's driver (0.59.0 K3).** Runs one command under `ptrace`, counts its *mutating* syscalls, and `SIGKILL`s it at the *n*-th.
//!
//! **What it is, and what it is not.** It kills a process at a syscall boundary. It does not lose power (the failpoint rows hold fsync order), and it
//! does not tear a write inside one syscall (the direct restore tests hold that). Those three limits are the sweep's, and every place that names the
//! sweep says them.
//!
//! **Linux x86_64 only** (`orig_rax` and the argument registers; aarch64 is a second register path and a second table, and is not built).
//!
//! **No `unsafe` in this crate and no `pre_exec`.** The child is `sh -c 'read _; exec "$0" "$@"'` with a pipe on its stdin; the driver `PTRACE_SEIZE`s
//! it while it waits, then releases it by writing to the pipe. Counting starts at the `PTRACE_EVENT_EXEC` stop, so the shell's own syscalls are never
//! counted.
//!
//! **The table is closed.** Every syscall number the traced process issues is classified here as *mutating* (counted), *open* (counted when its flags
//! create or truncate) or *read-only / process plumbing* (not counted); **a syscall that is none of these ends the run with an error naming its number
//! and its name**, so a new `O_TRUNC`, `copy_file_range` or `fchmod` can never go unseen the way the reviewer's hand list could miss one.
//!
//! **Threads are refused.** A `clone`, `fork` or `vfork` event ends the run with an error: the ordinal is counted across tasks in arrival order, so it
//! is deterministic only while the writer is single-threaded.

#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::ffi::OsString;
use std::fs::File;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use nix::libc;
use nix::sys::ptrace::{self, Options};
use nix::sys::signal::{self, Signal};
use nix::sys::wait::{WaitStatus, waitpid};
use nix::unistd::Pid;

mod names;

/// Where in the *n*-th mutating syscall the kill lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    /// As the syscall is entered: the kernel has not performed it. Consecutive ordinals reach every state between two syscalls.
    Entry,
    /// As it returns: the kernel has performed it. **Measured in K1 to reach no state the entry sweep does not** (the state after ordinal *n* is the
    /// state at the entry of *n + 1*), so the sweep does not use it; the mode is kept for a write whose partial effect matters.
    Exit,
}

/// One mutating syscall the traced process issued.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mutating {
    /// Its ordinal, from 1.
    pub ordinal: u64,
    /// Its x86_64 syscall number.
    pub number: i64,
    /// Its name.
    pub name: &'static str,
}

/// What a traced run did.
#[derive(Debug)]
pub struct Trace {
    /// Every mutating syscall, in order (the whole run, or up to the kill).
    pub mutating: Vec<Mutating>,
    /// The syscall the kill landed on, when it did.
    pub killed_at: Option<Mutating>,
    /// The process's exit code, when it exited.
    pub exit_code: Option<i32>,
}

/// One syscall the table does not classify.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unclassified {
    /// Its number.
    pub number: i64,
    /// Its name (`"?"` when the name table has none).
    pub name: &'static str,
    /// The first three argument registers at its first entry, to help decide what it does.
    pub args: [u64; 3],
}

/// Why a traced run was refused.
#[derive(Debug)]
pub enum TraceError {
    /// The process issued a syscall the table does not classify. **The test fails and names it.**
    Unclassified(Vec<Unclassified>),
    /// The process created a task (`clone`, `fork` or `vfork`): the sweep is deterministic only while the writer is single-threaded.
    TaskCreated {
        /// The `PTRACE_EVENT_*` value.
        event: i32,
    },
    /// Spawning, attaching or waiting failed.
    Setup(String),
}

impl std::fmt::Display for TraceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unclassified(list) => {
                write!(
                    f,
                    "the traced process issued syscalls the sweep's table does not classify (decide whether each is mutating, read-only or plumbing, and \
                     add it to `tools/test-support/src/ptrace_sweep.rs`):"
                )?;
                for item in list {
                    write!(
                        f,
                        " {} ({}, first arguments {:#x?});",
                        item.number, item.name, item.args
                    )?;
                }
                Ok(())
            }
            Self::TaskCreated { event } => write!(
                f,
                "the traced process created a task (ptrace event {event}: clone, fork or vfork): the sweep counts ordinals across tasks in arrival order, so it \
                 is deterministic only while the writer is single-threaded"
            ),
            Self::Setup(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for TraceError {}

fn setup<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> Result<T, TraceError> {
    result.map_err(|error| TraceError::Setup(format!("{what}: {error}")))
}

fn name_of(number: i64) -> &'static str {
    names::NAMES
        .binary_search_by_key(&number, |(candidate, _)| *candidate)
        .map_or("?", |index| {
            names::NAMES.get(index).map_or("?", |(_, name)| *name)
        })
}

/// How the table classes one syscall.
enum Class {
    /// Counted.
    Mutating,
    /// An `open`-family call: counted when its flags (this register's value) create, truncate, or make a temporary file.
    Open(u64),
    /// Not counted: it reads, or it is process plumbing, or it touches no file's bytes.
    Quiet,
}

const O_CREAT: u64 = 0o100;
const O_TRUNC: u64 = 0o1000;
const O_TMPFILE: u64 = 0o20_200_000;

fn classify(number: i64, regs: &libc::user_regs_struct) -> Option<Class> {
    use libc::{
        SYS_access, SYS_arch_prctl, SYS_brk, SYS_chmod, SYS_chown, SYS_clock_gettime,
        SYS_clock_nanosleep, SYS_close, SYS_copy_file_range, SYS_creat, SYS_dup, SYS_dup2,
        SYS_dup3, SYS_execve, SYS_exit, SYS_exit_group, SYS_faccessat, SYS_faccessat2,
        SYS_fallocate, SYS_fchmod, SYS_fchmodat, SYS_fchown, SYS_fchownat, SYS_fcntl,
        SYS_fdatasync, SYS_fstat, SYS_fstatfs, SYS_fsync, SYS_ftruncate, SYS_futex, SYS_futimesat,
        SYS_getcwd, SYS_getdents, SYS_getdents64, SYS_getegid, SYS_geteuid, SYS_getgid, SYS_getpid,
        SYS_getppid, SYS_getrandom, SYS_getresgid, SYS_getresuid, SYS_getrlimit, SYS_gettid,
        SYS_getuid, SYS_ioctl, SYS_kill, SYS_lchown, SYS_link, SYS_linkat, SYS_lseek, SYS_lstat,
        SYS_madvise, SYS_mkdir, SYS_mkdirat, SYS_mknod, SYS_mknodat, SYS_mmap, SYS_mprotect,
        SYS_mremap, SYS_munmap, SYS_nanosleep, SYS_newfstatat, SYS_open, SYS_openat, SYS_poll,
        SYS_pread64, SYS_preadv, SYS_prlimit64, SYS_pwrite64, SYS_pwritev, SYS_pwritev2, SYS_read,
        SYS_readlink, SYS_readlinkat, SYS_readv, SYS_rename, SYS_renameat, SYS_renameat2,
        SYS_rmdir, SYS_rseq, SYS_rt_sigaction, SYS_rt_sigprocmask, SYS_rt_sigreturn,
        SYS_sched_getaffinity, SYS_sched_yield, SYS_sendfile, SYS_set_robust_list,
        SYS_set_tid_address, SYS_sigaltstack, SYS_stat, SYS_statfs, SYS_statx, SYS_symlink,
        SYS_symlinkat, SYS_sysinfo, SYS_truncate, SYS_uname, SYS_unlink, SYS_unlinkat, SYS_utime,
        SYS_utimensat, SYS_utimes, SYS_write, SYS_writev,
    };
    let number = number as libc::c_long;
    // Mutating: counted.
    if [
        SYS_write,
        SYS_pwrite64,
        SYS_writev,
        SYS_pwritev,
        SYS_pwritev2,
        SYS_fsync,
        SYS_fdatasync,
        SYS_ftruncate,
        SYS_truncate,
        SYS_fallocate,
        SYS_unlink,
        SYS_unlinkat,
        SYS_rmdir,
        SYS_rename,
        SYS_renameat,
        SYS_renameat2,
        SYS_mkdir,
        SYS_mkdirat,
        SYS_link,
        SYS_linkat,
        SYS_symlink,
        SYS_symlinkat,
        SYS_chmod,
        SYS_fchmod,
        SYS_fchmodat,
        SYS_chown,
        SYS_fchown,
        SYS_lchown,
        SYS_fchownat,
        SYS_utime,
        SYS_utimes,
        SYS_futimesat,
        SYS_utimensat,
        SYS_copy_file_range,
        SYS_sendfile,
        SYS_creat,
        SYS_mknod,
        SYS_mknodat,
    ]
    .contains(&number)
    {
        return Some(Class::Mutating);
    }
    // `ioctl` is classified by its request, not by its number: these only query a terminal or a pipe, or set a descriptor's close-on-exec flag (a prikk that asks whether its stdin is a
    // terminal issues `TCGETS2`); any other request (`FICLONE` writes a file's bytes) is unclassified and stops the run.
    if number == SYS_ioctl {
        const TCGETS: u64 = 0x5401;
        const TCGETS2: u64 = 0x802c_542a;
        const TIOCGWINSZ: u64 = 0x5413;
        const FIONREAD: u64 = 0x541b;
        // The close-on-exec flag of a descriptor: process plumbing, no file's bytes.
        const FIOCLEX: u64 = 0x5451;
        const FIONCLEX: u64 = 0x5450;
        return [TCGETS, TCGETS2, TIOCGWINSZ, FIONREAD, FIOCLEX, FIONCLEX]
            .contains(&regs.rsi)
            .then_some(Class::Quiet);
    }
    // `kill` with signal 0 sends nothing: it asks whether a process exists (a lock's holder, `ActiveLock`'s liveness probe, which `format upgrade` makes). Any other
    // signal is unclassified and stops the run.
    if number == SYS_kill {
        return (regs.rsi == 0).then_some(Class::Quiet);
    }
    if number == SYS_open {
        return Some(Class::Open(regs.rsi));
    }
    if number == SYS_openat {
        return Some(Class::Open(regs.rdx));
    }
    // Read-only, or process plumbing: not counted. Every entry is a syscall some writer of ours was seen to issue (`strace -f -c` over the
    // writers, K1 and K3), or one that only reads; an `ioctl` (`FICLONE` writes), `kill` with a signal, `io_uring_enter`, `openat2` and the rest are deliberately
    // absent, so the run that first issues one stops and says so.
    if [
        SYS_read,
        SYS_pread64,
        SYS_readv,
        SYS_preadv,
        SYS_lseek,
        SYS_close,
        SYS_dup,
        SYS_dup2,
        SYS_dup3,
        SYS_fstat,
        SYS_stat,
        SYS_lstat,
        SYS_newfstatat,
        SYS_statx,
        SYS_statfs,
        SYS_fstatfs,
        SYS_getdents,
        SYS_getdents64,
        SYS_readlink,
        SYS_readlinkat,
        SYS_access,
        SYS_faccessat,
        SYS_faccessat2,
        SYS_getcwd,
        SYS_fcntl,
        SYS_mmap,
        SYS_munmap,
        SYS_mprotect,
        SYS_mremap,
        SYS_brk,
        SYS_madvise,
        SYS_rt_sigaction,
        SYS_rt_sigprocmask,
        SYS_rt_sigreturn,
        SYS_sigaltstack,
        SYS_getrandom,
        SYS_getpid,
        SYS_getppid,
        SYS_gettid,
        SYS_getuid,
        SYS_geteuid,
        SYS_getgid,
        SYS_getegid,
        SYS_prlimit64,
        SYS_getrlimit,
        SYS_set_tid_address,
        SYS_set_robust_list,
        SYS_rseq,
        SYS_sched_getaffinity,
        SYS_sched_yield,
        SYS_poll,
        SYS_execve,
        SYS_arch_prctl,
        SYS_exit,
        SYS_exit_group,
        SYS_clock_gettime,
        SYS_clock_nanosleep,
        SYS_nanosleep,
        SYS_uname,
        SYS_futex,
        SYS_getresuid,
        SYS_getresgid,
        SYS_sysinfo,
    ]
    .contains(&number)
    {
        return Some(Class::Quiet);
    }
    None
}

/// The wait loop's resume: `PTRACE_CONT` before the exec (the shell's syscalls are not wanted), `PTRACE_SYSCALL` after it.
fn resume(pid: Pid, counting: bool, sig: Option<Signal>) -> Result<(), TraceError> {
    if counting {
        setup(ptrace::syscall(pid, sig), "PTRACE_SYSCALL")
    } else {
        setup(ptrace::cont(pid, sig), "PTRACE_CONT")
    }
}

/// Run `real` (its program, arguments, working directory and environment) under the driver, its stdout and stderr in `output`, and kill it at the
/// `kill_at`-th mutating syscall when that is given (`When` says whether at entry or at exit). With no `kill_at` the run completes and `Trace::mutating`
/// is the whole list: the ordinals to sweep.
///
/// # Errors
///
/// An unclassified syscall, a created task, or a failure to spawn, attach or wait.
pub fn trace(
    real: &Command,
    output: &Path,
    kill_at: Option<u64>,
    when: When,
) -> Result<Trace, TraceError> {
    let mut wrapper = Command::new("sh");
    wrapper
        .arg("-c")
        .arg("read _go; exec \"$0\" \"$@\"")
        .arg(real.get_program())
        .args(real.get_args());
    if let Some(dir) = real.get_current_dir() {
        wrapper.current_dir(dir);
    }
    for (key, value) in real.get_envs() {
        match value {
            Some(value) => wrapper.env(key, value),
            None => wrapper.env_remove(key),
        };
    }
    let program = resolve_program(real.get_program());
    let log = setup(File::create(output), "creating the output file")?;
    let log_err = setup(log.try_clone(), "cloning the output file")?;
    wrapper
        .stdin(Stdio::piped())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    let mut child = setup(wrapper.spawn(), "spawning the command")?;
    let pid =
        Pid::from_raw(i32::try_from(child.id()).map_err(|_| TraceError::Setup("pid".into()))?);
    let options = Options::PTRACE_O_TRACESYSGOOD
        | Options::PTRACE_O_TRACEEXEC
        | Options::PTRACE_O_TRACECLONE
        | Options::PTRACE_O_TRACEFORK
        | Options::PTRACE_O_TRACEVFORK
        | Options::PTRACE_O_EXITKILL;
    setup(ptrace::seize(pid, options), "PTRACE_SEIZE")?;
    {
        // Release the shell: it is blocked in `read`, and execs the command when the pipe delivers a line.
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| TraceError::Setup("no stdin pipe".into()))?;
        setup(stdin.write_all(b"\n"), "releasing the shell")?;
    }
    let mut trace = Trace {
        mutating: Vec::new(),
        killed_at: None,
        exit_code: None,
    };
    let mut counting = false;
    let mut in_syscall = false;
    let mut pending_exit_kill = false;
    let mut failure: Option<TraceError> = None;
    let mut unclassified: Vec<Unclassified> = Vec::new();
    loop {
        let status = setup(waitpid(pid, None), "waitpid")?;
        match status {
            WaitStatus::Exited(_, code) => {
                trace.exit_code = Some(code);
                break;
            }
            WaitStatus::Signaled(..) => break,
            WaitStatus::PtraceEvent(_, _, event) => {
                if std::env::var_os("PRIKK_SWEEP_DEBUG").is_some() {
                    eprintln!("ptrace event {event}");
                }
                if event == libc::PTRACE_EVENT_EXEC {
                    // The first exec is the shell's (a seize can land before or after it); counting starts at the exec of the command itself, found by
                    // what `/proc/<pid>/exe` resolves to, never by counting execs.
                    if std::fs::read_link(format!("/proc/{pid}/exe")).ok() == program {
                        counting = true;
                        // The next syscall stop is `execve`'s own exit.
                        in_syscall = true;
                    }
                } else if [
                    libc::PTRACE_EVENT_CLONE,
                    libc::PTRACE_EVENT_FORK,
                    libc::PTRACE_EVENT_VFORK,
                ]
                .contains(&event)
                {
                    failure = Some(TraceError::TaskCreated { event });
                    let _ = signal::kill(pid, Signal::SIGKILL);
                    let _ = waitpid(pid, None);
                    break;
                }
                resume(pid, counting, None)?;
            }
            WaitStatus::PtraceSyscall(_) => {
                in_syscall = !in_syscall;
                let mut kill_now = false;
                if in_syscall {
                    let regs = setup(ptrace::getregs(pid), "PTRACE_GETREGS")?;
                    let number = regs.orig_rax as i64;
                    if std::env::var_os("PRIKK_SWEEP_DEBUG").is_some() {
                        eprintln!("syscall {number} {} counting={counting}", name_of(number));
                    }
                    let counted = match classify(number, &regs) {
                        Some(Class::Mutating) => true,
                        Some(Class::Open(flags)) => {
                            flags & (O_CREAT | O_TRUNC) != 0 || flags & O_TMPFILE == O_TMPFILE
                        }
                        Some(Class::Quiet) => false,
                        None => {
                            if !unclassified.iter().any(|seen| seen.number == number) {
                                unclassified.push(Unclassified {
                                    number,
                                    name: name_of(number),
                                    args: [regs.rdi, regs.rsi, regs.rdx],
                                });
                            }
                            false
                        }
                    };
                    if counted {
                        let ordinal = trace.mutating.len() as u64 + 1;
                        let entry = Mutating {
                            ordinal,
                            number,
                            name: name_of(number),
                        };
                        trace.mutating.push(entry.clone());
                        if kill_at == Some(ordinal) {
                            trace.killed_at = Some(entry);
                            match when {
                                When::Entry => kill_now = true,
                                When::Exit => pending_exit_kill = true,
                            }
                        }
                    }
                } else if pending_exit_kill {
                    pending_exit_kill = false;
                    kill_now = true;
                }
                if kill_now {
                    setup(signal::kill(pid, Signal::SIGKILL), "SIGKILL")?;
                    // The tracee may already be gone (SIGKILL takes effect at once, even in a ptrace stop): ESRCH is the expected answer.
                    let _ = ptrace::cont(pid, None);
                } else {
                    resume(pid, counting, None)?;
                }
            }
            WaitStatus::Stopped(_, sig) => {
                // A signal-delivery stop: pass the signal on.
                resume(pid, counting, Some(sig))?;
            }
            other => {
                return Err(TraceError::Setup(format!(
                    "unexpected wait status {other:?}"
                )));
            }
        }
    }
    let _ = child.wait();
    // For measurement: `PRIKK_SWEEP_LOG=<file>` records `<ordinal> <number> <name>` for every mutating syscall of the run.
    if let Some(path) = std::env::var_os("PRIKK_SWEEP_LOG") {
        let lines: String = trace
            .mutating
            .iter()
            .map(|m| format!("{} {} {}\n", m.ordinal, m.number, m.name))
            .collect();
        let _ = std::fs::write(path, lines);
    }
    if failure.is_none() && !unclassified.is_empty() {
        failure = Some(TraceError::Unclassified(unclassified));
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(trace),
    }
}

/// The environment variables of `real`, for a caller that wants to report them.
#[must_use]
pub fn env_of(real: &Command) -> Vec<(OsString, Option<OsString>)> {
    real.get_envs()
        .map(|(key, value)| (key.to_owned(), value.map(ToOwned::to_owned)))
        .collect()
}

/// The path `/proc/<pid>/exe` will show for `program`: the file itself, or, for a bare name, the first match on `PATH`, canonicalised.
fn resolve_program(program: &std::ffi::OsStr) -> Option<std::path::PathBuf> {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return std::fs::canonicalize(path).ok();
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
            .and_then(|candidate| std::fs::canonicalize(candidate).ok())
    })
}

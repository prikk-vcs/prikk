//! A lock names its holder, not just a pid (0.57.0 step 2, part L1).
//!
//! A lock file used to record `pid=<n>` and nothing else, so `prikk unlock` could only ask "is there
//! a process with this number?" -- a question a reused pid, a reboot, or another container's pid
//! namespace each answer wrongly. The body now also records enough to ask the question that matters,
//! "is the process that took this lock still the one running?": its start time, and where a pid
//! number needs it to mean anything (Linux), the boot id and the pid namespace.
//!
//! # The body
//!
//! ```text
//! prikk-lock-v2            <- version line, always first
//! pid=1234
//! kind=active
//! os=linux
//! start=98765              <- the holder's start time (platform units, opaque beyond equality)
//! boot=<boot id>           <- Linux only
//! pidns=pid:[4026531836]   <- Linux only
//! timens=time:[4026531834] <- Linux only, when the kernel has time namespaces
//! machine=<machine id>     <- the machine's identity, where the platform has one
//! end-of-lock-v2           <- end line, always last
//! ```
//!
//! The file is created exclusively and then written, so it is visible before its body is complete
//! (`lock::create_lock_file`; step 0c's racy-entry horizon is that creation stamp, so the creation is
//! not changed). A reader can therefore catch a body cut anywhere. **The version line first and the
//! end line last make three shapes tell apart:** a *complete new* body has both; an *old* body
//! (0.56.0 and earlier: `pid=`, `kind=`, then the fixed `note=` line) has the `pid=` line first and
//! that exact note line last; **anything else is *incomplete*** (empty, cut anywhere, damaged), and
//! an incomplete body's pid is never used -- `pid=12` read from a half-written `pid=1234` must not
//! be asked about as process 12.
//!
//! 0.56.0's reader keeps only `pid=` and `kind=` lines and ignores the rest, so it lists a new body
//! unchanged (measured against the published binary in the L1 report).
//!
//! # The answers
//!
//! **appears running** -- the lock is known to come from this machine, the pid is alive, its start
//! time equals the recorded one, and (Linux) the boot id, the pid namespace and the time namespace
//! match. **does not appear running** -- *of a lock known to come from this machine*: the pid is gone,
//! or its start time differs (the number was reused), or the boot id differs (this machine
//! restarted). **unknown** -- the record is from an older prikk, or incomplete, or from another pid or
//! time namespace, another operating system or another machine, or a field cannot be read, or the
//! machine it came from cannot be established. Each answer carries a [`LivenessReason`] that says
//! which. "Appears running" is the one answer a refusal rests on (L2), so every path that can reach
//! it requires every field the platform records to be present and equal.
//!
//! # Facts are relative to their reader (0.57.0 step 2, L1b)
//!
//! A value one process recorded and another re-reads is comparable only when both read it in the same
//! frame. Three frames made L1 say "does not appear running" of a live holder, each measured:
//!
//! - **Time namespace.** Since Linux 5.6, `/proc/<pid>/stat` field 22 is reported with the *reader's*
//!   time-namespace boot-time offset added: the same live process read 139487 from inside a
//!   namespace unshared with `--boottime 1000` and 39487 from outside, in one PID namespace. So the
//!   time namespace is recorded and compared like the PID namespace.
//! - **A zombie leader with live threads.** A process whose main thread has exited while other
//!   threads run shows state `Z` and answers `kill(pid, 0)`. `Z` is gone only when the process has no
//!   other thread (`Threads:` in `/proc/<pid>/status`); with the count unreadable it is not called
//!   gone.
//! - **Another machine.** A repository on a shared drive holds other machines' locks. A differing boot
//!   id is a restart only of this machine, and a pid or start time of another machine says nothing
//!   about this one. So "gone", "reused" and "rebooted" need the machine identity to be equal, and a
//!   lock from a different machine reads unknown. On Linux an equal boot id already establishes the
//!   same machine (boot ids are random per boot); elsewhere, and on Linux when the boot id differs, the
//!   identity must be present on both sides and equal. The identity is `/etc/machine-id` (fallback
//!   D-Bus's copy) on Linux, `gethostuuid` on macOS, `MachineGuid` on Windows; the hostname is not
//!   used, even as a fallback, because hostnames repeat. Each is read once per process.
//!
//! The boot time of macOS is deliberately not recorded: it moves when the wall clock is stepped, so a
//! mismatch could be false, and "does not appear running" is the dangerous direction to be wrong in.
//! macOS and Windows start times are absolute instants, so unlike Linux's since-boot count they do
//! not repeat across a restart and need no boot id.

use std::sync::OnceLock;

/// The version line a complete new body starts with.
const VERSION_LINE: &str = "prikk-lock-v2";
/// The end line a complete new body finishes with.
const END_LINE: &str = "end-of-lock-v2";
/// The last line of a 0.56.0 body, exactly.
const OLD_NOTE_LINE: &str = "note=PR-007 lock has no stale-lock stealing yet";

/// Best-effort liveness of a lock's holder -- see the module doc for exactly what each answer
/// requires, and `unlock`'s module doc for why a negative or unknown answer is never authorization to
/// clear a lock on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PidLiveness {
    /// The process that took the lock is still running: the pid is alive and its start time (and,
    /// on Linux, boot id and pid namespace) equal the recorded ones.
    AppearsRunning,
    /// The process that took the lock does not appear to be running: the pid is gone, was reused by
    /// another process, or the machine restarted. **Not proof it is safe to clear.**
    DoesNotAppearRunning,
    /// The check could not be made, or the record cannot be trusted to make it; the
    /// [`LivenessReason`] says why.
    Unknown,
}

/// Why a lock got the [`PidLiveness`] it got -- the listing prints this so "unknown" is never
/// bare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivenessReason {
    /// Alive, same start time (and boot and namespace on Linux).
    SameProcess,
    /// No process has the recorded pid.
    ProcessGone,
    /// The recorded pid is alive but its start time differs: another process holds the number.
    PidReused,
    /// The boot id differs: the machine restarted since the lock was taken.
    Rebooted,
    /// A 0.56.0-or-older body: it records no start time.
    OlderPrikk,
    /// The body is empty, cut, or damaged.
    IncompleteBody,
    /// Taken in a pid or time namespace other than this one: the pid number, or the start time read
    /// for it, means something else here.
    OtherNamespace,
    /// Taken on another operating system.
    OtherOs,
    /// Taken on another machine (a shared drive): its pids, boot id and start times mean nothing
    /// here.
    OtherMachine,
    /// Whether the lock was taken on this machine cannot be established: the machine identity is
    /// missing or unreadable on one side, and nothing else (an equal boot id) settles it.
    MachineNotEstablished,
    /// A field the platform records is missing from the body, or cannot be read here.
    FieldUnreadable,
    /// The operating system would not say whether the pid exists, or this platform has no way to ask.
    CouldNotCheck,
}

impl LivenessReason {
    /// The words the listing and the refusal use.
    #[must_use]
    pub fn sentence(self) -> &'static str {
        match self {
            Self::SameProcess => {
                "the process that took the lock is still running (same pid and start time)"
            }
            Self::ProcessGone => "no process has the recorded pid",
            Self::PidReused => {
                "the recorded pid now belongs to a different process (its start time differs)"
            }
            Self::Rebooted => "the machine has restarted since the lock was taken",
            Self::OlderPrikk => {
                "written by an older prikk, which recorded no start time to check against"
            }
            Self::IncompleteBody => "the lock body is incomplete: still being written, or damaged",
            Self::OtherNamespace => {
                "taken in another pid or time namespace, where this pid number or start time means something else"
            }
            Self::OtherOs => "taken on another operating system",
            Self::OtherMachine => {
                "taken on another machine, whose processes and boot say nothing about this one"
            }
            Self::MachineNotEstablished => {
                "cannot tell whether this lock was taken on this machine: a machine identity is missing or unreadable"
            }
            Self::FieldUnreadable => {
                "a field of the holder record is missing or could not be read here"
            }
            Self::CouldNotCheck => {
                "the operating system would not say whether the recorded pid exists"
            }
        }
    }

    /// The [`PidLiveness`] this reason stands for.
    #[must_use]
    pub fn liveness(self) -> PidLiveness {
        match self {
            Self::SameProcess => PidLiveness::AppearsRunning,
            Self::ProcessGone | Self::PidReused | Self::Rebooted => {
                PidLiveness::DoesNotAppearRunning
            }
            Self::OlderPrikk
            | Self::IncompleteBody
            | Self::OtherNamespace
            | Self::OtherOs
            | Self::OtherMachine
            | Self::MachineNotEstablished
            | Self::FieldUnreadable
            | Self::CouldNotCheck => PidLiveness::Unknown,
        }
    }
}

/// The words for a lock's liveness, defined once: `prikk unlock`'s listing and `doctor`'s held-lock
/// finding both print this, so the three answers never read differently in two places.
#[must_use]
pub fn describe_liveness(liveness: PidLiveness, reason: LivenessReason) -> String {
    let reason = reason.sentence();
    match liveness {
        PidLiveness::AppearsRunning => format!("appears running -- do not clear: {reason}"),
        PidLiveness::DoesNotAppearRunning => {
            format!("does not appear to be running: {reason}; not proof it is safe to clear")
        }
        PidLiveness::Unknown => format!("unknown: {reason}; not proof it is safe to clear"),
    }
}

/// A lock body naming `pid` as its holder, with this process's own start time, boot id and pid
/// namespace -- for tests of other crates that need a lock file naming a pid they chose (a
/// dead one, say) that a real `prikk` run on this machine will judge by the real rules.
#[cfg(feature = "test-support")]
#[must_use]
pub fn lock_body_naming_pid_for_test(kind: &str, pid: u32) -> String {
    render_body(kind, pid, own_facts())
}

/// What can be known about one process for a holder record. `start` is opaque beyond equality.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HolderFacts {
    pub(crate) os: String,
    pub(crate) start: Option<String>,
    pub(crate) boot: Option<String>,
    pub(crate) pidns: Option<String>,
    /// Linux: `/proc/self/ns/time`. `/proc/<pid>/stat` field 22 carries the *reader's* time-namespace
    /// boot-time offset (Linux 5.6+), so a start time is comparable only between processes that read
    /// it in the same time namespace.
    pub(crate) timens: Option<String>,
    /// The machine's identity (see `platform::machine_id`): "the same machine" is what lets a
    /// boot id, a pid and a start time be compared at all.
    pub(crate) machine: Option<String>,
}

/// This process's own facts, read once per process (a lock taken is a hot path; the facts cannot
/// change while the process lives).
pub(crate) fn own_facts() -> &'static HolderFacts {
    static OWN: OnceLock<HolderFacts> = OnceLock::new();
    OWN.get_or_init(|| HolderFacts {
        os: std::env::consts::OS.to_string(),
        start: platform::own_start(),
        boot: platform::boot_id(),
        pidns: platform::pid_namespace(),
        timens: platform::time_namespace(),
        machine: platform::machine_id(),
    })
}

/// The body of a lock file for `pid`, holding `kind`.
pub(crate) fn render_body(kind: &str, pid: u32, facts: &HolderFacts) -> String {
    let mut body = format!("{VERSION_LINE}\npid={pid}\nkind={kind}\nos={}\n", facts.os);
    for (key, value) in [
        ("start", &facts.start),
        ("boot", &facts.boot),
        ("pidns", &facts.pidns),
        ("timens", &facts.timens),
        ("machine", &facts.machine),
    ] {
        if let Some(value) = value {
            body.push_str(&format!("{key}={value}\n"));
        }
    }
    body.push_str(END_LINE);
    body.push('\n');
    body
}

/// A complete new body, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Record {
    pub(crate) pid: u32,
    pub(crate) kind: String,
    pub(crate) facts: HolderFacts,
}

/// The three shapes a lock body can have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Body {
    /// Version line first, end line last.
    Complete(Record),
    /// A 0.56.0-or-older body, complete.
    Old { pid: u32, kind: String },
    /// Anything else. Its pid is never used.
    Incomplete,
}

pub(crate) fn parse_body(bytes: &[u8]) -> Body {
    let text = String::from_utf8_lossy(bytes);
    if !text.ends_with('\n') {
        return Body::Incomplete;
    }
    let lines: Vec<&str> = text.lines().collect();
    let (Some(first), Some(last)) = (lines.first(), lines.last()) else {
        return Body::Incomplete;
    };
    if *first == VERSION_LINE {
        return if *last == END_LINE && lines.len() >= 3 {
            parse_complete(lines.get(1..lines.len() - 1).unwrap_or_default())
        } else {
            Body::Incomplete
        };
    }
    if first.starts_with("pid=") && *last == OLD_NOTE_LINE {
        let mut pid = None;
        let mut kind = None;
        for line in &lines {
            if let Some(value) = line.strip_prefix("pid=") {
                pid = value.parse::<u32>().ok();
            } else if let Some(value) = line.strip_prefix("kind=") {
                kind = Some(value.to_string());
            }
        }
        if let (Some(pid), Some(kind)) = (pid, kind) {
            return Body::Old { pid, kind };
        }
    }
    Body::Incomplete
}

fn parse_complete(lines: &[&str]) -> Body {
    let mut pid = None;
    let mut kind = None;
    let mut facts = HolderFacts {
        os: String::new(),
        start: None,
        boot: None,
        pidns: None,
        timens: None,
        machine: None,
    };
    for line in lines {
        if let Some(value) = line.strip_prefix("pid=") {
            pid = value.parse::<u32>().ok();
        } else if let Some(value) = line.strip_prefix("kind=") {
            kind = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("os=") {
            facts.os = value.to_string();
        } else if let Some(value) = line.strip_prefix("start=") {
            facts.start = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("boot=") {
            facts.boot = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("pidns=") {
            facts.pidns = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("timens=") {
            facts.timens = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("machine=") {
            facts.machine = Some(value.to_string());
        }
    }
    match (pid, kind) {
        (Some(pid), Some(kind)) if !facts.os.is_empty() => {
            Body::Complete(Record { pid, kind, facts })
        }
        _ => Body::Incomplete,
    }
}

/// What the operating system says about one pid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProcessRead {
    Gone,
    /// The OS would not say.
    Unreadable,
    /// Alive; its start time if it could be read.
    Alive(Option<String>),
}

/// The two things judging needs from the machine, so a test can supply any machine it likes.
pub(crate) trait Probe {
    fn ours(&self) -> &HolderFacts;
    fn read(&self, pid: u32) -> ProcessRead;
}

pub(crate) struct SystemProbe;

impl Probe for SystemProbe {
    fn ours(&self) -> &HolderFacts {
        own_facts()
    }
    fn read(&self, pid: u32) -> ProcessRead {
        platform::read_process(pid)
    }
}

/// The reason a complete record's holder gets the answer it gets.
pub(crate) fn judge(record: &Record, probe: &dyn Probe) -> LivenessReason {
    let ours = probe.ours();
    if record.facts.os != ours.os {
        return LivenessReason::OtherOs;
    }
    // Every fact below -- a pid, a start time, a boot id -- is relative to a machine. "Does not
    // appear running" is said only of a lock known to come from this one: a repository on a shared
    // drive holds locks of machines whose process tables mean nothing here.
    let same_machine_known = match (&record.facts.machine, &ours.machine) {
        (Some(theirs), Some(mine)) if theirs != mine => return LivenessReason::OtherMachine,
        (Some(_), Some(_)) => true,
        _ => false,
    };
    if record.facts.os == "linux" {
        let (Some(rec_boot), Some(our_boot), Some(rec_ns), Some(our_ns)) = (
            &record.facts.boot,
            &ours.boot,
            &record.facts.pidns,
            &ours.pidns,
        ) else {
            return LivenessReason::FieldUnreadable;
        };
        // A restart is settled first: nothing that held a lock before it can hold it now, whatever
        // namespace it was in. The boot id is not namespaced, so a container sees the host's. A
        // differing boot id means a restart only of *this* machine; of another machine's lock it
        // says nothing about this one. (An equal boot id already establishes the same machine: boot
        // ids are random per boot.)
        if rec_boot != our_boot {
            return if same_machine_known {
                LivenessReason::Rebooted
            } else {
                LivenessReason::MachineNotEstablished
            };
        }
        if rec_ns != our_ns {
            return LivenessReason::OtherNamespace;
        }
        // `/proc/<pid>/stat` field 22 is reported with the reader's time-namespace offset added, so
        // a holder in another time namespace reads as a "different" start time while running. A
        // kernel without time namespaces has the link on neither side; the link on one side only is
        // a disagreement we cannot interpret.
        match (&record.facts.timens, &ours.timens) {
            (None, None) => {}
            (Some(theirs), Some(mine)) if theirs == mine => {}
            (Some(_), Some(_)) => return LivenessReason::OtherNamespace,
            _ => return LivenessReason::FieldUnreadable,
        }
    } else if !same_machine_known {
        return LivenessReason::MachineNotEstablished;
    }
    let Some(recorded_start) = &record.facts.start else {
        return LivenessReason::FieldUnreadable;
    };
    match probe.read(record.pid) {
        ProcessRead::Gone => LivenessReason::ProcessGone,
        ProcessRead::Unreadable => LivenessReason::CouldNotCheck,
        ProcessRead::Alive(None) => LivenessReason::FieldUnreadable,
        ProcessRead::Alive(Some(now)) if now == *recorded_start => LivenessReason::SameProcess,
        ProcessRead::Alive(Some(_)) => LivenessReason::PidReused,
    }
}

/// One lock body, inspected: what the listing shows, and the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Inspection {
    pub(crate) kind: String,
    /// `None` for an incomplete body: its pid is never shown or used.
    pub(crate) pid: Option<u32>,
    pub(crate) start: Option<String>,
    pub(crate) reason: LivenessReason,
}

pub(crate) fn inspect(bytes: &[u8], probe: &dyn Probe) -> Inspection {
    match parse_body(bytes) {
        Body::Complete(record) => {
            let reason = judge(&record, probe);
            Inspection {
                kind: record.kind,
                pid: Some(record.pid),
                start: record.facts.start,
                reason,
            }
        }
        Body::Old { pid, kind } => Inspection {
            kind,
            pid: Some(pid),
            start: None,
            reason: LivenessReason::OlderPrikk,
        },
        Body::Incomplete => Inspection {
            kind: "unknown".to_string(),
            pid: None,
            start: None,
            reason: LivenessReason::IncompleteBody,
        },
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::ProcessRead;

    /// `(state, start time)` from the text of a `/proc/<pid>/stat` line. The command name (field 2)
    /// is in parentheses and may itself contain spaces and parentheses, so fields are counted from
    /// the last `)`: the state is field 3 and the start time, in clock ticks since boot, field 22.
    pub(super) fn parse_stat(text: &str) -> Option<(char, String)> {
        let after = text.rsplit_once(')')?.1;
        let mut fields = after.split_whitespace();
        let state = fields.next()?.chars().next()?;
        let start = fields.nth(18)?;
        start.parse::<u64>().ok()?;
        Some((state, start.to_string()))
    }

    pub(super) fn own_start() -> Option<String> {
        let text = std::fs::read_to_string("/proc/self/stat").ok()?;
        parse_stat(&text).map(|(_, start)| start)
    }

    pub(super) fn boot_id() -> Option<String> {
        let text = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
        let id = text.trim();
        (!id.is_empty()).then(|| id.to_string())
    }

    pub(super) fn pid_namespace() -> Option<String> {
        let link = std::fs::read_link("/proc/self/ns/pid").ok()?;
        Some(link.to_string_lossy().into_owned())
    }

    /// The decision for one process's `/proc/<pid>/stat` text and its thread count. `threads` is the
    /// `Threads:` line of `/proc/<pid>/status`, `None` if it could not be read.
    ///
    /// State `Z` ("zombie") is *gone* only when no other thread of the process runs: a thread-group
    /// leader whose main thread has exited while other threads continue shows `Z` and is a live
    /// process (`kill(pid, 0)` succeeds on it, and the lock's holder may be one of those threads --
    /// a host program using the published library can hold a lock from a thread). A `Z` whose
    /// thread count is unknown is not called gone.
    pub(super) fn classify_stat(text: &str, threads: Option<u32>) -> ProcessRead {
        match parse_stat(text) {
            Some(('X', _)) => ProcessRead::Gone,
            Some(('Z', start)) => match threads {
                Some(0 | 1) => ProcessRead::Gone,
                Some(_) => ProcessRead::Alive(Some(start)),
                None => ProcessRead::Alive(None),
            },
            Some((_, start)) => ProcessRead::Alive(Some(start)),
            None => ProcessRead::Alive(None),
        }
    }

    /// The `Threads:` count in the text of `/proc/<pid>/status`.
    pub(super) fn parse_threads(status: &str) -> Option<u32> {
        status
            .lines()
            .find_map(|line| line.strip_prefix("Threads:"))
            .and_then(|value| value.trim().parse().ok())
    }

    pub(super) fn time_namespace() -> Option<String> {
        let link = std::fs::read_link("/proc/self/ns/time").ok()?;
        Some(link.to_string_lossy().into_owned())
    }

    /// `/etc/machine-id` (systemd's per-installation random id; fallback D-Bus's copy). Two machines
    /// differ unless an image was cloned without regenerating it, which systemd documents as a
    /// misconfiguration; a clone also needs a different boot id *and* a shared repository to matter.
    /// The hostname is not used, even as a fallback: hostnames repeat (`localhost`), and a wrong
    /// "same machine" is the dangerous direction. A machine with neither file reads unknown.
    pub(super) fn machine_id() -> Option<String> {
        for path in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
            if let Ok(text) = std::fs::read_to_string(path) {
                let id = text.trim();
                if !id.is_empty() && id != "uninitialized" {
                    return Some(id.to_string());
                }
            }
        }
        None
    }

    pub(super) fn read_process(pid: u32) -> ProcessRead {
        match super::pid_exists(pid) {
            Some(false) => return ProcessRead::Gone,
            None => return ProcessRead::Unreadable,
            Some(true) => {}
        }
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(text) => {
                let threads = std::fs::read_to_string(format!("/proc/{pid}/status"))
                    .ok()
                    .and_then(|status| parse_threads(&status));
                classify_stat(&text, threads)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => ProcessRead::Gone,
            Err(_) => ProcessRead::Alive(None),
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::ProcessRead;

    fn start_of(pid: u32) -> ProcessRead {
        match prikk_ffi::process_start_time(pid) {
            prikk_ffi::ProcessStart::Started(start) => ProcessRead::Alive(Some(start.to_string())),
            prikk_ffi::ProcessStart::Gone => ProcessRead::Gone,
            prikk_ffi::ProcessStart::Indeterminate => ProcessRead::Alive(None),
        }
    }

    pub(super) fn own_start() -> Option<String> {
        match start_of(std::process::id()) {
            ProcessRead::Alive(start) => start,
            _ => None,
        }
    }

    pub(super) fn boot_id() -> Option<String> {
        None
    }

    pub(super) fn pid_namespace() -> Option<String> {
        None
    }

    pub(super) fn time_namespace() -> Option<String> {
        None
    }

    /// The hardware UUID (`gethostuuid`, through `prikk-ffi`).
    pub(super) fn machine_id() -> Option<String> {
        prikk_ffi::machine_uuid()
    }

    pub(super) fn read_process(pid: u32) -> ProcessRead {
        match super::pid_exists(pid) {
            Some(false) => ProcessRead::Gone,
            None => ProcessRead::Unreadable,
            Some(true) => start_of(pid),
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::ProcessRead;

    fn start_of(pid: u32) -> ProcessRead {
        match prikk_ffi::process_start_time(pid) {
            prikk_ffi::ProcessStart::Started(start) => ProcessRead::Alive(Some(start.to_string())),
            prikk_ffi::ProcessStart::Gone => ProcessRead::Gone,
            prikk_ffi::ProcessStart::Indeterminate => ProcessRead::Alive(None),
        }
    }

    pub(super) fn own_start() -> Option<String> {
        match start_of(std::process::id()) {
            ProcessRead::Alive(start) => start,
            _ => None,
        }
    }

    pub(super) fn boot_id() -> Option<String> {
        None
    }

    pub(super) fn pid_namespace() -> Option<String> {
        None
    }

    pub(super) fn time_namespace() -> Option<String> {
        None
    }

    /// `MachineGuid` (`HKLM\SOFTWARE\Microsoft\Cryptography`, through `prikk-ffi`).
    pub(super) fn machine_id() -> Option<String> {
        prikk_ffi::machine_guid()
    }

    pub(super) fn read_process(pid: u32) -> ProcessRead {
        match super::pid_exists(pid) {
            Some(false) => ProcessRead::Gone,
            None => ProcessRead::Unreadable,
            Some(true) => start_of(pid),
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod platform {
    use super::ProcessRead;

    pub(super) fn own_start() -> Option<String> {
        None
    }

    pub(super) fn boot_id() -> Option<String> {
        None
    }

    pub(super) fn pid_namespace() -> Option<String> {
        None
    }

    pub(super) fn time_namespace() -> Option<String> {
        None
    }

    pub(super) fn machine_id() -> Option<String> {
        None
    }

    pub(super) fn read_process(_pid: u32) -> ProcessRead {
        ProcessRead::Unreadable
    }
}

/// Does a process with this pid exist: `Some(true)`, `Some(false)`, or `None` when the operating
/// system would not say (or this platform cannot be asked).
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn pid_exists(pid: u32) -> Option<bool> {
    let raw = i32::try_from(pid).ok()?;
    let rustix_pid = rustix::process::Pid::from_raw(raw)?;
    match rustix::process::test_kill_process(rustix_pid) {
        Ok(()) => Some(true),
        // `EPERM` means the kernel found a process to check permissions against -- it exists, this
        // caller simply cannot signal it. That is still existence, not absence.
        Err(rustix::io::Errno::PERM) => Some(true),
        Err(rustix::io::Errno::SRCH) => Some(false),
        Err(_) => None,
    }
}

/// DC-99: `ProcessLiveness::Exists` covers both a confirmed-running handle and the access-denied
/// case, the same two situations the Unix `Ok`/`EPERM` arms both map to "exists".
#[cfg(target_os = "windows")]
fn pid_exists(pid: u32) -> Option<bool> {
    match prikk_ffi::process_liveness(pid) {
        prikk_ffi::ProcessLiveness::Exists => Some(true),
        prikk_ffi::ProcessLiveness::DoesNotExist => Some(false),
        prikk_ffi::ProcessLiveness::Indeterminate => None,
    }
}

#[cfg(test)]
mod tests;

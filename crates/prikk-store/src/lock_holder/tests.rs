#![allow(clippy::indexing_slicing)]

use super::{
    Body, HolderFacts, LivenessReason, PidLiveness, Probe, ProcessRead, Record, SystemProbe,
    inspect, judge, own_facts, parse_body, render_body,
};

/// A machine that says whatever a test needs it to say.
struct FakeMachine {
    ours: HolderFacts,
    process: ProcessRead,
}

impl Probe for FakeMachine {
    fn ours(&self) -> &HolderFacts {
        &self.ours
    }
    fn read(&self, _pid: u32) -> ProcessRead {
        self.process.clone()
    }
}

fn linux_facts() -> HolderFacts {
    HolderFacts {
        os: "linux".to_string(),
        start: Some("5000".to_string()),
        boot: Some("boot-A".to_string()),
        pidns: Some("pid:[4026531836]".to_string()),
    }
}

fn windows_facts() -> HolderFacts {
    HolderFacts {
        os: "windows".to_string(),
        start: Some("133700000000000000".to_string()),
        boot: None,
        pidns: None,
    }
}

fn record(facts: HolderFacts) -> Record {
    Record {
        pid: 1234,
        kind: "active".to_string(),
        facts,
    }
}

fn judged(recorded: HolderFacts, ours: HolderFacts, process: ProcessRead) -> LivenessReason {
    judge(&record(recorded), &FakeMachine { ours, process })
}

#[test]
fn a_new_body_round_trips_through_render_and_parse() {
    let body = render_body("ref", 77, &linux_facts());
    assert_eq!(
        parse_body(body.as_bytes()),
        Body::Complete(Record {
            pid: 77,
            kind: "ref".to_string(),
            facts: linux_facts(),
        })
    );
}

/// The reason the body has a version line and an end line: the file is visible before its body is
/// written, so a reader can catch it cut anywhere. No cut of a new body may read as complete, and none
/// may hand back a pid (`pid=12` of `pid=1234` is not process 12).
#[test]
fn every_cut_of_a_new_body_is_incomplete_and_yields_no_pid() {
    let body = render_body("active", 1234, &linux_facts());
    for cut in 0..body.len() {
        let seen = inspect(
            &body.as_bytes()[..cut],
            &FakeMachine {
                ours: linux_facts(),
                process: ProcessRead::Alive(Some("5000".to_string())),
            },
        );
        assert_eq!(
            parse_body(&body.as_bytes()[..cut]),
            Body::Incomplete,
            "cut at {cut}"
        );
        assert_eq!(seen.pid, None, "cut at {cut}");
        assert_eq!(seen.reason, LivenessReason::IncompleteBody, "cut at {cut}");
        assert_eq!(seen.reason.liveness(), PidLiveness::Unknown, "cut at {cut}");
    }
    assert!(matches!(parse_body(body.as_bytes()), Body::Complete(_)));
}

/// A 0.56.0 body, exactly as `lock_body` wrote it.
const OLD_BODY: &str = "pid=1234\nkind=active\nnote=PR-007 lock has no stale-lock stealing yet\n";

#[test]
fn an_old_body_reads_as_old_and_is_unknown_written_by_an_older_prikk() {
    assert_eq!(
        parse_body(OLD_BODY.as_bytes()),
        Body::Old {
            pid: 1234,
            kind: "active".to_string()
        }
    );
    let seen = inspect(
        OLD_BODY.as_bytes(),
        &FakeMachine {
            ours: linux_facts(),
            process: ProcessRead::Alive(Some("5000".to_string())),
        },
    );
    assert_eq!(seen.pid, Some(1234));
    assert_eq!(seen.reason, LivenessReason::OlderPrikk);
    assert_eq!(seen.reason.liveness(), PidLiveness::Unknown);
}

/// An old body cut anywhere is not an old body: `pid=12` is not process 12.
#[test]
fn every_cut_of_an_old_body_is_incomplete_not_old() {
    for cut in 0..OLD_BODY.len() {
        assert_eq!(
            parse_body(&OLD_BODY.as_bytes()[..cut]),
            Body::Incomplete,
            "cut at {cut}"
        );
    }
}

#[test]
fn garbage_and_empty_bodies_are_incomplete() {
    for body in [
        "",
        "\n",
        "not a lock body at all",
        "prikk-lock-v2\n",
        "pid=x\nkind=y\n",
    ] {
        assert_eq!(parse_body(body.as_bytes()), Body::Incomplete, "{body:?}");
    }
    // A version line and an end line around a body with no pid is damaged, not complete.
    assert_eq!(
        parse_body(b"prikk-lock-v2\nkind=active\nos=linux\nend-of-lock-v2\n"),
        Body::Incomplete
    );
    // A pid that does not parse is damaged, not complete.
    assert_eq!(
        parse_body(b"prikk-lock-v2\npid=12x\nkind=active\nos=linux\nend-of-lock-v2\n"),
        Body::Incomplete
    );
}

// The case table, one assertion per row. `judge` is pure, so every platform's rows run everywhere.

#[test]
fn row_alive_and_the_same_process_appears_running() {
    let reason = judged(
        linux_facts(),
        linux_facts(),
        ProcessRead::Alive(Some("5000".into())),
    );
    assert_eq!(reason, LivenessReason::SameProcess);
    assert_eq!(reason.liveness(), PidLiveness::AppearsRunning);
    let reason = judged(
        windows_facts(),
        windows_facts(),
        ProcessRead::Alive(windows_facts().start),
    );
    assert_eq!(reason.liveness(), PidLiveness::AppearsRunning);
}

#[test]
fn row_the_holder_dead_does_not_appear_running() {
    let reason = judged(linux_facts(), linux_facts(), ProcessRead::Gone);
    assert_eq!(reason, LivenessReason::ProcessGone);
    assert_eq!(reason.liveness(), PidLiveness::DoesNotAppearRunning);
}

#[test]
fn row_the_pid_reused_by_another_process_does_not_appear_running() {
    let reason = judged(
        linux_facts(),
        linux_facts(),
        ProcessRead::Alive(Some("5001".into())),
    );
    assert_eq!(reason, LivenessReason::PidReused);
    assert_eq!(reason.liveness(), PidLiveness::DoesNotAppearRunning);
    let reason = judged(
        windows_facts(),
        windows_facts(),
        ProcessRead::Alive(Some("1".into())),
    );
    assert_eq!(reason, LivenessReason::PidReused);
}

#[test]
fn row_another_pid_namespace_is_unknown_even_when_a_process_has_that_number() {
    let mut recorded = linux_facts();
    recorded.pidns = Some("pid:[4026532999]".to_string());
    // The number is alive here with an equal start time: still unknown, because here is not where
    // the lock was taken.
    let reason = judged(
        recorded,
        linux_facts(),
        ProcessRead::Alive(Some("5000".into())),
    );
    assert_eq!(reason, LivenessReason::OtherNamespace);
    assert_eq!(reason.liveness(), PidLiveness::Unknown);
}

#[test]
fn row_a_reboot_does_not_appear_running_even_when_the_number_and_start_time_match() {
    let mut recorded = linux_facts();
    recorded.boot = Some("boot-OLD".to_string());
    // Start times count ticks since boot and repeat across reboots: an equal one proves nothing.
    let reason = judged(
        recorded,
        linux_facts(),
        ProcessRead::Alive(Some("5000".into())),
    );
    assert_eq!(reason, LivenessReason::Rebooted);
    assert_eq!(reason.liveness(), PidLiveness::DoesNotAppearRunning);
}

#[test]
fn row_a_body_from_another_operating_system_is_unknown() {
    let reason = judged(
        windows_facts(),
        linux_facts(),
        ProcessRead::Alive(Some("5000".into())),
    );
    assert_eq!(reason, LivenessReason::OtherOs);
    assert_eq!(reason.liveness(), PidLiveness::Unknown);
}

#[test]
fn row_an_unreadable_field_is_unknown_never_does_not_appear_running() {
    // The record lacks the start time.
    let mut recorded = linux_facts();
    recorded.start = None;
    let reason = judged(
        recorded,
        linux_facts(),
        ProcessRead::Alive(Some("5000".into())),
    );
    assert_eq!(reason, LivenessReason::FieldUnreadable);
    // The record lacks the boot id, or the namespace (Linux records both).
    for strip in [0, 1] {
        let mut recorded = linux_facts();
        if strip == 0 {
            recorded.boot = None
        } else {
            recorded.pidns = None
        }
        let reason = judged(recorded, linux_facts(), ProcessRead::Gone);
        assert_eq!(reason, LivenessReason::FieldUnreadable, "strip {strip}");
    }
    // This machine cannot read its own boot id or namespace.
    let mut ours = linux_facts();
    ours.boot = None;
    assert_eq!(
        judged(linux_facts(), ours, ProcessRead::Gone),
        LivenessReason::FieldUnreadable
    );
    // The process is alive but its start time cannot be read now.
    assert_eq!(
        judged(linux_facts(), linux_facts(), ProcessRead::Alive(None)),
        LivenessReason::FieldUnreadable
    );
    // The operating system will not say whether the pid exists.
    let reason = judged(linux_facts(), linux_facts(), ProcessRead::Unreadable);
    assert_eq!(reason, LivenessReason::CouldNotCheck);
    assert_eq!(reason.liveness(), PidLiveness::Unknown);
}

// The real machine.

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn this_process_appears_running_from_a_body_it_wrote_about_itself() {
    let facts = own_facts();
    assert!(facts.start.is_some(), "this platform records a start time");
    let body = render_body("active", std::process::id(), facts);
    let seen = inspect(body.as_bytes(), &SystemProbe);
    assert_eq!(seen.reason, LivenessReason::SameProcess);
    assert_eq!(seen.pid, Some(std::process::id()));
    assert_eq!(seen.start, facts.start);
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn this_pid_with_another_start_time_is_a_reused_pid_on_the_real_machine() {
    let mut facts = own_facts().clone();
    facts.start = facts.start.map(|start| format!("{start}0"));
    let body = render_body("active", std::process::id(), &facts);
    let seen = inspect(body.as_bytes(), &SystemProbe);
    assert_eq!(seen.reason, LivenessReason::PidReused);
}

/// A complete body naming pid 0 (no process can be asked about it): unknown, never absent.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn a_complete_body_naming_pid_zero_is_unknown_not_absent() {
    let body = render_body("active", 0, own_facts());
    let seen = inspect(body.as_bytes(), &SystemProbe);
    assert_eq!(seen.reason, LivenessReason::CouldNotCheck);
    assert_eq!(seen.reason.liveness(), PidLiveness::Unknown);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_records_boot_id_and_namespace_and_a_foreign_one_is_unknown_on_the_real_machine() {
    let facts = own_facts();
    assert!(facts.boot.is_some() && facts.pidns.is_some());
    let mut foreign = facts.clone();
    foreign.pidns = Some("pid:[1]".to_string());
    let body = render_body("active", std::process::id(), &foreign);
    assert_eq!(
        inspect(body.as_bytes(), &SystemProbe).reason,
        LivenessReason::OtherNamespace
    );
    let mut rebooted = facts.clone();
    rebooted.boot = Some("00000000-0000-0000-0000-000000000000".to_string());
    let body = render_body("active", std::process::id(), &rebooted);
    assert_eq!(
        inspect(body.as_bytes(), &SystemProbe).reason,
        LivenessReason::Rebooted
    );
}

/// `/proc/<pid>/stat` counts fields from the last `)`: a command name may hold spaces and parens.
#[cfg(target_os = "linux")]
#[test]
fn parse_stat_counts_fields_from_the_last_paren() {
    let line = "42 (a ) (b c) S 1 42 42 0 -1 4194304 100 0 0 0 1 2 0 0 20 0 1 0 987654 1000 200 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0\n";
    assert_eq!(
        super::platform::parse_stat(line),
        Some(('S', "987654".to_string()))
    );
    assert_eq!(super::platform::parse_stat("42 (x) S 1 2"), None);
    assert_eq!(super::platform::parse_stat(""), None);
}

#[cfg(unix)]
mod children {
    use std::process::{Command, Stdio};

    use super::*;

    fn body_naming(pid: u32) -> String {
        let ProcessRead::Alive(Some(start)) = SystemProbe.read(pid) else {
            panic!("the child should be alive with a readable start time");
        };
        let mut facts = own_facts().clone();
        facts.start = Some(start);
        render_body("active", pid, &facts)
    }

    #[test]
    fn a_live_child_appears_running_and_the_same_body_does_not_once_it_is_gone()
    -> std::io::Result<()> {
        let mut child = Command::new("sleep")
            .arg("60")
            .stdin(Stdio::null())
            .spawn()?;
        let body = body_naming(child.id());
        assert_eq!(
            inspect(body.as_bytes(), &SystemProbe).reason,
            LivenessReason::SameProcess
        );
        child.kill()?;
        child.wait()?;
        assert_eq!(
            inspect(body.as_bytes(), &SystemProbe).reason,
            LivenessReason::ProcessGone
        );
        Ok(())
    }

    /// An exited child its parent has not yet reaped is a zombie: `kill(pid, 0)` still succeeds on it,
    /// but it can never run again, so it must not read as a running holder.
    #[test]
    fn an_exited_but_unreaped_child_does_not_appear_running() -> std::io::Result<()> {
        let mut child = Command::new("sleep")
            .arg("60")
            .stdin(Stdio::null())
            .spawn()?;
        let body = body_naming(child.id());
        // Kill by signal without waiting: the child becomes a zombie until `wait`.
        let pid = child.id();
        assert!(
            Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status()?
                .success()
        );
        let mut reason = LivenessReason::SameProcess;
        for _ in 0..200 {
            reason = inspect(body.as_bytes(), &SystemProbe).reason;
            if reason != LivenessReason::SameProcess {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        let _ = child.wait();
        assert_eq!(reason, LivenessReason::ProcessGone);
        Ok(())
    }
}

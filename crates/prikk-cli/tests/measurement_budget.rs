//! Controls for the measurement watcher (`tools/corpus/tests/support/budget.rs`; measurement-budget handoff §4 and §6.1-6.2).
//!
//! The watcher ends the process it runs in, so each control runs a **worker** -- an `#[ignore]`d test of this file -- as a
//! child process (the self-reexec pattern `rfc133_node_count_memory.rs` already uses) and reads what it left on disk. The
//! worker's **budgets are constants of this file**, as an instrument's are; the only thing passed in the environment is
//! where to write the report, which is plumbing, not a knob.
//!
//! Linux-only, like the harnesses that use the watcher (`/proc`, `kill`).

#![cfg(target_os = "linux")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

#[path = "../../../tools/corpus/tests/support/budget.rs"]
mod budget;

/// The worker's declared budget. Small on purpose: the unit is stopped at twice this.
const WORKER_BUDGET: Duration = Duration::from_millis(1_500);

/// Where a worker writes its report (test plumbing).
const REPORT_ENV: &str = "PRIKK_BUDGET_CONTROL_REPORT";

fn worker_report() -> PathBuf {
    PathBuf::from(
        std::env::var(REPORT_ENV).expect("a worker is run by its control, with a report path"),
    )
}

/// Worker for control 1: two quick steps, then one that would run 60 s -- a `sleep` child, so the control can also see that
/// the watcher killed what the unit started.
#[test]
#[ignore = "worker process of measurement_budget's controls; never run directly"]
fn worker_over_budget() {
    let report = worker_report();
    let unit = budget::Unit::begin("control-over-budget", WORKER_BUDGET, &report);
    unit.step("quick one", || {
        std::thread::sleep(Duration::from_millis(150))
    });
    unit.step("quick two", || {
        std::thread::sleep(Duration::from_millis(150))
    });
    unit.step("slow", || {
        let child = Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("spawning sleep");
        std::fs::write(report.with_extension("pid"), child.id().to_string()).unwrap();
        let mut child = child;
        let _ = child.wait();
    });
    // Not reached under a working watcher: the unit is stopped at 3 s, in the `slow` step.
    let _ = unit.finish();
}

/// Worker for control 2: two steps finish, and the third is killed by a **test-only failpoint** (an abort, standing for a
/// power-off: nothing after it runs, no destructor, no `finish`).
#[test]
#[ignore = "worker process of measurement_budget's controls; never run directly"]
fn worker_killed_midway() {
    let report = worker_report();
    let unit = budget::Unit::begin("control-killed-midway", Duration::from_secs(600), &report);
    unit.step("first", || std::thread::sleep(Duration::from_millis(100)));
    unit.step("second", || std::thread::sleep(Duration::from_millis(100)));
    unit.step("third", || std::process::abort());
}

/// Worker for the negative: within budget, it finishes and the watcher never fires.
#[test]
#[ignore = "worker process of measurement_budget's controls; never run directly"]
fn worker_within_budget() {
    let report = worker_report();
    let unit = budget::Unit::begin("control-within-budget", Duration::from_secs(600), &report);
    unit.step("only", || std::thread::sleep(Duration::from_millis(100)));
    let table = unit.finish();
    std::fs::write(report.with_extension("table"), table).unwrap();
}

struct Ran {
    status: Option<i32>,
    report: String,
    pid_file: PathBuf,
    elapsed: Duration,
}

/// Run one worker to the end, or kill it after `wait_at_most` (a watcher that does not fire is the failure this measures).
fn run_worker(worker: &str, dir: &Path, wait_at_most: Duration) -> Ran {
    let report = dir.join(format!("{worker}.md"));
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            worker,
            "--nocapture",
            "--test-threads=1",
        ])
        .env(REPORT_ENV, &report)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawning the worker");
    let began = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status.code();
        }
        if began.elapsed() > wait_at_most {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    Ran {
        status,
        report: std::fs::read_to_string(&report).unwrap_or_default(),
        pid_file: report.with_extension("pid"),
        elapsed: began.elapsed(),
    }
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "prikk-measurement-budget-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// **Control 1 -- the watcher stops a unit.** A unit whose work exceeds twice its budget writes `STOPPED: over budget`
/// (naming the step it was in), keeps the steps it finished, kills what it started, and **fails**.
/// **Perturb:** make the watcher's test `elapsed >= limit` never true (`false`): the worker sleeps its full 60 s, this
/// control's own 20 s cap kills it, and it goes red on `status`.
#[test]
fn a_unit_over_twice_its_budget_stops_itself_writes_the_stop_line_and_fails() {
    let dir = scratch("over");
    let ran = run_worker("worker_over_budget", &dir, Duration::from_secs(20));
    assert_eq!(
        ran.status,
        Some(2),
        "the unit failed by itself (not by the cap): {}",
        ran.report
    );
    assert!(
        ran.elapsed < Duration::from_secs(15),
        "it stopped near 2x the budget, not at the cap: {:?}",
        ran.elapsed
    );
    assert!(
        ran.report.contains("STOPPED: over budget"),
        "{}",
        ran.report
    );
    assert!(
        ran.report.contains("in step `slow`"),
        "names the step it was in: {}",
        ran.report
    );
    assert!(
        ran.report.contains("| quick one |") && ran.report.contains("| quick two |"),
        "the steps finished before the stop stay in the report: {}",
        ran.report
    );
    let child: u32 = std::fs::read_to_string(&ran.pid_file)
        .expect("the worker recorded its child")
        .trim()
        .parse()
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let alive = Path::new(&format!("/proc/{child}")).exists()
        && std::fs::read_to_string(format!("/proc/{child}/stat"))
            .is_ok_and(|stat| !stat.contains(") Z"));
    assert!(
        !alive,
        "the child the unit started was killed (pid {child})"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// **Control 2 -- the partial report survives.** A unit killed midway (a test-only abort, standing for a power-off) leaves
/// every step finished before it in its report, each with elapsed time and boot id, and none for the step in flight.
/// **Perturb:** stop appending a step's row when it ends (write only at `finish`): red.
#[test]
fn a_unit_killed_midway_leaves_every_finished_step_on_disk() {
    let dir = scratch("killed");
    let ran = run_worker("worker_killed_midway", &dir, Duration::from_secs(20));
    assert_ne!(ran.status, Some(0), "the failpoint ended the process");
    let rows: Vec<&str> = ran
        .report
        .lines()
        .filter(|line| {
            line.starts_with("| first |")
                || line.starts_with("| second |")
                || line.starts_with("| third |")
        })
        .collect();
    assert_eq!(
        rows.len(),
        2,
        "exactly the two finished steps: {}",
        ran.report
    );
    let boot = budget::boot_id();
    for row in &rows {
        assert!(
            row.contains(&format!("`{boot}`")),
            "each row carries the boot id: {row}"
        );
        let elapsed: f64 = row.split('|').nth(3).unwrap().trim().parse().unwrap();
        assert!(elapsed >= 0.09, "and an elapsed time: {row}");
    }
    assert!(!ran.report.contains("| third |"), "{}", ran.report);
    assert!(
        !ran.report.contains("STOPPED"),
        "a kill is not the watcher: {}",
        ran.report
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// **The negative:** a unit inside its budget finishes normally, is never stopped, and hands back its step table.
#[test]
fn a_unit_within_its_budget_finishes_and_is_never_stopped() {
    let dir = scratch("within");
    let ran = run_worker("worker_within_budget", &dir, Duration::from_secs(20));
    assert_eq!(ran.status, Some(0), "{}", ran.report);
    assert!(!ran.report.contains("STOPPED"), "{}", ran.report);
    let table = std::fs::read_to_string(dir.join("worker_within_budget.table")).unwrap();
    assert!(
        table.contains("| only |") && table.contains("Budget 600 s"),
        "{table}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// The process reader the watcher kills with: a child and a grandchild are found, and an unrelated process is not.
#[test]
fn descendants_finds_children_and_grandchildren_only() {
    let mut child = Command::new("sh")
        .args(["-c", "sleep 30 & wait"])
        .spawn()
        .expect("spawning sh");
    std::thread::sleep(Duration::from_millis(300));
    let found = budget::descendants(std::process::id());
    assert!(found.contains(&child.id()), "the child: {found:?}");
    assert!(found.len() >= 2, "and its `sleep` grandchild: {found:?}");
    assert!(!found.contains(&std::process::id()), "not itself");
    assert!(!found.contains(&1), "nor init");
    // Clean up only what this control started (other controls' workers may be running beside it).
    for pid in budget::descendants(child.id()) {
        let _ = Command::new("kill")
            .arg("-KILL")
            .arg(pid.to_string())
            .output();
    }
    let _ = child.kill();
    let _ = child.wait();
}

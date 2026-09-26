//! A time budget and a watcher for a measurement unit (measurement-budget handoff, `rfcs/handoffs/133-performance-cost-and-its-evidence/
//! measurement-budget-handoff-v1.md` §4).
//!
//! **Why.** Measurement sessions held the owner's machine for hours across 0.46.0 and 0.47.0 because no unit stated a budget, no step
//! was timed, and nothing stopped one that ran long. A power-off, or a shared machine, then decided what a session cost.
//!
//! **What it is.** A [`Unit`] is one measurement invocation. It **declares its budget in source** (a named constant in the
//! instrument, set from measured times with a stated margin) -- there is no environment variable, argument or file that can
//! raise it, for the reason RFC 133's release-gate profile has none. It then:
//! - writes the unit's **report file at once**, and appends **every step's start, end, elapsed time and the boot id as it
//!   happens**, flushed and synced: a unit stopped for any reason, a power-off included, leaves every finished step on disk;
//! - starts a **watcher** that, at [`OVER_BUDGET_FACTOR`] times the budget, kills every process the unit started, appends
//!   `STOPPED: over budget` (and the step it was in) to the report, and ends the process with a failing status. It does not
//!   keep running silently;
//! - on [`Unit::finish`] hands back the step table, for the instrument to put in its final report, and stops the watcher.
//!
//! **Shared by inclusion.** `tools/corpus/tests/build_cost_curve.rs` and `crates/prikk-cli/tests/rfc133_node_count_memory.rs`
//! (and the controls in `crates/prikk-cli/tests/measurement_budget.rs`) each name this one file with `#[path]`, so the
//! two harnesses cannot drift. It uses only `std`, and reads `/proc` (Linux) for the boot id, the load and the processes to
//! kill; elsewhere those read as `unknown` / nothing.

#![allow(dead_code)]

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// A unit is stopped at this many times its declared budget.
pub const OVER_BUDGET_FACTOR: u32 = 2;

/// The kernel's boot id (`/proc/sys/kernel/random/boot_id`): changes at every boot, so a report can show which steps ran in
/// which boot, and a power-off is visible as a change.
pub fn boot_id() -> String {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map_or_else(|_| "unknown".to_string(), |text| text.trim().to_string())
}

/// The 1-, 5- and 15-minute load averages, as printed by the kernel.
pub fn load_average() -> String {
    std::fs::read_to_string("/proc/loadavg").map_or_else(
        |_| "unknown".to_string(),
        |text| {
            text.split_whitespace()
                .take(3)
                .collect::<Vec<_>>()
                .join(" ")
        },
    )
}

/// One finished step.
#[derive(Debug, Clone)]
pub struct StepRecord {
    pub name: String,
    /// Seconds since the unit began, at the step's start.
    pub started_at: f64,
    pub elapsed: f64,
    pub boot: String,
}

#[derive(Default)]
struct State {
    steps: Vec<StepRecord>,
    /// The step in flight and when it began.
    current: Option<(String, Instant)>,
    finished: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

/// One measurement unit under a budget. See the module documentation.
pub struct Unit {
    name: String,
    budget: Duration,
    began: Instant,
    boot: String,
    report: PathBuf,
    shared: Arc<Shared>,
}

/// Append `text` to the report file and sync it, so what is written survives whatever ends the process next.
fn append_synced(report: &Path, text: &str) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(report)
        .expect("opening the unit's report");
    file.write_all(text.as_bytes())
        .expect("appending to the unit's report");
    file.sync_all().expect("syncing the unit's report");
}

/// The processes that descend from `root` (children, their children, ...), by reading `/proc/<pid>/stat`.
pub fn descendants(root: u32) -> Vec<u32> {
    let mut parent_of = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        // `pid (comm) state ppid ...`; `comm` may hold spaces and parentheses, so split after the last `)`.
        let Some(rest) = stat.rsplit_once(')').map(|(_, rest)| rest) else {
            continue;
        };
        let ppid = rest
            .split_whitespace()
            .nth(1)
            .and_then(|field| field.parse::<u32>().ok());
        if let Some(ppid) = ppid {
            parent_of.push((pid, ppid));
        }
    }
    let mut found = Vec::new();
    let mut frontier = vec![root];
    while let Some(parent) = frontier.pop() {
        for &(pid, ppid) in &parent_of {
            if ppid == parent && !found.contains(&pid) {
                found.push(pid);
                frontier.push(pid);
            }
        }
    }
    found
}

/// Kill (`SIGKILL`) every process below this one. `kill` is the coreutils command: this workspace forbids `unsafe`, so no
/// syscall is made here.
fn kill_descendants() {
    let pids = descendants(std::process::id());
    if pids.is_empty() {
        return;
    }
    let _ = std::process::Command::new("kill")
        .arg("-KILL")
        .args(pids.iter().map(u32::to_string))
        .output();
}

impl Unit {
    /// Begin a unit: write its report file (header, budget, boot id, load) and start the watcher. `budget` must be a named
    /// constant of the instrument, never an input.
    pub fn begin(name: &str, budget: Duration, report: &Path) -> Self {
        if let Some(dir) = report.parent() {
            std::fs::create_dir_all(dir).expect("creating the report directory");
        }
        let boot = boot_id();
        let mut header = String::new();
        let _ = write!(
            header,
            "# {name} -- unit report (in progress)\n\n\
             Budget **{} s**; the unit stops itself at **{} s** ({}x). Boot `{boot}`; load at start {}.\n\n\
             | step | started (s) | elapsed (s) | boot |\n|---|---:|---:|---|\n",
            budget.as_secs_f64(),
            budget.as_secs_f64() * f64::from(OVER_BUDGET_FACTOR),
            OVER_BUDGET_FACTOR,
            load_average()
        );
        std::fs::write(report, &header).expect("writing the unit's report");
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
        });
        let began = Instant::now();
        let unit = Self {
            name: name.to_string(),
            budget,
            began,
            boot,
            report: report.to_path_buf(),
            shared: Arc::clone(&shared),
        };
        let limit = budget * OVER_BUDGET_FACTOR;
        let report_path = report.to_path_buf();
        let unit_name = name.to_string();
        std::thread::spawn(move || {
            let mut state = shared.state.lock().expect("watcher lock");
            loop {
                if state.finished {
                    return;
                }
                let elapsed = began.elapsed();
                if elapsed >= limit {
                    let in_step =
                        state
                            .current
                            .as_ref()
                            .map_or("between steps".to_string(), |(step, at)| {
                                format!(
                                    "in step `{step}` ({:.1} s into it)",
                                    at.elapsed().as_secs_f64()
                                )
                            });
                    kill_descendants();
                    append_synced(
                        &report_path,
                        &format!(
                            "\n**STOPPED: over budget** -- unit `{unit_name}` ran {:.1} s against a budget of {:.1} s \
                             (stopped at {}x), {in_step}. The steps above finished; nothing after them ran.\n",
                            elapsed.as_secs_f64(),
                            budget.as_secs_f64(),
                            OVER_BUDGET_FACTOR
                        ),
                    );
                    eprintln!("STOPPED: over budget -- unit `{unit_name}` ({in_step})");
                    std::process::exit(2);
                }
                let (next, _) = shared
                    .wake
                    .wait_timeout(state, limit - elapsed)
                    .expect("watcher wait");
                state = next;
            }
        });
        unit
    }

    /// Run one step, recording its start, end, elapsed time and the boot id, appended to the report as soon as it ends.
    pub fn step<T>(&self, name: &str, work: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        self.shared.state.lock().expect("state lock").current = Some((name.to_string(), started));
        let value = work();
        let elapsed = started.elapsed().as_secs_f64();
        let record = StepRecord {
            name: name.to_string(),
            started_at: started.duration_since(self.began).as_secs_f64(),
            elapsed,
            boot: boot_id(),
        };
        append_synced(&self.report, &format!("{}\n", table_row(&record)));
        eprintln!("[{}] step `{name}`: {elapsed:.1} s", self.name);
        let mut state = self.shared.state.lock().expect("state lock");
        state.current = None;
        state.steps.push(record);
        value
    }

    /// The unit's budget, as declared.
    pub fn budget(&self) -> Duration {
        self.budget
    }

    /// The boot this unit began in.
    pub fn boot(&self) -> &str {
        &self.boot
    }

    /// Seconds since the unit began.
    pub fn elapsed(&self) -> f64 {
        self.began.elapsed().as_secs_f64()
    }

    /// The steps finished so far.
    pub fn steps(&self) -> Vec<StepRecord> {
        self.shared.state.lock().expect("state lock").steps.clone()
    }

    /// End the unit and stop its watcher. Returns the step table (markdown) for the instrument's final report, which
    /// replaces the in-progress file.
    pub fn finish(self) -> String {
        {
            let mut state = self.shared.state.lock().expect("state lock");
            state.finished = true;
        }
        self.shared.wake.notify_all();
        let steps = self.steps();
        step_table(&self.name, self.budget, &self.boot, self.elapsed(), &steps)
    }
}

fn table_row(record: &StepRecord) -> String {
    format!(
        "| {} | {:.1} | {:.1} | `{}` |",
        record.name, record.started_at, record.elapsed, record.boot
    )
}

/// The step table, with the budget it ran under and its total.
pub fn step_table(
    name: &str,
    budget: Duration,
    boot: &str,
    total: f64,
    steps: &[StepRecord],
) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "## Unit `{name}`: steps\n\nBudget {:.0} s; the unit took **{total:.1} s** ({:.0} % of it). Boot at start `{boot}`.\n\n\
         | step | started (s) | elapsed (s) | boot |\n|---|---:|---:|---|\n",
        budget.as_secs_f64(),
        100.0 * total / budget.as_secs_f64().max(f64::EPSILON)
    );
    for record in steps {
        let _ = writeln!(out, "{}", table_row(record));
    }
    out
}

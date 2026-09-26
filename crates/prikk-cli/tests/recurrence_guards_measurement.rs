//! RFC 160 **G1**: what the whole-read guard (P1) costs the store's test suite -- the same test binary, run to completion with the guard
//! on and with it off (`PRIKK_WHOLE_READ_GUARD=off`, the measurement-only switch), the arms alternating, under the measurement watcher.
//!
//! `PRIKK_G1_STORE_TEST_BINARY` names the `prikk-store` lib test executable (`cargo test -p prikk-store --features test-support --lib
//! --no-run` prints it): it is **built before the unit starts**, so the unit times the suite, not the compiler. Linux-only, like the
//! watcher. Asserts nothing about the ratio: the handoff's acceptance (at most 10 %, or the report says what it costs) is read from the
//! report it writes to `.git-exclude/measurements/rfc160/`.

#![cfg(target_os = "linux")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

#[path = "../../../tools/corpus/tests/support/budget.rs"]
mod budget;

/// The unit's budget (stops itself at twice this).
const G1_BUDGET: Duration = Duration::from_secs(10 * 60);

/// Arm order, alternating and balanced: on, off, off, on, on, off.
const ORDER: [bool; 6] = [true, false, false, true, true, false];

#[test]
#[ignore = "measurement unit G1 (RFC 160 P1's cost to the store suite); run deliberately"]
fn g1_the_whole_read_guards_cost_to_the_store_suite() {
    let binary = std::env::var("PRIKK_G1_STORE_TEST_BINARY")
        .expect("PRIKK_G1_STORE_TEST_BINARY names the prikk-store lib test executable");
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.git-exclude/measurements/rfc160");
    std::fs::create_dir_all(&dir).unwrap();
    let report = dir.join("g1-whole-read-guard-suite-overhead.md");
    let unit = budget::Unit::begin(
        "RFC 160 G1: the guard's cost to the store suite",
        G1_BUDGET,
        &report,
    );
    let load = budget::load_average();
    let mut on = Vec::new();
    let mut off = Vec::new();
    let mut rows = String::from("| run | arm | wall (s) | result line |\n|---|---|---:|---|\n");
    for (index, guard_on) in ORDER.iter().copied().enumerate() {
        let arm = if guard_on { "guard on" } else { "guard off" };
        let (seconds, line) = unit.step(&format!("run {index}: {arm}"), || {
            let mut command = Command::new(&binary);
            if !guard_on {
                command.env("PRIKK_WHOLE_READ_GUARD", "off");
            }
            let began = Instant::now();
            let output = command.output().expect("running the store suite");
            let seconds = began.elapsed().as_secs_f64();
            let text = String::from_utf8_lossy(&output.stdout).to_string();
            let line = text
                .lines()
                .find(|line| line.starts_with("test result:"))
                .unwrap_or("(no result line)")
                .to_string();
            assert!(output.status.success(), "the suite failed ({arm}): {line}");
            (seconds, line)
        });
        rows.push_str(&format!("| {index} | {arm} | {seconds:.2} | {line} |\n"));
        if guard_on {
            on.push(seconds);
        } else {
            off.push(seconds);
        }
    }
    let median = |values: &mut Vec<f64>| {
        values.sort_by(|a, b| a.partial_cmp(b).unwrap());
        values[values.len() / 2]
    };
    let (on_median, off_median) = (median(&mut on.clone()), median(&mut off.clone()));
    let overhead = (on_median / off_median - 1.0) * 100.0;
    let steps = unit.finish();
    let summary = format!(
        "\n## Results (boot `{}`, load at start {load})\n\n{rows}\nMedian with the guard {on_median:.2} s, without {off_median:.2} s: **{overhead:+.1} %** (n = {} per arm; the handoff's acceptance: at most +10 %).\n",
        budget::boot_id(),
        on.len()
    );
    println!("{summary}\n{steps}");
    let mut text = std::fs::read_to_string(&report).unwrap_or_default();
    text.push_str(&summary);
    std::fs::write(&report, text).unwrap();
}

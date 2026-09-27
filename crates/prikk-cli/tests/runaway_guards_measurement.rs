//! RFC 160 §9 **T1**: R3's whole property-test set (every framed reader's termination, outcome-count and work-bound corpus), run as
//! one unit under the measurement watcher. Linux-only, like the watcher; the tests it runs are not.

#![cfg(target_os = "linux")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::process::Command;
use std::time::Duration;

#[path = "../../../tools/corpus/tests/support/budget.rs"]
mod budget;

/// T1's budget (stops itself at twice this).
const T1_BUDGET: Duration = Duration::from_secs(10 * 60);

#[test]
#[ignore = "measurement unit T1 (RFC 160 R3's whole property-test set); run deliberately"]
fn t1_the_runaway_guard_suite_runs_as_one_unit() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.git-exclude/measurements/rfc160");
    std::fs::create_dir_all(&dir).unwrap();
    let report = dir.join("t1-runaway-guards-suite.md");
    let unit = budget::Unit::begin("RFC 160 T1: R3's property-test set", T1_BUDGET, &report);
    let load = budget::load_average();
    let output = unit.step("cargo test -p prikk-store -- runaway_guards", || {
        Command::new(env!("CARGO"))
            .args([
                "test",
                "-p",
                "prikk-store",
                "--locked",
                "--features",
                "test-support",
                "--lib",
                "--",
                "runaway_guards",
            ])
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .output()
            .expect("running the store's runaway-guard suite")
    });
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result_line = text
        .lines()
        .find(|line| line.starts_with("test result:"))
        .unwrap_or("(no result line)")
        .to_string();
    assert!(output.status.success(), "{result_line}\n{text}");
    let steps = unit.finish();
    let summary = format!(
        "\n## Result (boot `{}`, load at start {load})\n\n{result_line}\n",
        budget::boot_id()
    );
    println!("{summary}\n{steps}");
    let mut saved = std::fs::read_to_string(&report).unwrap_or_default();
    saved.push_str(&summary);
    std::fs::write(&report, saved).unwrap();
}

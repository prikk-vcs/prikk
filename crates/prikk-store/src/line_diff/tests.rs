//! The line diff against `diff -u`'s own conventions, an exhaustive reference, and its worst cases.

#![allow(clippy::expect_used, clippy::indexing_slicing)]

use super::{
    LineOp, WORK_BOUND_STEPS, edit_script, edit_script_within, split_lines, unified_hunks,
    unified_hunks_within,
};

/// The hunks alone, for the tests that are about their text and not about the work bound.
fn hunks_of(left: &str, right: &str) -> Vec<String> {
    unified_hunks(left, right).hunks
}

/// SplitMix64: deterministic, dependency-free pseudo-randomness for the property test.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(bound.max(1)).unwrap_or(1)).unwrap_or(0)
    }
}

/// The length of a longest common subsequence, by the O(N·M) dynamic program: the reference the
/// linear-space algorithm is compared against.
fn lcs_length(a: &[&str], b: &[&str]) -> usize {
    let mut table = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            table[i][j] = if a[i] == b[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    table[0][0]
}

/// Replay an edit script over `left`, taking inserted lines from `right`, and refuse a script whose
/// `Equal` steps are not equal lines or that does not consume both sides exactly.
fn replay(left: &[&str], right: &[&str], ops: &[LineOp]) -> Result<Vec<String>, String> {
    let (mut li, mut ri) = (0, 0);
    let mut out = Vec::new();
    for op in ops {
        match op {
            LineOp::Equal => {
                let (l, r) = (left.get(li), right.get(ri));
                if l != r || l.is_none() {
                    return Err(format!("Equal at {li}/{ri} joins {l:?} and {r:?}"));
                }
                out.push(left[li].to_string());
                li += 1;
                ri += 1;
            }
            LineOp::Delete => {
                left.get(li).ok_or("Delete past the left side")?;
                li += 1;
            }
            LineOp::Insert => {
                out.push(
                    right
                        .get(ri)
                        .ok_or("Insert past the right side")?
                        .to_string(),
                );
                ri += 1;
            }
        }
    }
    if li != left.len() || ri != right.len() {
        return Err(format!(
            "consumed {li}/{} and {ri}/{}",
            left.len(),
            right.len()
        ));
    }
    Ok(out)
}

/// An independent mini `patch`: applies rendered hunks to `left` and refuses any context or deleted line
/// that is not really there. Written apart from the renderer so a renderer that lies cannot also lie here.
fn apply_hunks(left: &str, hunks: &[String]) -> Result<String, String> {
    let left_lines = split_lines(left);
    let mut out = String::new();
    let mut taken = 0usize;
    for hunk in hunks {
        let mut lines: Vec<&str> = hunk.split('\n').collect();
        if lines.pop() != Some("") {
            return Err(format!("a hunk must end with a newline: {hunk:?}"));
        }
        let header = lines.first().ok_or("empty hunk")?;
        let rest = header.strip_prefix("@@ -").ok_or("no @@ header")?;
        let (left_range, right_range) = rest.split_once(" +").ok_or("no + range")?;
        let right_range = right_range.strip_suffix(" @@").ok_or("no closing @@")?;
        let left_range = left_range.trim_end();
        let parse_range = |range: &str| -> Result<(usize, usize), String> {
            match range.split_once(',') {
                Some((s, c)) => Ok((
                    s.parse::<usize>().map_err(|e| e.to_string())?,
                    c.parse::<usize>().map_err(|e| e.to_string())?,
                )),
                None => Ok((range.parse::<usize>().map_err(|e| e.to_string())?, 1)),
            }
        };
        let (start, count) = parse_range(left_range)?;
        let (right_start, right_count) = parse_range(right_range)?;
        let before = if count == 0 { start } else { start - 1 };
        while taken < before {
            out.push_str(left_lines.get(taken).ok_or("hunk starts past the end")?);
            taken += 1;
        }
        // The header is a claim about the body and about where the hunk lands on the right; `patch` checks
        // both, so this does too.
        let written_before = out.split_inclusive('\n').count();
        let expected_right_start = if right_count == 0 {
            written_before
        } else {
            written_before + 1
        };
        if right_start != expected_right_start {
            return Err(format!(
                "{header:?}: right side starts at {expected_right_start}, not {right_start}"
            ));
        }
        let (mut seen_left, mut seen_right) = (0usize, 0usize);
        let body = &lines[1..];
        for (index, line) in body.iter().enumerate() {
            if line.starts_with('\\') {
                continue;
            }
            let no_newline = body
                .get(index + 1)
                .is_some_and(|next| next.starts_with("\\ No newline"));
            let marker = line.chars().next().ok_or("blank hunk line")?;
            let text = &line[1..];
            let content = if no_newline {
                text.to_string()
            } else {
                format!("{text}\n")
            };
            match marker {
                ' ' | '-' => {
                    seen_left += 1;
                    seen_right += usize::from(marker == ' ');
                    let have = left_lines.get(taken).ok_or("hunk reads past the end")?;
                    if *have != content {
                        return Err(format!(
                            "line {} is {have:?}, the hunk says {content:?}",
                            taken + 1
                        ));
                    }
                    taken += 1;
                    if marker == ' ' {
                        out.push_str(&content);
                    }
                }
                '+' => {
                    seen_right += 1;
                    out.push_str(&content);
                }
                other => return Err(format!("unknown marker {other:?}")),
            }
        }
        if (seen_left, seen_right) != (count, right_count) {
            return Err(format!(
                "{header:?} counts {count} and {right_count}, the body has {seen_left} and {seen_right}"
            ));
        }
    }
    while let Some(line) = left_lines.get(taken) {
        out.push_str(line);
        taken += 1;
    }
    Ok(out)
}

#[test]
fn identical_texts_have_no_hunks() {
    assert!(hunks_of("", "").is_empty());
    assert!(hunks_of("a\nb\n", "a\nb\n").is_empty());
    assert!(hunks_of("a\nb", "a\nb").is_empty());
}

#[test]
fn one_change_carries_three_lines_of_context_each_way() {
    let left: String = (1..=20).map(|n| format!("line {n}\n")).collect();
    let right = left.replace("line 10\n", "LINE TEN\n");
    assert_eq!(
        hunks_of(&left, &right),
        [
            "@@ -7,7 +7,7 @@\n line 7\n line 8\n line 9\n-line 10\n+LINE TEN\n line 11\n line 12\n line 13\n"
                .to_string()
        ]
    );
}

#[test]
fn a_change_near_the_top_or_bottom_has_less_context() {
    assert_eq!(
        hunks_of("a\nb\nc\nd\ne\n", "X\nb\nc\nd\ne\n"),
        ["@@ -1,4 +1,4 @@\n-a\n+X\n b\n c\n d\n".to_string()]
    );
    assert_eq!(
        hunks_of("a\nb\nc\nd\ne\n", "a\nb\nc\nd\nX\n"),
        ["@@ -2,4 +2,4 @@\n b\n c\n d\n-e\n+X\n".to_string()]
    );
}

#[test]
fn changes_seven_lines_apart_are_two_hunks_and_six_apart_are_one() {
    // 30 numbered lines with lines `at` (1-based) replaced. Built line by line: a textual `replace` of "5\n"
    // would also hit lines 15 and 25.
    let numbered = |changed: &[usize]| -> String {
        (1..=30)
            .map(|n| {
                if changed.contains(&n) {
                    format!("changed {n}\n")
                } else {
                    format!("{n}\n")
                }
            })
            .collect()
    };
    let left = numbered(&[]);
    // Lines 5 and 12: six unchanged lines between them (6..=11), so their contexts touch: one hunk.
    assert_eq!(hunks_of(&left, &numbered(&[5, 12])).len(), 1);
    // Lines 5 and 13: seven between them (6..=12), so their contexts do not touch: two hunks.
    assert_eq!(hunks_of(&left, &numbered(&[5, 13])).len(), 2);
}

#[test]
fn a_whole_new_or_deleted_file_is_one_hunk_from_or_to_line_zero() {
    assert_eq!(
        hunks_of("", "x\ny\n"),
        ["@@ -0,0 +1,2 @@\n+x\n+y\n".to_string()]
    );
    assert_eq!(
        hunks_of("x\ny\n", ""),
        ["@@ -1,2 +0,0 @@\n-x\n-y\n".to_string()]
    );
    assert_eq!(
        hunks_of("", "only\n"),
        ["@@ -0,0 +1 @@\n+only\n".to_string()]
    );
    assert_eq!(
        hunks_of("only\n", ""),
        ["@@ -1 +0,0 @@\n-only\n".to_string()]
    );
}

#[test]
fn a_last_line_without_a_newline_is_marked_as_diff_marks_it() {
    // Gaining a trailing newline is a change, and both forms say which side lacked it.
    assert_eq!(
        hunks_of("a\nb", "a\nb\n"),
        ["@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+b\n".to_string()]
    );
    assert_eq!(
        hunks_of("a\nb\n", "a\nb"),
        ["@@ -1,2 +1,2 @@\n a\n-b\n+b\n\\ No newline at end of file\n".to_string()]
    );
    assert_eq!(
        hunks_of("a\nb", "a\nc"),
        ["@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+c\n\\ No newline at end of file\n"
            .to_string()]
    );
    // An unchanged unterminated last line is context, and keeps its marker.
    assert_eq!(
        hunks_of("x\na\nb", "y\na\nb"),
        ["@@ -1,3 +1,3 @@\n-x\n+y\n a\n b\n\\ No newline at end of file\n".to_string()]
    );
}

#[test]
fn crlf_lines_keep_their_carriage_return() {
    assert_eq!(
        hunks_of("a\r\nb\r\n", "a\r\nB\r\n"),
        ["@@ -1,2 +1,2 @@\n a\r\n-b\r\n+B\r\n".to_string()]
    );
}

#[test]
fn a_moved_block_costs_the_minimum() {
    let left = split_lines("1\n2\n3\n4\n5\n6\n");
    let right = split_lines("4\n5\n6\n1\n2\n3\n");
    let ops = edit_script(&left, &right).ops;
    let changes = ops.iter().filter(|op| **op != LineOp::Equal).count();
    assert_eq!(
        changes, 6,
        "three lines out and three back in is the minimum"
    );
    assert!(replay(&left, &right, &ops).is_ok());
}

/// The property test: on thousands of random small inputs -- including empty sides, one-letter alphabets
/// (everything repeated) and unterminated last lines -- the script is valid, **minimal** (against the
/// exhaustive dynamic program), and the rendered hunks, applied by the independent applier, reproduce the
/// right side byte for byte.
#[test]
fn random_inputs_give_minimal_valid_scripts_and_hunks_that_apply() {
    let mut rng = Rng(0x00C0_FFEE);
    for case in 0..20_000 {
        let alphabet = 1 + rng.below(6);
        let make = |rng: &mut Rng| -> String {
            let lines = rng.below(28);
            let mut text: String = (0..lines)
                .map(|_| format!("{}\n", rng.below(alphabet)))
                .collect();
            if !text.is_empty() && rng.below(4) == 0 {
                text.pop();
            }
            text
        };
        let left = make(&mut rng);
        let right = make(&mut rng);
        let left_lines = split_lines(&left);
        let right_lines = split_lines(&right);

        let script = edit_script(&left_lines, &right_lines);
        assert!(
            script.minimal,
            "case {case}: an input this small is far under the work bound ({} steps)",
            script.steps
        );
        let ops = script.ops;
        let rebuilt = replay(&left_lines, &right_lines, &ops).unwrap_or_else(|why| {
            panic!("case {case}: invalid script for {left:?} -> {right:?}: {why}")
        });
        assert_eq!(rebuilt.concat(), right, "case {case}");

        let edits = ops.iter().filter(|op| **op != LineOp::Equal).count();
        let minimum =
            left_lines.len() + right_lines.len() - 2 * lcs_length(&left_lines, &right_lines);
        assert_eq!(
            edits, minimum,
            "case {case}: not minimal for {left:?} -> {right:?}"
        );

        let hunks = hunks_of(&left, &right);
        let applied = apply_hunks(&left, &hunks)
            .unwrap_or_else(|why| panic!("case {case}: {left:?} -> {right:?}: {why}\n{hunks:?}"));
        assert_eq!(applied, right, "case {case}: hunks {hunks:?}");
        assert_eq!(hunks.is_empty(), left == right, "case {case}");
    }
}

/// A file of `n` numbered lines, and the same lines in reverse order: the shape whose minimal script needs the
/// most search (every line is shared, and every one is out of place).
fn reversed_pair(n: usize) -> (String, String) {
    let left: String = (0..n).map(|i| format!("line {i}\n")).collect();
    let right: String = (0..n).rev().map(|i| format!("line {i}\n")).collect();
    (left, right)
}

fn edits(ops: &[LineOp]) -> usize {
    ops.iter().filter(|op| **op != LineOp::Equal).count()
}

/// RFC 153 §6a C 3-4: **above the bound** the script is non-minimal, **marked** so, and still valid -- applying
/// its hunks reproduces the right side byte for byte, which is the promise that does not bend.
#[test]
fn above_the_bound_the_script_is_larger_marked_and_still_applies() {
    let (left, right) = reversed_pair(200);
    let (left_lines, right_lines) = (split_lines(&left), split_lines(&right));
    let bounded = edit_script_within(&left_lines, &right_lines, 1_000);
    assert!(
        !bounded.minimal,
        "a 1,000-step budget cannot finish this search"
    );
    let rebuilt =
        replay(&left_lines, &right_lines, &bounded.ops).expect("the fallback script is valid");
    assert_eq!(rebuilt.concat(), right);
    // Every line is out of place, so the fallback deletes all 200 and inserts all 200; the minimum is 398.
    assert_eq!(edits(&bounded.ops), 400);

    let rendered = unified_hunks_within(&left, &right, 1_000);
    assert!(!rendered.minimal);
    assert_eq!(
        apply_hunks(&left, &rendered.hunks).expect("the fallback hunks apply"),
        right
    );
}

/// The same input **below** the bound is still minimal -- the property test's minimality assertion, scoped to
/// the inputs that are under it.
#[test]
fn below_the_bound_the_script_is_still_minimal() {
    let (left, right) = reversed_pair(200);
    let (left_lines, right_lines) = (split_lines(&left), split_lines(&right));
    let script = edit_script(&left_lines, &right_lines);
    assert!(script.minimal);
    assert_eq!(
        edits(&script.ops),
        398,
        "two 199-line runs out of place is the minimum"
    );
    assert!(script.steps > 0 && script.steps < WORK_BOUND_STEPS);
}

/// More budget never makes a script worse: `minimal` is monotone in the limit, every script at every limit is
/// valid, and the limit that finishes the search is the same one every time.
#[test]
fn a_larger_budget_never_loses_minimality_and_every_script_is_valid() {
    let (left, right) = reversed_pair(60);
    let (left_lines, right_lines) = (split_lines(&left), split_lines(&right));
    let full = edit_script_within(&left_lines, &right_lines, u64::MAX);
    assert!(full.minimal);
    let mut seen_minimal = false;
    for limit in [
        0,
        1,
        10,
        100,
        500,
        full.steps / 2,
        full.steps,
        full.steps + 1,
        u64::MAX,
    ] {
        let script = edit_script_within(&left_lines, &right_lines, limit);
        let rebuilt = replay(&left_lines, &right_lines, &script.ops).expect("valid at every limit");
        assert_eq!(rebuilt.concat(), right, "limit {limit}");
        if script.minimal {
            seen_minimal = true;
            assert_eq!(edits(&script.ops), edits(&full.ops), "limit {limit}");
        } else {
            assert!(
                !seen_minimal,
                "limit {limit}: minimality was lost by adding budget"
            );
        }
    }
    assert!(seen_minimal);
}

/// The property test, **above** the bound: on thousands of random small inputs with a budget too small for many
/// of them, the script is always valid and its hunks always apply; and whenever it *claims* to be minimal it is
/// (against the exhaustive program). The fallback must actually be exercised, or this proves nothing.
#[test]
fn random_inputs_above_a_small_bound_are_valid_and_honest_about_minimality() {
    let mut rng = Rng(0x0000_B0D0);
    let mut larger = 0;
    for case in 0..20_000 {
        let alphabet = 1 + rng.below(6);
        let make = |rng: &mut Rng| -> String {
            let lines = rng.below(28);
            (0..lines)
                .map(|_| format!("{}\n", rng.below(alphabet)))
                .collect()
        };
        let (left, right) = (make(&mut rng), make(&mut rng));
        let (left_lines, right_lines) = (split_lines(&left), split_lines(&right));
        let limit = rng.below(40) as u64;
        let script = edit_script_within(&left_lines, &right_lines, limit);
        let rebuilt = replay(&left_lines, &right_lines, &script.ops)
            .unwrap_or_else(|why| panic!("case {case}: invalid at limit {limit}: {why}"));
        assert_eq!(rebuilt.concat(), right, "case {case}");
        let minimum =
            left_lines.len() + right_lines.len() - 2 * lcs_length(&left_lines, &right_lines);
        if script.minimal {
            assert_eq!(
                edits(&script.ops),
                minimum,
                "case {case}: claimed minimal at limit {limit}"
            );
        } else {
            assert!(edits(&script.ops) >= minimum, "case {case}");
            larger += 1;
        }
        let rendered = unified_hunks_within(&left, &right, limit);
        assert_eq!(rendered.minimal, script.minimal, "case {case}");
        assert_eq!(
            apply_hunks(&left, &rendered.hunks).unwrap_or_else(|why| panic!("case {case}: {why}")),
            right,
            "case {case}"
        );
    }
    assert!(
        larger > 500,
        "only {larger} of 20,000 cases reached the fallback"
    );
}

/// **Determinism replaces minimality above the bound** (RFC 153 §6a C 5): the same inputs give the same script
/// and the same step count on every run -- and the count is **pinned**, so a bound that read the clock (whose
/// steps would differ from run to run and machine to machine) or changed what a step is could not pass.
#[test]
fn the_bound_is_deterministic_and_counts_work_not_time() {
    let (left, right) = reversed_pair(40);
    let (left_lines, right_lines) = (split_lines(&left), split_lines(&right));
    let first = edit_script_within(&left_lines, &right_lines, 300);
    for _ in 0..20 {
        assert_eq!(edit_script_within(&left_lines, &right_lines, 300), first);
    }
    // Pinned: reversing 40 lines needs this many steps to finish, and a budget of 300 stops the search at the
    // first diagonal that passes it.
    let full = edit_script_within(&left_lines, &right_lines, u64::MAX);
    assert_eq!(
        (full.steps, edits(&full.ops), full.minimal),
        (1642, 78, true)
    );
    assert_eq!(
        (first.steps, edits(&first.ops), first.minimal),
        (301, 80, false)
    );
}

/// The real bound, on a real input: a 7,000-line file reversed needs about 12 million steps *per level* of the
/// search and more than 45 million in all, so [`WORK_BOUND_STEPS`] engages -- and the result is marked, valid and
/// applies. (The unit tests above use a small explicit limit; this one uses the shipped constant.)
#[test]
fn the_shipped_bound_engages_on_a_file_reversed_and_the_result_still_applies() {
    let (left, right) = reversed_pair(7_000);
    let rendered = unified_hunks(&left, &right);
    assert!(
        !rendered.minimal,
        "the shipped bound must engage on this input"
    );
    assert_eq!(
        apply_hunks(&left, &rendered.hunks).expect("the fallback hunks apply"),
        right
    );
    let again = unified_hunks(&left, &right);
    assert_eq!(again, rendered, "and it is the same answer again");
}

/// Worst-case timing and step counts. **Ignored**: run it in a release build,
/// `cargo test -p prikk-store --release --lib line_diff::tests::worst_case_timing -- --ignored --nocapture`.
///
/// Every shape is run **twice**: unbounded (`u64::MAX`), which says what the search really costs and how many
/// steps that is, and at the real [`WORK_BOUND_STEPS`], which says what the bound does with it. The ns-per-step
/// figure is the one the constant is derived from (its doc comment says how).
#[test]
#[ignore = "worst-case timing instrument; run in a release build with --ignored --nocapture"]
fn worst_case_timing() {
    fn time(label: &str, left: &str, right: &str) {
        let (left_lines, right_lines) = (split_lines(left), split_lines(right));
        let start = std::time::Instant::now();
        let full = edit_script_within(&left_lines, &right_lines, u64::MAX);
        let full_elapsed = start.elapsed();
        let start = std::time::Instant::now();
        let bounded = edit_script(&left_lines, &right_lines);
        let bounded_elapsed = start.elapsed();
        let distance = full.ops.iter().filter(|op| **op != LineOp::Equal).count();
        eprintln!(
            "{label}: {} vs {} lines, D = {distance}; unbounded {} steps in {:.1} ms ({:.2} ns/step); \
             at the bound: {:.1} ms, {} steps, minimal = {}, {} edits",
            left_lines.len(),
            right_lines.len(),
            full.steps,
            full_elapsed.as_secs_f64() * 1000.0,
            // Zero steps means the reductions emptied the problem: there is no per-step figure to print.
            if full.steps == 0 {
                f64::NAN
            } else {
                full_elapsed.as_secs_f64() * 1e9 / (full.steps as f64)
            },
            bounded_elapsed.as_secs_f64() * 1000.0,
            bounded.steps,
            bounded.minimal,
            bounded
                .ops
                .iter()
                .filter(|op| **op != LineOp::Equal)
                .count(),
        );
    }
    const MIB: usize = 1024 * 1024;
    // Lines of about 32 bytes, so a MiB is ~32,000 lines; a second family of 8-byte lines makes ~130,000.
    let build = |width: usize, tag: &str| -> String {
        let mut text = String::new();
        let mut n = 0usize;
        while text.len() < MIB {
            let mut line = format!("{tag}{n:08}");
            while line.len() + 1 < width {
                line.push('.');
            }
            line.push('\n');
            text.push_str(&line);
            n += 1;
        }
        text
    };
    for width in [32usize, 8] {
        let left = build(width, "L");
        // Required 1: rewritten line by line -- no line is shared.
        time(
            &format!("REQUIRED rewritten line by line (width {width})"),
            &left,
            &build(width, "R"),
        );
        // Required 2: every other line changed.
        let alternating: String = split_lines(&left)
            .iter()
            .enumerate()
            .map(|(i, line)| {
                if i % 2 == 0 {
                    format!("changed {i}\n")
                } else {
                    (*line).to_string()
                }
            })
            .collect();
        time(
            &format!("REQUIRED every other line changed (width {width})"),
            &left,
            &alternating,
        );
    }
    // Not required: the algorithm's real worst cases, both ~1 MiB.
    let left = build(32, "L");
    let reversed: String = split_lines(&left).iter().rev().copied().collect();
    time("EXTRA the same lines in reverse order", &left, &reversed);
    let mut rng = Rng(7);
    let pool: Vec<String> = (0..50).map(|n| format!("pool line {n:02}\n")).collect();
    let mut random = |lines: usize| -> String {
        (0..lines)
            .map(|_| pool[rng.below(pool.len())].clone())
            .collect()
    };
    let (a, b) = (random(60_000), random(60_000));
    time(
        "EXTRA two random files drawn from 50 distinct lines",
        &a,
        &b,
    );
    for alphabet in [5usize, 500] {
        let pool: Vec<String> = (0..alphabet)
            .map(|n| format!("pool line {n:03}\n"))
            .collect();
        let mut rng = Rng(11 + alphabet as u64);
        let mut random = |lines: usize| -> String {
            (0..lines)
                .map(|_| pool[rng.below(pool.len())].clone())
                .collect()
        };
        let (a, b) = (random(30_000), random(30_000));
        time(
            &format!("EXTRA two random files drawn from {alphabet} distinct lines"),
            &a,
            &b,
        );
    }
    let blocks: Vec<&str> = split_lines(&left);
    let block_reversed: String = blocks
        .chunks(200)
        .rev()
        .flat_map(|chunk| chunk.iter().copied())
        .collect();
    time(
        "EXTRA blocks of 200 lines in reverse order",
        &left,
        &block_reversed,
    );
    let shifted: String = split_lines(&left)
        .iter()
        .skip(1)
        .copied()
        .collect::<String>()
        + "L99999999.\n";
    time(
        "EXTRA one line dropped from the top, one added at the end",
        &left,
        &shifted,
    );
}

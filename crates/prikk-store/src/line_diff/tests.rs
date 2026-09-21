//! The line diff against `diff -u`'s own conventions, an exhaustive reference, and its worst cases.

#![allow(clippy::indexing_slicing)]

use super::{LineOp, edit_script, split_lines, unified_hunks};

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
    assert!(unified_hunks("", "").is_empty());
    assert!(unified_hunks("a\nb\n", "a\nb\n").is_empty());
    assert!(unified_hunks("a\nb", "a\nb").is_empty());
}

#[test]
fn one_change_carries_three_lines_of_context_each_way() {
    let left: String = (1..=20).map(|n| format!("line {n}\n")).collect();
    let right = left.replace("line 10\n", "LINE TEN\n");
    assert_eq!(
        unified_hunks(&left, &right),
        [
            "@@ -7,7 +7,7 @@\n line 7\n line 8\n line 9\n-line 10\n+LINE TEN\n line 11\n line 12\n line 13\n"
                .to_string()
        ]
    );
}

#[test]
fn a_change_near_the_top_or_bottom_has_less_context() {
    assert_eq!(
        unified_hunks("a\nb\nc\nd\ne\n", "X\nb\nc\nd\ne\n"),
        ["@@ -1,4 +1,4 @@\n-a\n+X\n b\n c\n d\n".to_string()]
    );
    assert_eq!(
        unified_hunks("a\nb\nc\nd\ne\n", "a\nb\nc\nd\nX\n"),
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
    assert_eq!(unified_hunks(&left, &numbered(&[5, 12])).len(), 1);
    // Lines 5 and 13: seven between them (6..=12), so their contexts do not touch: two hunks.
    assert_eq!(unified_hunks(&left, &numbered(&[5, 13])).len(), 2);
}

#[test]
fn a_whole_new_or_deleted_file_is_one_hunk_from_or_to_line_zero() {
    assert_eq!(
        unified_hunks("", "x\ny\n"),
        ["@@ -0,0 +1,2 @@\n+x\n+y\n".to_string()]
    );
    assert_eq!(
        unified_hunks("x\ny\n", ""),
        ["@@ -1,2 +0,0 @@\n-x\n-y\n".to_string()]
    );
    assert_eq!(
        unified_hunks("", "only\n"),
        ["@@ -0,0 +1 @@\n+only\n".to_string()]
    );
    assert_eq!(
        unified_hunks("only\n", ""),
        ["@@ -1 +0,0 @@\n-only\n".to_string()]
    );
}

#[test]
fn a_last_line_without_a_newline_is_marked_as_diff_marks_it() {
    // Gaining a trailing newline is a change, and both forms say which side lacked it.
    assert_eq!(
        unified_hunks("a\nb", "a\nb\n"),
        ["@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+b\n".to_string()]
    );
    assert_eq!(
        unified_hunks("a\nb\n", "a\nb"),
        ["@@ -1,2 +1,2 @@\n a\n-b\n+b\n\\ No newline at end of file\n".to_string()]
    );
    assert_eq!(
        unified_hunks("a\nb", "a\nc"),
        ["@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+c\n\\ No newline at end of file\n"
            .to_string()]
    );
    // An unchanged unterminated last line is context, and keeps its marker.
    assert_eq!(
        unified_hunks("x\na\nb", "y\na\nb"),
        ["@@ -1,3 +1,3 @@\n-x\n+y\n a\n b\n\\ No newline at end of file\n".to_string()]
    );
}

#[test]
fn crlf_lines_keep_their_carriage_return() {
    assert_eq!(
        unified_hunks("a\r\nb\r\n", "a\r\nB\r\n"),
        ["@@ -1,2 +1,2 @@\n a\r\n-b\r\n+B\r\n".to_string()]
    );
}

#[test]
fn a_moved_block_costs_the_minimum() {
    let left = split_lines("1\n2\n3\n4\n5\n6\n");
    let right = split_lines("4\n5\n6\n1\n2\n3\n");
    let ops = edit_script(&left, &right);
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

        let ops = edit_script(&left_lines, &right_lines);
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

        let hunks = unified_hunks(&left, &right);
        let applied = apply_hunks(&left, &hunks)
            .unwrap_or_else(|why| panic!("case {case}: {left:?} -> {right:?}: {why}\n{hunks:?}"));
        assert_eq!(applied, right, "case {case}: hunks {hunks:?}");
        assert_eq!(hunks.is_empty(), left == right, "case {case}");
    }
}

/// Worst-case timing, in the shape the Stage 1 handoff asks for. **Ignored**: run it in a release build,
/// `cargo test -p prikk-store --release --lib line_diff::tests::worst_case_timing -- --ignored --nocapture`.
///
/// The two required cases are a 1 MiB text file rewritten line by line, and one with every other line
/// changed. The rest are **not required** and are here so the report can say honestly where the algorithm's
/// cost really is.
#[test]
#[ignore = "worst-case timing instrument; run in a release build with --ignored --nocapture"]
fn worst_case_timing() {
    fn time(label: &str, left: &str, right: &str) {
        let (left_lines, right_lines) = (split_lines(left), split_lines(right));
        let start = std::time::Instant::now();
        let hunks = unified_hunks(left, right);
        let elapsed = start.elapsed();
        // The edit distance D, from a second run outside the timed region: the cost model is O((N+M)·D).
        let distance = edit_script(&left_lines, &right_lines)
            .iter()
            .filter(|op| **op != LineOp::Equal)
            .count();
        let work = (left_lines.len() + right_lines.len()) as f64 * distance as f64;
        eprintln!(
            "{label}: {} lines vs {} lines, D = {distance}, (N+M)*D = {work:.2e}, {} hunks, {:.1} ms ({:.2} ns per unit)",
            left_lines.len(),
            right_lines.len(),
            hunks.len(),
            elapsed.as_secs_f64() * 1000.0,
            elapsed.as_secs_f64() * 1e9 / work.max(1.0)
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

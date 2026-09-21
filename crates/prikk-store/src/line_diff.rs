//! A line diff (RFC 153 §3, §6a C): a shortest edit script over lines **within a documented work bound**, and the
//! unified hunks that render it.
//!
//! **Pure and dependency-free.** It reads no repository, calls no external tool and pulls in no crate: two
//! texts in, a script or rendered hunks out, and the same answer every time. It sits in the store's lower
//! layer for that reason -- nothing above it is needed to test it, and `prikk diff` (the upper layer's
//! `diff` module) is one caller among any.
//!
//! **What it computes.** Myers' O(ND) shortest edit script, in the linear-space form (the "middle snake",
//! §4b of the paper), so memory is O(N + M) however large D is. Two exact, minimality-preserving
//! reductions run first, and they are what make the common cases cheap:
//!
//! - the common **prefix and suffix** are trimmed (at every recursion level too);
//! - a line that occurs on **only one side** can never be part of a longest common subsequence, so it is set
//!   aside and re-inserted as a deletion or an insertion afterwards. A file rewritten line by line shares no
//!   line with its old self, and this turns that case from quadratic into linear.
//!
//! **The work bound (RFC 153 §6a C).** The search is still O((N+M)·D) in the worst case: two long files that
//! share many lines in very different orders (a file reversed, say) cost time proportional to their lines times
//! their edit distance, and measured that reached 5 s to 28 s on ordinary shapes. So the search is given a budget,
//! [`WORK_BOUND_STEPS`], and it is a count of **work actually done** -- diagonals visited plus matching lines
//! followed inside the middle-snake loop -- never wall-clock (not reproducible across machines) and never the
//! nominal `(N+M)·D` (wrong once the reductions below have run). One budget covers the whole script, spent in the
//! deterministic order the recursion visits regions.
//!
//! Above the bound the search **stops and the region it had not resolved becomes a valid, non-minimal script**:
//! delete the remaining left lines, insert the remaining right ones. Applying the hunks still reproduces the right
//! side byte for byte -- that promise does not bend -- and the result says so through `minimal: false`, so a
//! reader is never left to guess whether it got the shortest script. **Determinism replaces minimality above the
//! bound**: the count depends only on the two inputs, so the same inputs give the same script on every run and
//! every machine.
//!
//! **Line identity includes the terminator.** A line is compared with its own `\n` (or the lack of one), so
//! a last line that gains a trailing newline is a change, exactly as `diff -u` treats it, and `patch(1)`
//! round-trips the result.

use std::collections::HashMap;

/// Lines of unchanged context around each change (RFC 153 §3, handoff Stage 1).
pub(crate) const CONTEXT_LINES: usize = 3;

/// The work bound: **search steps** allowed to one edit script (RFC 153 §6a C). A step is one diagonal visited in
/// the middle-snake loop plus one line followed along a snake, so it counts work done -- not time, and not the
/// nominal `(N+M)·D`, which the reductions make wrong.
///
/// **How the value was chosen** (§6a C 2 leaves the derivation to the increment): the worst measured shape must
/// stay well under a second on the *slower* of the two machines measured -- `5.0 s` there against `2.3 s` here for
/// the same input, a factor of 2.17 (the second shape gave 1.93), taken as 2.2. The timing instrument
/// (`worst_case_timing`, `--ignored`, release) ran each shape unbounded and measured **2.3 to 5.0 ns per step**
/// here; the worst was two files over a 5-line alphabet, at 5.04 ns, because long snakes are cheap to count and
/// costly to touch. On the slower machine that is 5.04 × 2.2 = **11.1 ns per step**, and half a second at that
/// rate is 0.5 / 11.1e-9 = 45.1 million steps, so the constant is **45,000,000**. The same instrument, run at the
/// bound, is what `report §2` cites for what it does to each shape.
pub(crate) const WORK_BOUND_STEPS: u64 = 45_000_000;

/// An edit script, and whether the search that produced it finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditScript {
    /// The steps, first line to last.
    pub(crate) ops: Vec<LineOp>,
    /// `true` when the script is a shortest one; `false` when the work bound engaged and some region of it is
    /// the valid-but-larger "delete the rest, insert the rest" fallback.
    pub(crate) minimal: bool,
    /// Search steps spent, by the count [`WORK_BOUND_STEPS`] bounds. Never above the bound by more than the last
    /// diagonal's snake, since the bound is checked after every diagonal.
    pub(crate) steps: u64,
}

/// Unified hunks, and whether the script they render is a shortest one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnifiedHunks {
    /// Each hunk as text starting at its `@@` line and ending with a newline.
    pub(crate) hunks: Vec<String>,
    /// See [`EditScript::minimal`].
    pub(crate) minimal: bool,
}

/// Work spent and allowed, shared by every region one script's recursion searches.
struct Budget {
    spent: u64,
    limit: u64,
    /// Some region was left unresolved because the bound engaged.
    exhausted: bool,
}

/// One step of an edit script over lines, in order from the first line to the last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineOp {
    /// The next line of each side is the same line.
    Equal,
    /// The next left line is not in the right side.
    Delete,
    /// The next right line is not in the left side.
    Insert,
}

/// Split `text` into lines, each **keeping its own terminator**. Only the last line can lack one.
pub(crate) fn split_lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// A checked view of the diagonal arrays of Myers' algorithm: reads outside the allocation are zero and
/// writes outside it are dropped, never a panic (`indexing_slicing` is denied in this workspace). The tests
/// compare the result against an exhaustive dynamic-programming diff, which is what would catch a wrong
/// index that this quiet fallback would otherwise hide.
struct Diagonals {
    values: Vec<isize>,
    offset: isize,
}

impl Diagonals {
    fn new(max_d: isize) -> Self {
        let size = usize::try_from(max_d.saturating_mul(2).saturating_add(3)).unwrap_or(0);
        Self {
            values: vec![0; size],
            offset: max_d.saturating_add(1),
        }
    }

    fn slot(&self, k: isize) -> Option<usize> {
        usize::try_from(k.checked_add(self.offset)?).ok()
    }

    fn get(&self, k: isize) -> isize {
        self.slot(k)
            .and_then(|index| self.values.get(index))
            .copied()
            .unwrap_or(0)
    }

    fn set(&mut self, k: isize, x: isize) {
        if let Some(slot) = self.slot(k).and_then(|index| self.values.get_mut(index)) {
            *slot = x;
        }
    }
}

fn same(a: &[usize], x: isize, b: &[usize], y: isize) -> bool {
    match (usize::try_from(x), usize::try_from(y)) {
        (Ok(x), Ok(y)) => matches!((a.get(x), b.get(y)), (Some(p), Some(q)) if p == q),
        _ => false,
    }
}

/// The middle snake of an optimal path between `a` and `b` (Myers 1986, §4b): the run of matches
/// `(x, y) .. (u, v)` in the middle of a shortest edit script, in forward coordinates. Both sequences are
/// non-empty and share no first or last element, so the edit distance is at least 2 and both halves the
/// caller recurses on are strictly smaller.
fn middle_snake(a: &[usize], b: &[usize], budget: &mut Budget) -> Snake {
    let (Ok(n), Ok(m)) = (isize::try_from(a.len()), isize::try_from(b.len())) else {
        return Snake::Unresolved;
    };
    let delta = n - m;
    let odd = delta % 2 != 0;
    let max = (n + m + 1) / 2;
    let mut forward = Diagonals::new(max);
    let mut backward = Diagonals::new(max);
    forward.set(1, 0);
    backward.set(1, 0);
    for d in 0..=max {
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && forward.get(k - 1) < forward.get(k + 1)) {
                forward.get(k + 1)
            } else {
                forward.get(k - 1) + 1
            };
            let mut y = x - k;
            let (start_x, start_y) = (x, y);
            while x < n && y < m && same(a, x, b, y) {
                x += 1;
                y += 1;
            }
            forward.set(k, x);
            budget.spent = budget
                .spent
                .saturating_add(1 + u64::try_from(x - start_x).unwrap_or(0));
            if odd {
                let reverse_k = delta - k;
                if reverse_k > -d && reverse_k < d && forward.get(k) + backward.get(reverse_k) >= n
                {
                    return snake_from(start_x, start_y, x, y);
                }
            }
            if budget.spent > budget.limit {
                return Snake::Unresolved;
            }
            k += 2;
        }
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && backward.get(k - 1) < backward.get(k + 1)) {
                backward.get(k + 1)
            } else {
                backward.get(k - 1) + 1
            };
            let mut y = x - k;
            let (start_x, start_y) = (x, y);
            while x < n && y < m && same(a, n - 1 - x, b, m - 1 - y) {
                x += 1;
                y += 1;
            }
            backward.set(k, x);
            budget.spent = budget
                .spent
                .saturating_add(1 + u64::try_from(x - start_x).unwrap_or(0));
            if !odd {
                let forward_k = delta - k;
                if forward_k >= -d
                    && forward_k <= d
                    && backward.get(k) + forward.get(forward_k) >= n
                {
                    return snake_from(n - x, m - y, n - start_x, m - start_y);
                }
            }
            if budget.spent > budget.limit {
                return Snake::Unresolved;
            }
            k += 2;
        }
    }
    Snake::Unresolved
}

/// What one middle-snake search found.
enum Snake {
    /// The run of matches `(x, y) .. (u, v)` in the middle of a shortest script.
    Found(usize, usize, usize, usize),
    /// The search did not finish: the work bound engaged (or, which cannot happen, no snake was found). The
    /// caller falls back to deleting and inserting the whole region.
    Unresolved,
}

fn snake_from(x: isize, y: isize, u: isize, v: isize) -> Snake {
    match (
        usize::try_from(x),
        usize::try_from(y),
        usize::try_from(u),
        usize::try_from(v),
    ) {
        (Ok(x), Ok(y), Ok(u), Ok(v)) => Snake::Found(x, y, u, v),
        _ => Snake::Unresolved,
    }
}

/// A longest common subsequence of `a` and `b`, as `(index in a, index in b)` pairs in increasing order,
/// with `a_offset`/`b_offset` added so recursion reports positions in the caller's sequences.
fn common_pairs(
    a: &[usize],
    b: &[usize],
    a_offset: usize,
    b_offset: usize,
    budget: &mut Budget,
    out: &mut Vec<(usize, usize)>,
) {
    let mut prefix = 0;
    while matches!((a.get(prefix), b.get(prefix)), (Some(p), Some(q)) if p == q) {
        out.push((a_offset + prefix, b_offset + prefix));
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < a.len() - prefix
        && suffix < b.len() - prefix
        && matches!(
            (a.get(a.len() - 1 - suffix), b.get(b.len() - 1 - suffix)),
            (Some(p), Some(q)) if p == q
        )
    {
        suffix += 1;
    }
    let middle_a = a.get(prefix..a.len() - suffix).unwrap_or_default();
    let middle_b = b.get(prefix..b.len() - suffix).unwrap_or_default();
    if !middle_a.is_empty() && !middle_b.is_empty() {
        // A region with lines on both sides and no common ends needs a search. Once the bound has engaged
        // nothing more is searched: the region stays unresolved, which the fallback renders as a plain
        // delete-and-insert. That is decided by the count alone, so it is the same on every run.
        let found = if budget.exhausted {
            Snake::Unresolved
        } else {
            middle_snake(middle_a, middle_b, budget)
        };
        match found {
            Snake::Found(x, y, u, v) => {
                common_pairs(
                    middle_a.get(..x).unwrap_or_default(),
                    middle_b.get(..y).unwrap_or_default(),
                    a_offset + prefix,
                    b_offset + prefix,
                    budget,
                    out,
                );
                for step in 0..u.saturating_sub(x) {
                    out.push((a_offset + prefix + x + step, b_offset + prefix + y + step));
                }
                common_pairs(
                    middle_a.get(u..).unwrap_or_default(),
                    middle_b.get(v..).unwrap_or_default(),
                    a_offset + prefix + u,
                    b_offset + prefix + v,
                    budget,
                    out,
                );
            }
            Snake::Unresolved => budget.exhausted = true,
        }
    }
    for step in 0..suffix {
        out.push((
            a_offset + a.len() - suffix + step,
            b_offset + b.len() - suffix + step,
        ));
    }
}

/// An edit script turning `left` into `right` within [`WORK_BOUND_STEPS`]: the fewest `Delete`s plus `Insert`s
/// while the search finishes (`minimal`), a valid larger one when it does not, and deterministic either way. Where
/// a run of deletions and insertions sits between two equal lines, every deletion comes before every insertion,
/// as `diff -u` prints them.
#[cfg(test)]
pub(crate) fn edit_script<'a>(left: &[&'a str], right: &[&'a str]) -> EditScript {
    edit_script_within(left, right, WORK_BOUND_STEPS)
}

/// [`edit_script`] with an explicit `limit`, so a test can put a small input above the bound.
pub(crate) fn edit_script_within<'a>(
    left: &[&'a str],
    right: &[&'a str],
    limit: u64,
) -> EditScript {
    let mut ids: HashMap<&'a str, usize> = HashMap::new();
    let mut intern = |line: &&'a str| {
        let next = ids.len();
        *ids.entry(*line).or_insert(next)
    };
    let left_ids: Vec<usize> = left.iter().map(&mut intern).collect();
    let right_ids: Vec<usize> = right.iter().map(&mut intern).collect();

    let mut prefix = 0;
    while matches!((left_ids.get(prefix), right_ids.get(prefix)), (Some(p), Some(q)) if p == q) {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < left_ids.len() - prefix
        && suffix < right_ids.len() - prefix
        && matches!(
            (
                left_ids.get(left_ids.len() - 1 - suffix),
                right_ids.get(right_ids.len() - 1 - suffix)
            ),
            (Some(p), Some(q)) if p == q
        )
    {
        suffix += 1;
    }
    let left_end = left_ids.len() - suffix;
    let right_end = right_ids.len() - suffix;

    // A line on one side only cannot be in a longest common subsequence: set it aside.
    let mut in_left = vec![false; ids.len()];
    let mut in_right = vec![false; ids.len()];
    for &id in left_ids.get(prefix..left_end).unwrap_or_default() {
        if let Some(seen) = in_left.get_mut(id) {
            *seen = true;
        }
    }
    for &id in right_ids.get(prefix..right_end).unwrap_or_default() {
        if let Some(seen) = in_right.get_mut(id) {
            *seen = true;
        }
    }
    let shared = |id: usize| {
        in_left.get(id).copied().unwrap_or(false) && in_right.get(id).copied().unwrap_or(false)
    };
    let kept_left: Vec<usize> = (prefix..left_end)
        .filter(|&index| left_ids.get(index).is_some_and(|&id| shared(id)))
        .collect();
    let kept_right: Vec<usize> = (prefix..right_end)
        .filter(|&index| right_ids.get(index).is_some_and(|&id| shared(id)))
        .collect();
    let reduced_left: Vec<usize> = kept_left
        .iter()
        .filter_map(|&index| left_ids.get(index).copied())
        .collect();
    let reduced_right: Vec<usize> = kept_right
        .iter()
        .filter_map(|&index| right_ids.get(index).copied())
        .collect();

    let mut pairs = Vec::new();
    let mut budget = Budget {
        spent: 0,
        limit,
        exhausted: false,
    };
    common_pairs(&reduced_left, &reduced_right, 0, 0, &mut budget, &mut pairs);

    let mut ops = Vec::with_capacity(left.len() + right.len());
    ops.extend(std::iter::repeat_n(LineOp::Equal, prefix));
    let (mut next_left, mut next_right) = (prefix, prefix);
    for (reduced_a, reduced_b) in pairs {
        let (Some(&a), Some(&b)) = (kept_left.get(reduced_a), kept_right.get(reduced_b)) else {
            continue;
        };
        ops.extend(std::iter::repeat_n(LineOp::Delete, a - next_left));
        ops.extend(std::iter::repeat_n(LineOp::Insert, b - next_right));
        ops.push(LineOp::Equal);
        next_left = a + 1;
        next_right = b + 1;
    }
    ops.extend(std::iter::repeat_n(LineOp::Delete, left_end - next_left));
    ops.extend(std::iter::repeat_n(LineOp::Insert, right_end - next_right));
    ops.extend(std::iter::repeat_n(LineOp::Equal, suffix));
    EditScript {
        ops,
        minimal: !budget.exhausted,
        steps: budget.spent,
    }
}

/// Where a hunk header says a range starts and how long it is, in `diff -u`'s convention: a count of one
/// prints no `,count`, and an **empty** range starts at the line *before* it (so an insertion at the top of
/// a file is `-0,0`).
fn range(start: usize, count: usize) -> String {
    match count {
        0 => format!("{start},0"),
        1 => format!("{}", start + 1),
        _ => format!("{},{count}", start + 1),
    }
}

/// Push one line of a hunk with its marker, and `diff -u`'s note when it lacks its terminator.
fn push_line(body: &mut String, marker: char, line: &str) {
    body.push(marker);
    body.push_str(line);
    if !line.ends_with('\n') {
        body.push_str("\n\\ No newline at end of file\n");
    }
}

/// The unified hunks that turn `left` into `right`, each rendered as text starting at its `@@` line and
/// ending with a newline: **[`CONTEXT_LINES`] of context**, hunks merged when their context would touch,
/// and no hunks at all when the texts are identical. `minimal` says whether the script they render is a shortest
/// one (see the module doc).
pub(crate) fn unified_hunks(left: &str, right: &str) -> UnifiedHunks {
    unified_hunks_within(left, right, WORK_BOUND_STEPS)
}

/// [`unified_hunks`] with an explicit work `limit` (see [`edit_script_within`]).
pub(crate) fn unified_hunks_within(left: &str, right: &str, limit: u64) -> UnifiedHunks {
    let left_lines = split_lines(left);
    let right_lines = split_lines(right);
    let EditScript { ops, minimal, .. } = edit_script_within(&left_lines, &right_lines, limit);
    let changes: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| **op != LineOp::Equal)
        .map(|(index, _)| index)
        .collect();

    // Group changes whose contexts would touch or overlap: at most 2 * context equal lines apart.
    let mut groups: Vec<(usize, usize)> = Vec::new();
    for &change in &changes {
        match groups.last_mut() {
            Some((_, last)) if change - *last - 1 <= 2 * CONTEXT_LINES => *last = change,
            _ => groups.push((change, change)),
        }
    }

    let mut hunks = Vec::with_capacity(groups.len());
    let (mut op_index, mut left_index, mut right_index) = (0usize, 0usize, 0usize);
    for (first, last) in groups {
        let start = first.saturating_sub(CONTEXT_LINES);
        let end = (last + 1 + CONTEXT_LINES).min(ops.len());
        while op_index < start {
            match ops.get(op_index) {
                Some(LineOp::Equal) => {
                    left_index += 1;
                    right_index += 1;
                }
                Some(LineOp::Delete) => left_index += 1,
                Some(LineOp::Insert) => right_index += 1,
                None => break,
            }
            op_index += 1;
        }
        let (hunk_left_start, hunk_right_start) = (left_index, right_index);
        let (mut left_count, mut right_count) = (0usize, 0usize);
        let mut body = String::new();
        for op in ops.get(start..end).unwrap_or_default() {
            match op {
                LineOp::Equal => {
                    if let Some(line) = left_lines.get(left_index) {
                        push_line(&mut body, ' ', line);
                    }
                    left_index += 1;
                    right_index += 1;
                    left_count += 1;
                    right_count += 1;
                }
                LineOp::Delete => {
                    if let Some(line) = left_lines.get(left_index) {
                        push_line(&mut body, '-', line);
                    }
                    left_index += 1;
                    left_count += 1;
                }
                LineOp::Insert => {
                    if let Some(line) = right_lines.get(right_index) {
                        push_line(&mut body, '+', line);
                    }
                    right_index += 1;
                    right_count += 1;
                }
            }
        }
        op_index = end;
        hunks.push(format!(
            "@@ -{} +{} @@\n{body}",
            range(hunk_left_start, left_count),
            range(hunk_right_start, right_count)
        ));
    }
    UnifiedHunks { hunks, minimal }
}

#[cfg(test)]
mod tests;

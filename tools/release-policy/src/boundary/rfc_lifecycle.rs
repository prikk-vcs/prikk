//! RFC 120 §9.4a: the RFC-lifecycle gate. *An RFC in `rfcs/proposed/` must not have a directory in
//! `rfcs/handoffs/`* -- a handoff is written for accepted work, so a proposed RFC that has one was
//! accepted and never moved. Measured 2026-09-13: RFC 133 and RFC 136 had sat in `proposed/` that way,
//! both accepted by the owner, until the day this rule was ruled.
//!
//! **Matches on the full number token, never a prefix.** `133` must not match a `13-*` directory and
//! `DC-43` must not match `DC-10`: the token is the leading digits (or `DC-` and its digits) up to the
//! first `-` or `.` after them, and two names collide only when their tokens are equal.
//!
//! **Checks presence only.** Whether a handoff is live, finished, or superseded is not read -- a
//! handoff directory is the evidence of acceptance this rule binds, the same presence-not-truth
//! posture as `open_work_index.rs`.

use std::path::Path;

use super::{BoundaryError, push};

const PROPOSED_DIR: &str = "rfcs/proposed";
const HANDOFFS_DIR: &str = "rfcs/handoffs";

pub(super) fn check(root: &Path, errors: &mut Vec<BoundaryError>) {
    let Some(proposed) = entry_names(root, PROPOSED_DIR, EntryKind::File, errors) else {
        return;
    };
    let Some(handoffs) = entry_names(root, HANDOFFS_DIR, EntryKind::Directory, errors) else {
        return;
    };
    for file in &proposed {
        let Some(token) = number_token(file) else {
            continue;
        };
        for directory in &handoffs {
            if number_token(directory).as_deref() == Some(token.as_str()) {
                push(
                    errors,
                    "rfc-lifecycle",
                    format!(
                        "{PROPOSED_DIR}/{file} is proposed but has a handoff directory \
                         {HANDOFFS_DIR}/{directory}"
                    ),
                );
            }
        }
    }
}

#[derive(Clone, Copy)]
enum EntryKind {
    File,
    Directory,
}

/// The sorted names of `location`'s entries of `kind`, or `None` after reporting why the directory
/// could not be read.
fn entry_names(
    root: &Path,
    location: &str,
    kind: EntryKind,
    errors: &mut Vec<BoundaryError>,
) -> Option<Vec<String>> {
    let Ok(read_dir) = std::fs::read_dir(root.join(location)) else {
        push(
            errors,
            "rfc-lifecycle",
            format!("{location}: directory unreadable"),
        );
        return None;
    };
    let mut names = Vec::new();
    for entry in read_dir {
        let Ok(entry) = entry else {
            push(
                errors,
                "rfc-lifecycle",
                format!("{location}: entry unreadable"),
            );
            continue;
        };
        let Ok(file_type) = entry.file_type() else {
            push(
                errors,
                "rfc-lifecycle",
                format!("{location}: entry type unreadable"),
            );
            continue;
        };
        let kind_matches = match kind {
            EntryKind::File => file_type.is_file(),
            EntryKind::Directory => file_type.is_dir(),
        };
        if kind_matches {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Some(names)
}

/// The RFC number token a file or directory name starts with: `133` for `133-slug.md`, `DC-43` for
/// `DC-43-SLUG.md` or `DC-43-slug`. `None` for a name that carries no number (`.gitkeep`,
/// `consolidation`) -- such a name identifies no RFC, so it can neither be proposed nor collide.
fn number_token(name: &str) -> Option<String> {
    let (prefix, rest) = match name.get(..3) {
        Some(head) if head.eq_ignore_ascii_case("DC-") => ("DC-", name.get(3..)?),
        _ => ("", name),
    };
    let digits: String = rest
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect();
    let after = rest.get(digits.len()..)?;
    let terminated = after.is_empty() || after.starts_with('-') || after.starts_with('.');
    (!digits.is_empty() && terminated).then(|| format!("{prefix}{digits}"))
}

#[cfg(test)]
#[path = "rfc_lifecycle/tests.rs"]
mod tests;

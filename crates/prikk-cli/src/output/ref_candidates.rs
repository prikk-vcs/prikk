//! 0.59.0 step G2d: the text of `prikk doctor --remove-ref-candidates`.

/// `prikk doctor --remove-ref-candidates` (0.59.0 step G2d): one line per file of `refs/tmp`, what it is and what was (or would be) done.
pub(crate) fn print_remove_ref_candidates_report(report: &prikk_store::RemoveRefCandidatesReport) {
    use prikk_store::{RefCandidateAction, RefCandidateKind};
    if report.candidates.is_empty() {
        println!("no ref pointer candidate remains");
        return;
    }
    for candidate in &report.candidates {
        let what = match &candidate.kind {
            RefCandidateKind::LegacyPointer {
                ref_name,
                ref_state_id,
            } => format!("a ref pointer candidate for {ref_name} naming RefState {ref_state_id}"),
            _ => "not a ref pointer candidate".to_string(),
        };
        match (&candidate.action, &candidate.recovery) {
            (RefCandidateAction::Removed, Some(recovery)) => println!(
                "refs/tmp/{}: removed ({what}, {} byte(s)); saved first: {recovery}",
                candidate.name, candidate.bytes
            ),
            (RefCandidateAction::AlreadyAbsent, _) => println!(
                "refs/tmp/{}: already absent ({what}); nothing to remove",
                candidate.name
            ),
            _ => println!(
                "refs/tmp/{}: would be removed ({what}, {} byte(s))",
                candidate.name, candidate.bytes
            ),
        }
    }
}

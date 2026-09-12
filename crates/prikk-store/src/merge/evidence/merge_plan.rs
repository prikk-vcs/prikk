use super::MergeEvidenceDisplay;

/// Public display view for a read-only merge plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergePlanDisplay {
    /// Underlying evidence display used as the diagnostic source.
    pub evidence: MergeEvidenceDisplay,
    /// DC-25 plan status.
    pub status: &'static str,
    /// Short non-mutating action text.
    pub action: &'static str,
}

impl MergePlanDisplay {
    pub(crate) fn from_evidence(evidence: MergeEvidenceDisplay) -> Self {
        let status = plan_status_from_name(evidence.outcome);
        Self {
            evidence,
            status,
            action: action_for_plan_status(status),
        }
    }

    /// Number of report items displayed by this view.
    pub fn displayed_item_count(&self) -> usize {
        self.evidence.displayed_item_count()
    }

    /// Number of report items in the full evidence report.
    pub fn total_item_count(&self) -> usize {
        self.evidence.total_item_count()
    }
}

fn plan_status_from_name(outcome: &str) -> &'static str {
    match outcome {
        "Confluent" => "ConfluentSubset",
        "Conflict" => "BlockedConflict",
        "OrderedDependency" => "BlockedOrderedDependency",
        "Unsupported" => "BlockedUnsupported",
        "Deferred" => "BlockedDeferred",
        "NotConfluent" => "BlockedNotConfluent",
        "EvidenceFailure" => "BlockedEvidenceFailure",
        "InvalidCandidate" => "BlockedInvalidCandidate",
        _ => "BlockedEvidenceFailure",
    }
}

fn action_for_plan_status(status: &str) -> &'static str {
    match status {
        "ConfluentSubset" => "review the evidence, then run 'prikk merge' to execute",
        "BlockedConflict" => "inspect evidence; conflict resolution is not implemented",
        "BlockedOrderedDependency" => {
            "inspect ordering evidence; execution ordering policy is not implemented"
        }
        "BlockedUnsupported" => "inspect unsupported operation evidence",
        "BlockedDeferred" => "inspect deferred design evidence",
        "BlockedNotConfluent" => "inspect replay/final-state mismatch evidence",
        "BlockedEvidenceFailure" => "repair or verify repository evidence before planning",
        "BlockedInvalidCandidate" => "select valid sealed candidates before planning",
        _ => "unrecognized plan status; inspect evidence",
    }
}

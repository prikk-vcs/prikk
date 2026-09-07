//! Repository-local publication-trust verification.

use prikk_error::{PrikkError, Result};
use prikk_object::ObjectEnvelope;

use crate::foundation::layout::RepositoryLayout;
use crate::trust::{
    MaintainerTrustPolicy, PublicationTrustIssue, load_maintainer_trust_policy,
    verify_trusted_publication_envelope,
};

pub(super) struct PublicationTrustVerifier<'a> {
    layout: &'a RepositoryLayout,
    policy: Option<MaintainerTrustPolicy>,
    policy_issue_added: bool,
    pub(super) checked_records: usize,
    pub(super) issues: Vec<PublicationTrustIssue>,
}

impl<'a> PublicationTrustVerifier<'a> {
    pub(super) const fn new(layout: &'a RepositoryLayout) -> Self {
        Self {
            layout,
            policy: None,
            policy_issue_added: false,
            checked_records: 0,
            issues: Vec::new(),
        }
    }

    /// Returns the matched adopted key id on success (DC-78 §D3), so a caller checking a Block can
    /// report which key sealed it. `None` covers both a recorded trust-policy failure and an
    /// unmatched signature — either way, nothing to report.
    pub(super) fn verify(&mut self, envelope: &ObjectEnvelope) -> Result<Option<String>> {
        self.checked_records = self
            .checked_records
            .checked_add(1)
            .ok_or_else(|| PrikkError::Integrity("publication trust count overflow".to_string()))?;
        if self.policy.is_none() && !self.policy_issue_added {
            match load_maintainer_trust_policy(self.layout) {
                Ok(policy) => self.policy = Some(policy),
                Err(err) => {
                    self.policy_issue_added = true;
                    self.issues.push(PublicationTrustIssue::new(
                        "PRIKK-TRUST-POLICY-INVALID",
                        format!("publication trust policy is invalid: {err}"),
                    ));
                    return Ok(None);
                }
            }
        }
        let Some(policy) = self.policy.as_ref() else {
            return Ok(None);
        };
        match verify_trusted_publication_envelope(policy, envelope) {
            Ok(key_id) => Ok(Some(key_id)),
            Err(issue) => {
                self.issues.push(issue);
                Ok(None)
            }
        }
    }
}

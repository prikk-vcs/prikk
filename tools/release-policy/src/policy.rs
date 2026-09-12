mod challenge;
mod evidence;
mod signer;

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::json;
use crate::oracle::{Case, Observation, ObservationDocument, Oracle};
use crate::schema::SchemaProfile;

#[derive(Debug)]
pub(crate) struct PolicyOutput {
    pub(crate) observations: ObservationDocument,
    pub(crate) reasons: BTreeMap<(String, String), String>,
}

pub(crate) fn run_check(root: &Path) -> Result<()> {
    let oracle = Oracle::load(root)?;
    let output = evaluate(&oracle)?;
    compare_expected(&oracle, &output)?;
    println!(
        "release policy: all {} oracle cases passed",
        output.observations.cases.len()
    );
    Ok(())
}

/// The release-evidence validator for a document produced from this tree, rather than a frozen
/// oracle fixture: the crate set is the caller's -- normally
/// `release_evidence::workspace_crate_order` -- never the corpus literal (RFC 141 §7a).
// RFC 141 §7a: `cfg(test)` because nothing shipped judges a *live* document yet -- the oracle
// judges frozen fixtures, and `produce` records rather than refuses (RFC 141 §7b.1). The
// derivation is exercised by the controls; its first production caller is increment 4's.
#[cfg(test)]
pub(crate) fn release_evidence_reason(
    document: &Value,
    expected_crates: &[(String, u64)],
) -> Option<&'static str> {
    evidence::single_reason(document, expected_crates)
}

/// What differs between a document's crate rows and `expected_crates`, naming the crate.
// RFC 141 §7a: `cfg(test)` because nothing shipped judges a *live* document yet -- the oracle
// judges frozen fixtures, and `produce` records rather than refuses (RFC 141 §7b.1). The
// derivation is exercised by the controls; its first production caller is increment 4's.
#[cfg(test)]
pub(crate) fn release_evidence_crate_set_mismatch(
    document: &Value,
    expected_crates: &[(String, u64)],
) -> Option<String> {
    evidence::crate_set_mismatch(
        evidence::value_array(document, "crates").unwrap_or_default(),
        expected_crates,
    )
}

pub(crate) fn evaluate(oracle: &Oracle) -> Result<PolicyOutput> {
    let schema_bytes = oracle.manifest.normative_schema.path.as_str();
    let schema_value = json::parse(&std::fs::read(oracle.root().join(schema_bytes))?)
        .map_err(|error| Error::new(format!("schema profile input: {error}")))?;
    let schema = SchemaProfile::compile(&schema_value)?;
    let mut observations = Vec::new();
    let mut reasons = BTreeMap::new();
    for case in &oracle.manifest.cases {
        let result = evaluate_case(oracle, case, &schema)?;
        reasons.insert(
            (case.suite_id.clone(), case.fixture_case_id.clone()),
            result.reason,
        );
        observations.push(result.observation);
    }
    observations.sort_by(|left, right| {
        (&left.suite_id, &left.case_id).cmp(&(&right.suite_id, &right.case_id))
    });
    Ok(PolicyOutput {
        observations: ObservationDocument {
            schema_version: "rust-policy-observations-v1".to_owned(),
            python_baseline_commit: "12c137d".to_owned(),
            profile_contract_commit: "ea427df".to_owned(),
            cases: observations,
        },
        reasons,
    })
}

struct CaseResult {
    observation: Observation,
    reason: String,
}

fn evaluate_case(oracle: &Oracle, case: &Case, schema: &SchemaProfile) -> Result<CaseResult> {
    let (outcome, structural, semantic, reason) = match case.suite_id.as_str() {
        "signer-authority-live" => {
            let valid = signer::authority_valid(oracle.input(case, "authority")?);
            simple(valid, "authority-grammar")
        }
        "signer-authority" => {
            let row = table_case(oracle.input(case, "fixture-table")?, &case.fixture_case_id)?;
            let document = row
                .get("document")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::new("authority case missing document"))?;
            simple(
                signer::authority_valid(document.as_bytes()),
                "authority-grammar",
            )
        }
        "signer-governance" => {
            let row = table_case(oracle.input(case, "fixture-table")?, &case.fixture_case_id)?;
            match signer::transaction_reason(&row) {
                None => simple(true, "none"),
                Some(reason) => simple(false, reason),
            }
        }
        "signer-challenge" => {
            let context = json::parse(oracle.input(case, "fixture-table")?)
                .map_err(|error| Error::new(error.to_string()))?;
            match challenge::validate(&context, oracle.input(case, "challenge")?) {
                None => simple(true, "none"),
                Some(reason) => simple(false, reason),
            }
        }
        "release-evidence" => evidence::evaluate(oracle, case, schema)?,
        suite => return Err(Error::new(format!("unsupported oracle suite: {suite}"))),
    };
    let final_value = match outcome {
        "valid" | "valid-local-only" => "valid",
        "validator-error" => "validator-error",
        _ => "invalid",
    };
    Ok(CaseResult {
        observation: Observation {
            suite_id: case.suite_id.clone(),
            case_id: case.fixture_case_id.clone(),
            final_: final_value.to_owned(),
            case_outcome: outcome.to_owned(),
            input_digest: case.input_digest(),
            structural: structural.map(str::to_owned),
            semantic: semantic.map(str::to_owned),
        },
        reason: reason.to_owned(),
    })
}

fn simple(
    valid: bool,
    invalid_reason: &str,
) -> (
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    &str,
) {
    (
        if valid { "valid" } else { "invalid" },
        None,
        None,
        if valid { "none" } else { invalid_reason },
    )
}

pub(crate) fn table_case(bytes: &[u8], id: &str) -> Result<Value> {
    let table = json::parse(bytes).map_err(|error| Error::new(error.to_string()))?;
    table
        .get("cases")
        .and_then(Value::as_array)
        .and_then(|cases| {
            cases
                .iter()
                .find(|case| case.get("id").and_then(Value::as_str) == Some(id))
        })
        .cloned()
        .ok_or_else(|| Error::new(format!("fixture case not found: {id}")))
}

fn compare_expected(oracle: &Oracle, output: &PolicyOutput) -> Result<()> {
    let actual: BTreeMap<(&str, &str), &Observation> = output
        .observations
        .cases
        .iter()
        .map(|item| ((item.suite_id.as_str(), item.case_id.as_str()), item))
        .collect();
    let mut errors = Vec::new();
    for case in &oracle.manifest.cases {
        let key = (case.suite_id.as_str(), case.fixture_case_id.as_str());
        let Some(observation) = actual.get(&key) else {
            errors.push(format!("{}:{}: missing observation", key.0, key.1));
            continue;
        };
        let expected = &case.expected;
        if observation.final_ != expected.final_
            || observation.case_outcome != expected.case_outcome
            || observation
                .structural
                .as_deref()
                .unwrap_or(&expected.structural)
                != expected.structural
            || observation
                .semantic
                .as_deref()
                .unwrap_or(&expected.semantic)
                != expected.semantic
        {
            errors.push(format!("{}:{}: observation mismatch", key.0, key.1));
        }
        if output.reasons.get(&(key.0.to_owned(), key.1.to_owned()))
            != Some(&expected.primary_reason)
        {
            errors.push(format!("{}:{}: primary reason mismatch", key.0, key.1));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(Error::new(errors.join("\n")))
    }
}

#[cfg(test)]
#[path = "policy/tests.rs"]
mod tests;

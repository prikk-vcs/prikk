use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{governance, value_array};

const TRANSITIONS: [(&str, &[&str]); 4] = [
    ("pending", &["pending", "partial", "complete", "superseded"]),
    ("partial", &["partial", "complete", "superseded"]),
    ("complete", &["superseded"]),
    ("superseded", &[]),
];

pub(super) fn reason(
    prior: &Value,
    current: &Value,
    prior_bytes: &[u8],
    current_bytes: &[u8],
) -> Option<&'static str> {
    if crate::json::parse(prior_bytes).ok().as_ref() != Some(prior)
        || crate::json::parse(current_bytes).ok().as_ref() != Some(current)
    {
        return Some("evidence-byte-identity-or-link");
    }
    if prior.get("sequence").and_then(Value::as_str) != Some("001")
        || current.get("sequence").and_then(Value::as_str) != Some("002")
        || prior
            .get("prior_snapshot")
            .is_none_or(|value| !value.is_null())
    {
        return Some("evidence-byte-identity-or-link");
    }
    let expected_name = format!(
        "prikk-{}-release-evidence-{}.json",
        prior
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        prior
            .get("sequence")
            .and_then(Value::as_str)
            .unwrap_or_default()
    );
    let link = current.get("prior_snapshot");
    if link
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str)
        != Some(expected_name.as_str())
        || link
            .and_then(|value| value.get("sha256"))
            .and_then(Value::as_str)
            // sha2 0.11's digest output has no `LowerHex` impl; hex-encoded byte-by-byte instead.
            != Some(
                Sha256::digest(prior_bytes)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
                    .as_str(),
            )
    {
        return Some("evidence-byte-identity-or-link");
    }
    if basic_immutable_changed(prior, current) || published_values_regressed(prior, current) {
        return Some("evidence-byte-identity-or-link");
    }
    if tag_verification_progression_changed(prior, current) {
        return Some("evidence-tag-or-artifact");
    }
    if let Some(reason) =
        governance::progression_reason(prior.get("governance"), current.get("governance"))
    {
        return Some(reason);
    }
    let old_status = prior
        .get("overall_status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let new_status = current
        .get("overall_status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let allowed = TRANSITIONS
        .iter()
        .find(|(status, _)| *status == old_status)
        .is_some_and(|(_, targets)| targets.contains(&new_status));
    if !allowed {
        return Some("evidence-transition-or-attempt-prefix");
    }
    let old_attempts = value_array(prior, "attempts");
    let new_attempts = value_array(current, "attempts");
    let (Some(old_attempts), Some(new_attempts)) = (old_attempts, new_attempts) else {
        return Some("evidence-transition-or-attempt-prefix");
    };
    if new_attempts.len() <= old_attempts.len()
        || new_attempts.get(..old_attempts.len()) != Some(old_attempts)
    {
        return Some("evidence-transition-or-attempt-prefix");
    }
    None
}

fn basic_immutable_changed(old: &Value, new: &Value) -> bool {
    if old.get("version") != new.get("version") {
        return true;
    }
    for field in ["name", "object_id", "peeled_commit"] {
        if old.get("tag").and_then(|tag| tag.get(field))
            != new.get("tag").and_then(|tag| tag.get(field))
        {
            return true;
        }
    }
    for field in ["name", "checksum_name"] {
        if old.get("archive").and_then(|value| value.get(field))
            != new.get("archive").and_then(|value| value.get(field))
        {
            return true;
        }
    }
    let old_crates = value_array(old, "crates").unwrap_or_default();
    let new_crates = value_array(new, "crates").unwrap_or_default();
    old_crates.len() != new_crates.len()
        || old_crates.iter().zip(new_crates).any(|(left, right)| {
            ["name", "version", "publish_level"]
                .iter()
                .any(|field| left.get(field) != right.get(field))
        })
}

fn published_values_regressed(old: &Value, new: &Value) -> bool {
    for field in ["archive_sha256", "checksum_sha256"] {
        let previous = old.get("archive").and_then(|value| value.get(field));
        if previous.is_some_and(|value| !value.is_null())
            && previous != new.get("archive").and_then(|value| value.get(field))
        {
            return true;
        }
    }
    let old_crates = value_array(old, "crates").unwrap_or_default();
    let new_crates = value_array(new, "crates").unwrap_or_default();
    for (previous, current) in old_crates.iter().zip(new_crates) {
        for field in ["staged_sha256", "registry_checksum", "fetched_sha256"] {
            let value = previous.get(field);
            if value.is_some_and(|item| !item.is_null()) && value != current.get(field) {
                return true;
            }
        }
        for field in ["published", "registry_visible"] {
            if previous.get(field).and_then(Value::as_bool) == Some(true)
                && current.get(field).and_then(Value::as_bool) != Some(true)
            {
                return true;
            }
        }
    }
    for field in ["archive_attached", "checksum_attached"] {
        if old
            .get("archive")
            .and_then(|value| value.get(field))
            .and_then(Value::as_bool)
            == Some(true)
            && new
                .get("archive")
                .and_then(|value| value.get(field))
                .and_then(Value::as_bool)
                != Some(true)
        {
            return true;
        }
    }
    old.get("release_page")
        .and_then(|value| value.get("status"))
        == Some(&Value::String("published".to_owned()))
        && new
            .get("release_page")
            .and_then(|value| value.get("status"))
            != Some(&Value::String("published".to_owned()))
        || old
            .get("pages")
            .and_then(|value| value.get("deployed_commit"))
            .is_some_and(|value| !value.is_null())
            && old.get("pages") != new.get("pages")
}

fn tag_verification_progression_changed(old: &Value, new: &Value) -> bool {
    let old = old
        .get("tag")
        .and_then(|value| value.get("release_tag_verification"));
    let new = new
        .get("tag")
        .and_then(|value| value.get("release_tag_verification"));
    let (Some(old), Some(new)) = (old, new) else {
        return true;
    };
    if old.get("status").and_then(Value::as_str) != Some("not-observed")
        && old.get("status") != new.get("status")
    {
        return true;
    }
    [
        "signer_primary_fingerprint",
        "authority_path",
        "authority_blob_id",
        "verifier_result",
    ]
    .iter()
    .any(|field| {
        old.get(field).is_some_and(|value| !value.is_null()) && old.get(field) != new.get(field)
    })
}

#[cfg(test)]
#[path = "sequence/tests.rs"]
mod tests;

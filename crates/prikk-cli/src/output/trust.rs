//! `prikk trust maintainer list`/`check` output (RFC 138). Both wordings must satisfy §3.1's own
//! distinction: adopting a key means Prikk accepts its signatures on objects (Block/RefState); it
//! never lets that key move a ref. **Neither format ever says "required" or otherwise reports a
//! threshold as policy** -- `MaintainerTrustPolicy` holds a `Vec` and nothing else, trust is
//! any-of-N by construction. The `policy: required=1` literal this RFC declined to add a third
//! site for is gone entirely now: the carried-defects round replaced both existing sites
//! (`main.rs`'s `trust maintainer add`, `setup.rs`) with a derived `adopted maintainer keys: <n>`
//! count.

use crate::stdout::println;
use prikk_store::{AdoptedMaintainerKey, MaintainerTrustPolicy};

use super::verification::escape_json_string;

const NOT_REF_AUTHORITY_NOTE: &str = "note: this is object trust, not ref authority -- an adopted \
     key's signatures are accepted on objects it signed, but adopting a key never lets it move a \
     ref by itself";

/// `prikk trust maintainer list`, prose. An empty policy is a successful, empty answer (RFC 138
/// control 3), never an error.
pub(crate) fn print_trust_list(policy: &MaintainerTrustPolicy) {
    if policy.keys.is_empty() {
        println!("no maintainer keys adopted");
        return;
    }
    println!("adopted maintainer keys, in adoption order:");
    for (index, key) in policy.keys.iter().enumerate() {
        println!(
            "  {}. {}  {}",
            index + 1,
            key.key_id,
            prikk_hash::to_hex(&key.public_key)
        );
    }
    println!("{NOT_REF_AUTHORITY_NOTE}");
}

/// `prikk trust maintainer list --format json`: `trust-list-v1`, named for the tool (`trust`), the
/// question (`list`), and versioned like `verify-report-v1` and this project's other
/// schema-versioned reports -- same idiom, not the same name (RFC 138 §2).
pub(crate) fn print_trust_list_json(policy: &MaintainerTrustPolicy) {
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str("  \"schema_version\": \"trust-list-v1\",\n");
    json.push_str("  \"keys\": [");
    for (index, key) in policy.keys.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str("\n    {\"key_id\": ");
        json.push_str(&escape_json_string(&key.key_id));
        json.push_str(", \"public_key\": ");
        json.push_str(&escape_json_string(&prikk_hash::to_hex(&key.public_key)));
        json.push('}');
    }
    if !policy.keys.is_empty() {
        json.push_str("\n  ");
    }
    json.push_str("]\n}");
    println!("{json}");
}

/// `prikk trust maintainer check --key-id <ID>`, prose. Exits `0` whichever way the question
/// resolves (RFC 138 §3/RFC 121) -- the caller decides the exit code from the boolean this returns
/// unused here; this function only prints.
pub(crate) fn print_trust_check(key_id: &str, found: Option<&AdoptedMaintainerKey>) {
    match found {
        Some(key) => {
            println!(
                "trusted: {}  {}",
                key.key_id,
                prikk_hash::to_hex(&key.public_key)
            );
            println!("{NOT_REF_AUTHORITY_NOTE}");
        }
        None => println!("not trusted: {key_id}"),
    }
}

/// `prikk trust maintainer check --key-id <ID> --format json`: `trust-check-v1`.
pub(crate) fn print_trust_check_json(key_id: &str, found: Option<&AdoptedMaintainerKey>) {
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str("  \"schema_version\": \"trust-check-v1\",\n");
    json.push_str(&format!("  \"key_id\": {},\n", escape_json_string(key_id)));
    json.push_str(&format!("  \"trusted\": {},\n", found.is_some()));
    match found {
        Some(key) => json.push_str(&format!(
            "  \"public_key\": {}\n",
            escape_json_string(&prikk_hash::to_hex(&key.public_key))
        )),
        None => json.push_str("  \"public_key\": null\n"),
    }
    json.push('}');
    println!("{json}");
}

/// `prikk trust maintainer add --format json`, on success: `trust-add-v1` (0.56.0 step T), named for
/// the tool (`trust`) and the question (`add`) like `trust-list-v1` and `trust-check-v1`. It carries
/// what the prose form prints -- which key id, whether this call changed the adopted set
/// (`newly_added`, `false` for the idempotent re-add), and the `adopted maintainer keys: N` count --
/// plus the public key, lowercase hex. Nothing else is printed under the flag.
pub(crate) fn print_trust_add_json(
    adopted: &AdoptedMaintainerKey,
    newly_added: bool,
    adopted_count: usize,
) {
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str("  \"schema_version\": \"trust-add-v1\",\n");
    json.push_str(&format!(
        "  \"key_id\": {},\n",
        escape_json_string(&adopted.key_id)
    ));
    json.push_str(&format!(
        "  \"public_key\": {},\n",
        escape_json_string(&prikk_hash::to_hex(&adopted.public_key))
    ));
    json.push_str(&format!("  \"newly_added\": {newly_added},\n"));
    json.push_str(&format!("  \"adopted_count\": {adopted_count}\n"));
    json.push('}');
    println!("{json}");
}

/// `refusal-v1` (the project's reusable refusal document, see `bundle.rs`'s
/// `print_bundle_import_refusal_json`) for `trust maintainer add`'s one structurally classified
/// cause, a key id already bound to a different public key. `currently_adopted` picks the id:
/// `maintainer-key-id-adopted-with-different-key` when the id is in the adopted set, and
/// `maintainer-key-id-bound-to-removed-key` when it was removed and only its first key is on record.
/// `reason` is always `null`; `message` is `err.to_string()`, stderr's own text without `error: `.
pub(crate) fn print_trust_add_refusal_json(currently_adopted: bool, message: &str) {
    let id = if currently_adopted {
        "maintainer-key-id-adopted-with-different-key"
    } else {
        "maintainer-key-id-bound-to-removed-key"
    };
    let mut json = String::new();
    json.push_str(
        "{\"schema_version\": \"refusal-v1\", \"command\": \"trust-maintainer-add\", \"refusal\": ",
    );
    json.push_str(&escape_json_string(id));
    json.push_str(", \"reason\": null, \"message\": ");
    json.push_str(&escape_json_string(message));
    json.push('}');
    println!("{json}");
}

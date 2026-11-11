use std::path::Path;

use super::{BoundaryError, PRODUCTS, push};

const ALLOWED_THIRD_PARTY: [(&str, &[&str]); 8] = [
    ("prikk-error", &[]),
    // DC-96: windows-sys under [target.'cfg(windows)'.dependencies] -- the sole exemption in
    // UNSAFE_EXEMPT_CRATES below, and the sole reason this crate exists. 0.57.0 step 2 (L1) added libc
    // under [target.'cfg(target_os = "macos")'.dependencies], for the macOS process start time and boot time.
    ("prikk-ffi", &["libc", "windows-sys"]),
    ("prikk-hash", &["sha2"]),
    ("prikk-crypto", &["ed25519-dalek", "getrandom"]),
    // 0.57.0 U2 (owner decision D2, option B): NFC/NFD and full case folding for repository paths. `prikk-object` is the
    // lowest crate every path check reaches; `tinyvec` is transitive (via `unicode-normalization`) and not a direct edge.
    ("prikk-object", &["unicase", "unicode-normalization"]),
    ("prikk-replay", &[]),
    ("prikk-store", &["getrandom", "rustix"]),
    ("prikk", &[]),
];

pub(super) fn check(root: &Path, errors: &mut Vec<BoundaryError>) {
    for (crate_name, manifest_path) in PRODUCTS {
        match std::fs::read_to_string(root.join(manifest_path)) {
            Ok(text) => check_manifest(crate_name, &text, errors),
            Err(_) => push(
                errors,
                "dependency-placement",
                format!("{crate_name}: manifest unreadable"),
            ),
        }
    }
}

fn check_manifest(crate_name: &str, text: &str, errors: &mut Vec<BoundaryError>) {
    let Ok(manifest) = toml::from_str::<toml::Value>(text) else {
        push(
            errors,
            "dependency-placement",
            format!("{crate_name}: manifest unparseable"),
        );
        return;
    };
    let allowed = ALLOWED_THIRD_PARTY
        .iter()
        .find(|(name, _)| *name == crate_name)
        .map_or(&[][..], |(_, list)| *list);
    for (key, value) in dependency_entries(&manifest) {
        check_entry(crate_name, key, value, allowed, errors);
    }
}

/// Collects `[dependencies]`, `[build-dependencies]`, and every
/// `[target.*.dependencies]` / `[target.*.build-dependencies]` entry. These are the
/// only tables that can carry a shipping dependency; `[dev-dependencies]` is
/// deliberately excluded everywhere, including under `[target.*]` — it is the sink
/// this check protects.
fn dependency_entries(manifest: &toml::Value) -> Vec<(&str, &toml::Value)> {
    let mut entries = Vec::new();
    for section in ["dependencies", "build-dependencies"] {
        if let Some(table) = manifest.get(section).and_then(toml::Value::as_table) {
            entries.extend(table.iter().map(|(key, value)| (key.as_str(), value)));
        }
    }
    if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
        for target in targets.values() {
            for section in ["dependencies", "build-dependencies"] {
                if let Some(table) = target.get(section).and_then(toml::Value::as_table) {
                    entries.extend(table.iter().map(|(key, value)| (key.as_str(), value)));
                }
            }
        }
    }
    entries
}

fn check_entry(
    crate_name: &str,
    key: &str,
    value: &toml::Value,
    allowed: &[&str],
    errors: &mut Vec<BoundaryError>,
) {
    // A `package = "..."` field lets the key differ from the crate it resolves to,
    // defeating a key-only allowlist check regardless of whether the key itself
    // (including a `prikk-*` key) would otherwise be permitted.
    let renamed = value
        .as_table()
        .is_some_and(|table| table.contains_key("package"));
    if renamed || (!key.starts_with("prikk-") && !allowed.contains(&key)) {
        push(
            errors,
            "dependency-placement",
            format!("{crate_name}:{key}"),
        );
    }
}

#[cfg(test)]
#[path = "placement/tests.rs"]
mod tests;

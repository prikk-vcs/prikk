//! RFC 141 increment 1: producing a `release-evidence-v1` document (DC-35), never a validator --
//! `policy::evidence` already validates; this module builds.
//!
//! **The split (handoff §2)**: two of DC-35's three per-crate SHA-256 values cannot be obtained
//! offline (the registry checksum and the fetched bytes both require network access). So this
//! module is a pure function from already-gathered [`Observations`] to a document -- it never
//! fetches a checksum, never runs `cargo package`, never touches the network. Gathering those
//! observations is a separate, thin layer this increment does not build beyond what
//! `bin/produce_release_evidence.rs` needs to exercise the pure function once.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use cargo_metadata::{DependencyKind, Metadata, MetadataCommand};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::json;
use crate::schema::SchemaProfile;

const NORMATIVE_SCHEMA_PATH: &str = "release/schemas/release-evidence-v1.schema.json";

/// One crate's DC-35 observations. `publish_level` and `checksum_equality` are deliberately absent
/// here -- both are derived by this module, never accepted from a caller (handoff §3, items 1-2).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CrateObservation {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) exact_internal_requirements: bool,
    #[serde(default)]
    pub(crate) staged_sha256: Option<String>,
    #[serde(default)]
    pub(crate) registry_checksum: Option<String>,
    #[serde(default)]
    pub(crate) fetched_sha256: Option<String>,
    pub(crate) published: bool,
    pub(crate) registry_visible: bool,
}

/// Everything the producer needs that it cannot derive itself. `tag`/`archive`/`release_page`/
/// `pages`/`governance` are pass-through observations -- their *truthfulness* is the caller's own
/// responsibility to have observed (this increment's offline/network split, per the handoff, is
/// specifically about the three per-crate checksums; verifying a git tag signature or a GitHub
/// Pages deployment is its own capability this increment does not build). This module still reads
/// the specific fields it needs from them to derive `overall_status` honestly, and validates the
/// assembled document against the schema before returning it -- a caller cannot make this module
/// emit a structurally invalid document by supplying a malformed pass-through section.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Observations {
    pub(crate) version: String,
    pub(crate) tag: Value,
    pub(crate) archive: Value,
    pub(crate) crates: Vec<CrateObservation>,
    pub(crate) release_page: Value,
    pub(crate) pages: Value,
    pub(crate) governance: Value,
    /// Attempts recorded *this run*, appended after any carried-forward from `prior` (handoff §3:
    /// "attempts is cumulative and append-only"). Each entry supplies `time`/`operation`/`result`;
    /// `sequence` is overwritten by this module regardless of what is supplied, so the combined
    /// list is always contiguous from `1`.
    #[serde(default)]
    pub(crate) new_attempts: Vec<Value>,
    /// Force `overall_status: "superseded"`, bypassing the pending/partial/complete derivation --
    /// a fact about *this* release's relationship to a *later* one, which no observation of this
    /// release's own state can derive.
    #[serde(default)]
    pub(crate) superseded: bool,
}

/// Where to chain from, and what this module must confirm before trusting it: the caller's own
/// belief about the prior file's SHA-256 (`expected_sha256`), checked against the hash this module
/// computes from the file's actual bytes (handoff §6 control 5: "perturb the predecessor hash and
/// confirm rejection" -- a caller passing a stale or wrong expectation must be refused, not
/// silently overridden by whatever bytes happen to be on disk).
#[derive(Debug, Clone)]
pub(crate) struct PriorLink {
    pub(crate) path: std::path::PathBuf,
    pub(crate) expected_sha256: String,
}

/// Deserialize [`Observations`] from a JSON file, through this project's strict parser (rejects a
/// byte-order mark and duplicate object keys -- the same parser every other JSON input in this
/// tool goes through).
pub(crate) fn load_observations(path: &Path) -> Result<Observations> {
    let bytes = std::fs::read(path).map_err(Error::from)?;
    let value = json::parse(&bytes).map_err(|error| Error::new(error.to_string()))?;
    serde_json::from_value(value).map_err(Error::from)
}

/// Produce a `release-evidence-v1` document from `observations`, chaining from `prior` when given.
/// Validates the result against the schema before returning -- this module never emits a document
/// it has not itself confirmed is schema-valid.
pub(crate) fn produce(
    root: &Path,
    observations: Observations,
    prior: Option<&PriorLink>,
) -> Result<Value> {
    let levels = publish_levels(root)?;
    let mut crate_rows = Vec::with_capacity(observations.crates.len());
    for crate_observation in &observations.crates {
        let level = *levels.get(&crate_observation.name).ok_or_else(|| {
            Error::new(format!(
                "crate {:?} is not a workspace member; publish_level cannot be derived",
                crate_observation.name
            ))
        })?;
        crate_rows.push(crate_row(crate_observation, level));
    }
    // Dependency order -- publish level, then name -- which is the order a release publishes in
    // and the order `policy::evidence` checks a document against. Emitting rows in whatever order
    // the caller happened to list observations would make a correct document fail on position.
    crate_rows.sort_by(|left, right| {
        let key = |row: &Value| {
            (
                row.get("publish_level").and_then(Value::as_u64),
                row.get("name").and_then(Value::as_str).map(str::to_owned),
            )
        };
        key(left).cmp(&key(right))
    });

    let (sequence, prior_snapshot, mut attempts) = match prior {
        Some(link) => {
            let loaded = load_prior(link)?;
            (
                loaded.sequence + 1,
                json!({"name": loaded.file_name, "sha256": loaded.sha256}),
                loaded.attempts,
            )
        }
        None => (1_u32, Value::Null, Vec::new()),
    };
    for mut attempt in observations.new_attempts.clone() {
        let next_sequence =
            u64::try_from(attempts.len()).map_err(|_| Error::new("attempt sequence overflow"))? + 1;
        if let Some(object) = attempt.as_object_mut() {
            object.insert("sequence".to_string(), json!(next_sequence));
        } else {
            return Err(Error::new("each new_attempts entry must be a JSON object"));
        }
        attempts.push(attempt);
    }

    let overall_status = derive_overall_status(&observations, &crate_rows);

    let document = json!({
        "schema_version": 1,
        "sequence": format!("{sequence:03}"),
        "version": observations.version,
        "overall_status": overall_status,
        "prior_snapshot": prior_snapshot,
        "tag": observations.tag,
        "archive": observations.archive,
        "crates": crate_rows,
        "release_page": observations.release_page,
        "pages": observations.pages,
        "governance": observations.governance,
        "attempts": attempts,
    });

    let schema_value = json::parse(&std::fs::read(root.join(NORMATIVE_SCHEMA_PATH))?)
        .map_err(|error| Error::new(error.to_string()))?;
    let schema = SchemaProfile::compile(&schema_value)?;
    if !schema.is_valid(&document) {
        return Err(Error::new(format!(
            "produced document fails its own schema: {}",
            schema.errors(&document).join("; ")
        )));
    }
    Ok(document)
}

fn crate_row(observation: &CrateObservation, publish_level: u32) -> Value {
    json!({
        "name": observation.name,
        "version": observation.version,
        "exact_internal_requirements": observation.exact_internal_requirements,
        "publish_level": publish_level,
        "staged_sha256": observation.staged_sha256,
        "registry_checksum": observation.registry_checksum,
        "fetched_sha256": observation.fetched_sha256,
        "checksum_equality": checksum_equality(observation),
        "published": observation.published,
        "registry_visible": observation.registry_visible,
    })
}

/// Handoff §3 item 1, the single most damaging thing this increment could ship if gotten wrong:
/// a checksum absent from **any** of the three is `"not-observed"`, never defaulted to `"match"`
/// because nothing contradicted it. Only when all three are present does this compare them.
fn checksum_equality(observation: &CrateObservation) -> &'static str {
    match (
        &observation.staged_sha256,
        &observation.registry_checksum,
        &observation.fetched_sha256,
    ) {
        (Some(staged), Some(registry), Some(fetched)) => {
            if staged == registry && registry == fetched {
                "match"
            } else {
                "mismatch"
            }
        }
        _ => "not-observed",
    }
}

/// `pending` when nothing has been attempted for any crate; `complete` only when every crate's
/// checksums match and it is published and registry-visible, *and* the tag, archive, release page,
/// and pages sections the caller supplied are themselves complete-shaped; `partial` otherwise.
/// Never accepted as an argument (handoff §3) -- always derived from the same facts a reader of
/// the document would have to re-derive it from.
fn derive_overall_status(observations: &Observations, crate_rows: &[Value]) -> &'static str {
    if observations.superseded {
        return "superseded";
    }
    let nothing_attempted = crate_rows.iter().all(|row| {
        row.get("checksum_equality") == Some(&Value::String("not-observed".to_string()))
            && row.get("published") == Some(&Value::Bool(false))
            && row.get("registry_visible") == Some(&Value::Bool(false))
    });
    if nothing_attempted {
        return "pending";
    }
    let crates_complete = crate_rows.iter().all(|row| {
        row.get("checksum_equality") == Some(&Value::String("match".to_string()))
            && row.get("published") == Some(&Value::Bool(true))
            && row.get("registry_visible") == Some(&Value::Bool(true))
    });
    if crates_complete && rest_of_release_complete(observations) {
        "complete"
    } else {
        "partial"
    }
}

fn rest_of_release_complete(observations: &Observations) -> bool {
    observations
        .tag
        .get("release_tag_verification")
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str)
        == Some("verified")
        && observations
            .archive
            .get("archive_attached")
            .and_then(Value::as_bool)
            == Some(true)
        && observations
            .archive
            .get("checksum_attached")
            .and_then(Value::as_bool)
            == Some(true)
        && observations
            .archive
            .get("checksum_grammar")
            .and_then(Value::as_str)
            == Some("valid")
        && observations
            .archive
            .get("archive_root")
            .and_then(Value::as_str)
            == Some("valid")
        && observations
            .release_page
            .get("status")
            .and_then(Value::as_str)
            == Some("published")
        && pages_complete(&observations.pages, &observations.tag)
}

fn pages_complete(pages: &Value, tag: &Value) -> bool {
    match pages.get("status").and_then(Value::as_str) {
        Some("deployed") => pages.get("deployed_commit") == tag.get("peeled_commit"),
        Some("inapplicable") => pages
            .get("inapplicable_ruling")
            .and_then(Value::as_str)
            .is_some_and(|ruling| !ruling.is_empty()),
        _ => false,
    }
}

struct LoadedPrior {
    sequence: u32,
    file_name: String,
    sha256: String,
    attempts: Vec<Value>,
}

/// Read the prior evidence file, confirm its actual bytes hash to `link.expected_sha256`
/// (control 5's own hazard: a caller's stale belief about the predecessor must be refused, not
/// silently corrected), then extract what this module needs from it: its own `sequence` (to
/// increment) and its `attempts` (to carry forward, in order).
fn load_prior(link: &PriorLink) -> Result<LoadedPrior> {
    let bytes = std::fs::read(&link.path).map_err(Error::from)?;
    let actual_sha256 = hex_sha256(&bytes);
    if actual_sha256 != link.expected_sha256 {
        return Err(Error::new(format!(
            "prior evidence file {} does not hash to the expected predecessor SHA-256 \
             (expected {}, computed {actual_sha256}) -- refusing to chain from it",
            link.path.display(),
            link.expected_sha256
        )));
    }
    let document = json::parse(&bytes).map_err(|error| Error::new(error.to_string()))?;
    let sequence_str = document
        .get("sequence")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::new("prior evidence file has no string sequence field"))?;
    let sequence: u32 = sequence_str.parse().map_err(|_| {
        Error::new(format!(
            "prior evidence sequence {sequence_str:?} is not numeric"
        ))
    })?;
    let attempts = document
        .get("attempts")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| Error::new("prior evidence file has no attempts array"))?;
    let file_name = link
        .path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .ok_or_else(|| Error::new("prior evidence path has no file name"))?;
    Ok(LoadedPrior {
        sequence,
        file_name,
        sha256: actual_sha256,
        attempts,
    })
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The crates a release publishes, in the order a release-evidence document must list them.
///
/// RFC 141 §7a: this replaces `policy/evidence.rs`'s hand-written `CRATE_ORDER`, which had seven
/// entries when the workspace published eight and nothing forced it to track the difference.
/// Derived on every call -- workspace members, minus any with `publish = false`, sorted by publish
/// level and then by name -- so a member added, removed or re-pointed changes the answer with no
/// list to update.
// RFC 141 §7a: `cfg(test)` because nothing shipped judges a *live* document yet -- the oracle
// judges frozen fixtures, and `produce` records rather than refuses (RFC 141 §7b.1). The
// derivation is exercised by the controls; its first production caller is increment 4's.
#[cfg(test)]
pub(crate) fn workspace_crate_order(root: &Path) -> Result<Vec<(String, u64)>> {
    let metadata = workspace_metadata(root)?;
    let levels = levels_from_metadata(&metadata)?;
    let mut order: Vec<(String, u64)> = metadata
        .workspace_members
        .iter()
        .filter_map(|id| metadata.packages.iter().find(|package| &package.id == id))
        // `Some([])` is `publish = false`; `None` is publishable anywhere.
        .filter(|package| {
            package
                .publish
                .as_ref()
                .is_none_or(|registries| !registries.is_empty())
        })
        .filter_map(|package| {
            let name = package.name.to_string();
            levels.get(&name).map(|level| (name, u64::from(*level)))
        })
        .collect();
    order.sort_by(|(left_name, left_level), (right_name, right_level)| {
        left_level
            .cmp(right_level)
            .then_with(|| left_name.cmp(right_name))
    });
    Ok(order)
}

fn workspace_metadata(root: &Path) -> Result<Metadata> {
    MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .other_options(vec!["--locked".to_owned(), "--offline".to_owned()])
        .exec()
        .map_err(|error| Error::new(format!("cargo metadata failed: {error}")))
}

/// Handoff §3 item 2: `publish_level` is the topological level in the workspace's own dependency
/// graph, derived via `cargo_metadata` -- never a hardcoded list, so it has nothing to fall out of
/// sync with.
fn publish_levels(root: &Path) -> Result<BTreeMap<String, u32>> {
    levels_from_metadata(&workspace_metadata(root)?)
}

fn levels_from_metadata(metadata: &Metadata) -> Result<BTreeMap<String, u32>> {
    let workspace_names: BTreeSet<String> = metadata
        .workspace_members
        .iter()
        .filter_map(|id| metadata.packages.iter().find(|package| &package.id == id))
        .map(|package| package.name.to_string())
        .collect();
    let mut internal_deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for id in &metadata.workspace_members {
        let Some(package) = metadata.packages.iter().find(|package| &package.id == id) else {
            continue;
        };
        let deps: Vec<String> = package
            .dependencies
            .iter()
            .filter(|dependency| {
                dependency.kind != DependencyKind::Development
                    && workspace_names.contains(&dependency.name)
            })
            .map(|dependency| dependency.name.clone())
            .collect();
        internal_deps.insert(package.name.to_string(), deps);
    }

    let mut levels: BTreeMap<String, u32> = BTreeMap::new();
    let mut visiting: BTreeSet<String> = BTreeSet::new();
    for name in internal_deps.keys().cloned().collect::<Vec<_>>() {
        level_of(&name, &internal_deps, &mut levels, &mut visiting)?;
    }
    Ok(levels)
}

fn level_of(
    name: &str,
    internal_deps: &BTreeMap<String, Vec<String>>,
    levels: &mut BTreeMap<String, u32>,
    visiting: &mut BTreeSet<String>,
) -> Result<u32> {
    if let Some(level) = levels.get(name) {
        return Ok(*level);
    }
    if !visiting.insert(name.to_string()) {
        return Err(Error::new(format!(
            "dependency cycle detected involving {name} while deriving publish levels"
        )));
    }
    let own_deps = internal_deps.get(name).cloned().unwrap_or_default();
    let level = if own_deps.is_empty() {
        1
    } else {
        let mut max_dependency_level = 0;
        for dependency in &own_deps {
            max_dependency_level =
                max_dependency_level.max(level_of(dependency, internal_deps, levels, visiting)?);
        }
        max_dependency_level + 1
    };
    visiting.remove(name);
    levels.insert(name.to_string(), level);
    Ok(level)
}

#[cfg(test)]
#[path = "release_evidence/tests.rs"]
mod tests;

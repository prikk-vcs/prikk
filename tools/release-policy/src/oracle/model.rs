use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub(crate) schema_version: String,
    pub(crate) python_baseline_commit: String,
    pub(crate) profile_contract_commit: String,
    pub(crate) observation_adapter_commit: String,
    pub(crate) reason_taxonomy_version: u64,
    pub(crate) reason_map: FileIdentity,
    pub(crate) normative_schema: FileIdentity,
    pub(crate) packs: Vec<PackIdentity>,
    pub(crate) cases: Vec<Case>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileIdentity {
    pub(crate) path: String,
    pub(crate) byte_length: u64,
    pub(crate) sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PackIdentity {
    pub(crate) pack_id: String,
    pub(crate) path: String,
    pub(crate) byte_length: u64,
    pub(crate) sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Case {
    pub(crate) suite_id: String,
    pub(crate) case_id: String,
    pub(crate) fixture_case_id: String,
    pub(crate) inputs: Vec<Input>,
    pub(crate) expected: Expected,
    #[serde(default)]
    pub(crate) sequence: Option<Vec<SequenceMember>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Input {
    pub(crate) role: String,
    pub(crate) ordinal: u64,
    pub(crate) location: Location,
    pub(crate) byte_length: u64,
    pub(crate) sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum Location {
    Direct { path: String },
    Packed { pack_id: String, entry_id: String },
}

impl Location {
    pub(crate) fn key(&self) -> String {
        match self {
            Self::Direct { path } => format!("direct:{path}"),
            Self::Packed { pack_id, entry_id } => format!("packed:{pack_id}:{entry_id}"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Expected {
    pub(crate) structural: String,
    pub(crate) semantic: String,
    #[serde(rename = "final")]
    pub(crate) final_: String,
    pub(crate) case_outcome: String,
    pub(crate) primary_reason: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SequenceMember {
    pub(crate) input_ordinal: u64,
    pub(crate) predecessor_name: Option<String>,
    pub(crate) current_name: String,
    pub(crate) byte_length: u64,
    pub(crate) sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Pack {
    pub(crate) schema_version: String,
    pub(crate) entries: Vec<PackEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PackEntry {
    pub(crate) entry_id: String,
    pub(crate) content: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObservationDocument {
    pub(crate) schema_version: String,
    pub(crate) python_baseline_commit: String,
    pub(crate) profile_contract_commit: String,
    pub(crate) cases: Vec<Observation>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Observation {
    pub(crate) suite_id: String,
    pub(crate) case_id: String,
    #[serde(rename = "final")]
    pub(crate) final_: String,
    pub(crate) case_outcome: String,
    pub(crate) input_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) structural: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) semantic: Option<String>,
}

impl Case {
    pub(crate) fn input_digest(&self) -> String {
        let mut digest = Sha256::new();
        for input in &self.inputs {
            digest.update(format!("ordinal={}\n", input.ordinal));
            digest.update(format!("role={}\n", input.role));
            match &input.location {
                Location::Direct { path } => {
                    digest.update(format!("location=direct:{path}\n"));
                }
                Location::Packed { pack_id, entry_id } => {
                    digest.update(format!("location=packed:{pack_id}:{entry_id}\n"));
                }
            }
            digest.update(format!("byte_length={}\n", input.byte_length));
            digest.update(format!("sha256={}\n", input.sha256));
        }
        // sha2 0.11's digest output has no `LowerHex` impl; hex-encoded byte-by-byte instead.
        digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

#[derive(Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CoverageInventory {
    pub(crate) schema_version: String,
    pub(crate) total_cases: u64,
    pub(crate) suites: Vec<CountEntry>,
    pub(crate) reason_counts: Vec<ReasonCount>,
    pub(crate) subjects: Vec<SubjectEntry>,
    pub(crate) transition_pairs: Vec<TransitionEntry>,
    pub(crate) repair_regressions: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CountEntry {
    pub(crate) suite_id: String,
    pub(crate) case_count: u64,
}

#[derive(Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReasonCount {
    pub(crate) primary_reason: String,
    pub(crate) case_count: u64,
}

#[derive(Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubjectEntry {
    pub(crate) subject: String,
    pub(crate) case_keys: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct TransitionEntry {
    pub(crate) case_key: String,
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) expected_valid: bool,
}

pub(crate) type ReasonMap = BTreeMap<String, String>;

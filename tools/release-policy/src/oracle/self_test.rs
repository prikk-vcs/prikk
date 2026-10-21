mod candidate;
mod matrix;
mod profile;
mod responsibility;

use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::Oracle;
use super::verify;
use crate::error::{Error, Result};
use crate::json;

const MANIFEST: &str = "release/oracle/oracle-manifest-v1.json";

pub(super) fn run(root: &Path, oracle: &Oracle) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    // RFC 119 track A: 154 -> 111 when the 43 post-1.0 signer cases were parked (not deleted; see
    // release/oracle/parked-cases-v1.json).
    // RFC 119 track B: 111 -> 57. release-state's 23 cases and json-parser/schema-evaluator's 15
    // were removed outright (NEVER); 16 of release-evidence's 73 were parked (LATER, same file).
    if oracle.manifest.cases.len() != 57 {
        errors.push("self-test:case-count".to_owned());
    }
    if oracle.inputs.len()
        != oracle
            .manifest
            .cases
            .iter()
            .map(|case| case.inputs.len())
            .sum::<usize>()
    {
        errors.push("self-test:input-closure".to_owned());
    }
    profile::run(root, &mut errors)?;
    responsibility::verify(root, &mut errors)?;
    repository_mutations(root, &mut errors)?;
    Ok(errors)
}

fn repository_mutations(root: &Path, errors: &mut Vec<String>) -> Result<()> {
    let original = read_json(root, MANIFEST)?;
    matrix::run(root, &original, errors)?;
    reject_manifest(root, &original, errors, "identity", |manifest| {
        *member_mut(manifest, "profile_contract_commit")? = json!("incorrect");
        Ok(())
    })?;
    reject_manifest(root, &original, errors, "duplicate-case", |manifest| {
        let cases = array_field_mut(manifest, "cases")?;
        let duplicate = cases
            .first()
            .cloned()
            .ok_or_else(|| Error::new("self-test manifest has no cases"))?;
        cases.insert(1, duplicate);
        Ok(())
    })?;
    reject_manifest(root, &original, errors, "direct-traversal", |manifest| {
        *member_mut(member_mut(direct_input_mut(manifest)?, "location")?, "path")? =
            json!("../outside");
        Ok(())
    })?;
    reject_manifest(
        root,
        &original,
        errors,
        "pack-registry-omission",
        |manifest| {
            array_field_mut(manifest, "packs")?.pop();
            Ok(())
        },
    )?;
    reject_manifest(root, &original, errors, "pack-hash", |manifest| {
        let pack = array_field_mut(manifest, "packs")?
            .first_mut()
            .ok_or_else(|| Error::new("self-test manifest has no packs"))?;
        *member_mut(pack, "sha256")? = json!("0".repeat(64));
        Ok(())
    })?;
    reject_manifest(root, &original, errors, "input-hash", |manifest| {
        *member_mut(packed_input_mut(manifest)?, "sha256")? = json!("0".repeat(64));
        Ok(())
    })?;
    reject_manifest(root, &original, errors, "packed-suite", |manifest| {
        *member_mut(
            member_mut(packed_input_mut(manifest)?, "location")?,
            "pack_id",
        )? = json!("wrong-suite");
        Ok(())
    })?;
    reject_manifest(root, &original, errors, "sequence-reuse", |manifest| {
        let sequence = sequence_mut(manifest)?;
        let first = sequence
            .first()
            .cloned()
            .ok_or_else(|| Error::new("self-test sequence is empty"))?;
        let second = sequence
            .get_mut(1)
            .ok_or_else(|| Error::new("self-test sequence lacks current member"))?;
        *second = first;
        Ok(())
    })?;
    reject_manifest(root, &original, errors, "sequence-name", |manifest| {
        let current = sequence_mut(manifest)?
            .get_mut(1)
            .ok_or_else(|| Error::new("self-test sequence lacks current member"))?;
        *member_mut(current, "current_name")? = json!("wrong.json");
        Ok(())
    })?;
    let temporary = candidate::create(root)?;
    fs::write(
        temporary.path().join("release/oracle/packs/extra.json"),
        "{}\n",
    )?;
    if verify::load(temporary.path()).is_ok() {
        errors.push("self-test:physical-pack-closure-not-rejected".to_owned());
    }

    mutate_auxiliary(
        root,
        &original,
        errors,
        "reason-map-closure",
        "release/oracle/reason-map-v1.json",
        |value| {
            let object = value
                .as_object_mut()
                .ok_or_else(|| Error::new("self-test reason map is not an object"))?;
            let key = object
                .keys()
                .next()
                .cloned()
                .ok_or_else(|| Error::new("self-test reason map is empty"))?;
            object.remove(&key);
            Ok(())
        },
        Some("reason_map"),
    )?;
    for (name, mutation) in [
        (
            "coverage-subject",
            mutate_coverage_subject as fn(&mut Value) -> Result<()>,
        ),
        ("coverage-transition", mutate_coverage_transition),
        ("coverage-repair", mutate_coverage_repair),
    ] {
        mutate_auxiliary(
            root,
            &original,
            errors,
            name,
            "release/oracle/coverage-inventory-v1.json",
            mutation,
            None,
        )?;
    }
    Ok(())
}

fn reject_manifest(
    root: &Path,
    original: &Value,
    errors: &mut Vec<String>,
    name: &str,
    mutation: impl FnOnce(&mut Value) -> Result<()>,
) -> Result<()> {
    let mut changed = original.clone();
    mutation(&mut changed)?;
    if verify::load_manifest(root, changed).is_ok() {
        errors.push(format!("self-test:{name}-not-rejected"));
    }
    Ok(())
}

fn mutate_auxiliary(
    root: &Path,
    original_manifest: &Value,
    errors: &mut Vec<String>,
    name: &str,
    path: &str,
    mutation: fn(&mut Value) -> Result<()>,
    identity_field: Option<&str>,
) -> Result<()> {
    let temporary = candidate::create(root)?;
    let mut value = read_json(temporary.path(), path)?;
    mutation(&mut value)?;
    let bytes = serde_json::to_vec_pretty(&value)?;
    fs::write(temporary.path().join(path), &bytes)?;
    let mut manifest = original_manifest.clone();
    if let Some(field) = identity_field {
        let identity = member_mut(&mut manifest, field)?;
        *member_mut(identity, "byte_length")? = json!(bytes.len());
        // sha2 0.11's digest output has no `LowerHex` impl; hex-encoded byte-by-byte instead.
        *member_mut(identity, "sha256")? = json!(
            Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        fs::write(
            temporary.path().join(MANIFEST),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
    }
    if verify::load(temporary.path()).is_ok() {
        errors.push(format!("self-test:{name}-not-rejected"));
    }
    Ok(())
}

fn direct_input_mut(manifest: &mut Value) -> Result<&mut Value> {
    input_with_location_kind(manifest, "direct")
}

fn packed_input_mut(manifest: &mut Value) -> Result<&mut Value> {
    input_with_location_kind(manifest, "packed")
}

fn input_with_location_kind<'a>(manifest: &'a mut Value, kind: &str) -> Result<&'a mut Value> {
    array_field_mut(manifest, "cases")?
        .iter_mut()
        .filter_map(|case| case.get_mut("inputs").and_then(Value::as_array_mut))
        .flatten()
        .find(|input| {
            input
                .get("location")
                .and_then(|location| string_field(location, "kind"))
                == Some(kind)
        })
        .ok_or_else(|| Error::new(format!("self-test {kind} input absent")))
}

fn sequence_mut(manifest: &mut Value) -> Result<&mut Vec<Value>> {
    array_field_mut(manifest, "cases")?
        .iter_mut()
        .find_map(|case| case.get_mut("sequence").and_then(Value::as_array_mut))
        .ok_or_else(|| Error::new("self-test sequence absent"))
}

fn mutate_coverage_subject(value: &mut Value) -> Result<()> {
    let subject = array_field_mut(value, "subjects")?
        .first_mut()
        .ok_or_else(|| Error::new("self-test coverage subjects empty"))?;
    array_field_mut(subject, "case_keys")?.pop();
    Ok(())
}

fn mutate_coverage_transition(value: &mut Value) -> Result<()> {
    let pairs = array_field_mut(value, "transition_pairs")?;
    let (first, rest) = pairs
        .split_first_mut()
        .ok_or_else(|| Error::new("self-test coverage transitions empty"))?;
    let second = rest
        .first_mut()
        .ok_or_else(|| Error::new("self-test coverage needs two transitions"))?;
    let first_key = member_mut(first, "case_key")?.clone();
    let second_key = member_mut(second, "case_key")?.clone();
    *member_mut(first, "case_key")? = second_key;
    *member_mut(second, "case_key")? = first_key;
    Ok(())
}

fn mutate_coverage_repair(value: &mut Value) -> Result<()> {
    array_field_mut(value, "repair_regressions")?.pop();
    Ok(())
}

fn member_mut<'a>(value: &'a mut Value, field: &str) -> Result<&'a mut Value> {
    value
        .as_object_mut()
        .and_then(|object| object.get_mut(field))
        .ok_or_else(|| Error::new(format!("self-test field absent: {field}")))
}

fn array_field_mut<'a>(value: &'a mut Value, field: &str) -> Result<&'a mut Vec<Value>> {
    member_mut(value, field)?
        .as_array_mut()
        .ok_or_else(|| Error::new(format!("self-test field is not an array: {field}")))
}

fn string_field<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field).and_then(Value::as_str)
}

fn read_json(root: &Path, path: &str) -> Result<Value> {
    json::parse(&fs::read(root.join(path))?)
        .map_err(|error| Error::new(format!("self-test JSON {path}: {error}")))
}

#[cfg(test)]
#[path = "self_test/tests.rs"]
mod tests;

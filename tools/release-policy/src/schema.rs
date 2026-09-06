use std::collections::HashSet;

use jsonschema::{Draft, Validator};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::time::parse_utc_second;

const KEYWORDS: &[&str] = &[
    "$schema",
    "$id",
    "$ref",
    "$defs",
    "title",
    "description",
    "type",
    "additionalProperties",
    "required",
    "properties",
    "const",
    "enum",
    "pattern",
    "items",
    "minItems",
    "maxItems",
    "uniqueItems",
    "allOf",
    "oneOf",
    "if",
    "then",
    "minLength",
    "minimum",
    "format",
];

pub(crate) struct SchemaProfile {
    validator: Validator,
}

impl SchemaProfile {
    pub(crate) fn compile(schema: &Value) -> Result<Self> {
        preflight(schema)?;
        jsonschema::meta::validate(schema)
            .map_err(|error| Error::new(format!("schema meta-validation failed: {error}")))?;
        let validator = jsonschema::options()
            .with_draft(Draft::Draft202012)
            .should_validate_formats(true)
            .should_ignore_unknown_formats(false)
            .with_format("date-time", |value| parse_utc_second(value).is_some())
            .build(schema)
            .map_err(|error| Error::new(format!("schema compilation failed: {error}")))?;
        Ok(Self { validator })
    }

    pub(crate) fn is_valid(&self, instance: &Value) -> bool {
        self.validator.is_valid(instance)
    }

    pub(crate) fn errors(&self, instance: &Value) -> Vec<String> {
        self.validator
            .iter_errors(instance)
            .map(|error| format!("{}: {error}", error.instance_path()))
            .collect()
    }
}

fn preflight(schema: &Value) -> Result<()> {
    let allowed: HashSet<&str> = KEYWORDS.iter().copied().collect();
    preflight_at(schema, "$", &allowed)
}

fn preflight_at(schema: &Value, path: &str, allowed: &HashSet<&str>) -> Result<()> {
    let object = schema
        .as_object()
        .ok_or_else(|| Error::new(format!("{path}: schema must be an object")))?;
    for name in object.keys() {
        if !allowed.contains(name.as_str()) {
            return Err(Error::new(format!(
                "{path}: unsupported schema keyword: {name}"
            )));
        }
    }
    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        if !reference.starts_with("#/") {
            return Err(Error::new(format!(
                "{path}: nonlocal schema reference is forbidden"
            )));
        }
    }
    for container in ["properties", "$defs"] {
        if let Some(children) = object.get(container).and_then(Value::as_object) {
            for (name, child) in children {
                preflight_at(child, &format!("{path}.{container}.{name}"), allowed)?;
            }
        }
    }
    for container in ["allOf", "oneOf"] {
        if let Some(children) = object.get(container).and_then(Value::as_array) {
            for (index, child) in children.iter().enumerate() {
                preflight_at(child, &format!("{path}.{container}[{index}]"), allowed)?;
            }
        }
    }
    for name in ["items", "if", "then"] {
        if let Some(child) = object.get(name) {
            preflight_at(child, &format!("{path}.{name}"), allowed)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "schema/tests.rs"]
mod tests;

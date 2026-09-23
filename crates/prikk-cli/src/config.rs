//! `prikk config` -- RFC 158 Stage A handoff §4: the smallest version of `prikk config` that is a
//! real one (RFC 135 §9.1a's "first real adopter" trigger). One key today, `incoming.max-object-bytes`
//! (§3's per-object bound). Built only for what this key needs, in a shape a second key does not
//! have to undo.
//!
//! **Format: hand-built, no new dependency.** One `key = value` per line; blank lines and `#`
//! comments are allowed. `RFC 135 §4` named `app-json-settings`/serde as a candidate; the architect
//! ruled a reader every import consults is attack surface, and one integer key does not justify a
//! dependency (a later key that needs structure reopens the question as its own
//! `ALLOWED_THIRD_PARTY` decision).
//!
//! **Strict, and fails closed:** an unknown key, a duplicate key, or a malformed value (zero,
//! negative, non-integer) each refuse, naming the line -- never a silent fall back to the default.
//!
//! **`.prikk/config`, never the worktree** -- a checked-out or imported file must not be able to
//! set a bound; nothing under `.prikk/` is ever written by `checkout` or `bundle import`/`sync
//! accept`, which is the whole of the guarantee this needs (checked at §3's own "the bound is
//! never taken from the input it bounds").
//!
//! **The file is itself read through the shared bounded reader** (`bounded_read`, §1) -- 64 KiB is
//! ample for a hand-built key=value file, and there is no reason this one file should be exempt
//! from the same discipline every other incoming read now has.
//!
//! **Concurrency, and exactly why it is fine today:** `set` writes the *whole* file from the one
//! key it was given, without reading the file first. With one key, two concurrent `set`s therefore
//! race only for which rename lands last, and the loser's value is simply replaced -- last-writer-
//! wins, exact, with no lost update to any *other* key because no other key exists. **A second key
//! changes this:** `set` would become a read-modify-write (read the other key, change one, write
//! all), two concurrent `set`s of different keys could then drop one another's change, and that
//! needs a lock and a race test. That is the work of the round that adds the second key, not this
//! one's -- do not extend `set` to a second key without it.

use std::path::PathBuf;

use prikk_store::RepositoryLayout;

use crate::bounded_read::{SizeBound, read_bounded_file};
use crate::commands::CliError;
use crate::stdout::println;

/// One key exists today. Adding a second means widening this list, not redesigning the format.
const KNOWN_KEYS: &[&str] = &[INCOMING_MAX_OBJECT_BYTES_KEY];
const INCOMING_MAX_OBJECT_BYTES_KEY: &str = "incoming.max-object-bytes";
const CONFIG_FILE_NAME: &str = "config";
/// A hand-built key=value file needs nowhere near this much room; ample headroom against a
/// malicious or merely mistaken file without any real cost.
const CONFIG_FILE_MAX_BYTES: usize = 64 * 1024;

fn config_path(layout: &RepositoryLayout) -> PathBuf {
    layout.prikk_dir().join(CONFIG_FILE_NAME)
}

/// Every non-blank, non-comment `key = value` line in `.prikk/config`, in file order. `Ok(vec![])`
/// when the file does not exist -- absence means every key takes its default, not an error.
/// Refuses, naming the line, on anything that is not `key = value`, an unknown key, or a key set
/// more than once.
fn read_config_lines(layout: &RepositoryLayout) -> Result<Vec<(String, String)>, CliError> {
    let path = config_path(layout);
    if std::fs::symlink_metadata(&path).is_err() {
        return Ok(Vec::new());
    }
    let bytes = read_bounded_file(
        "repository config",
        &path,
        &SizeBound::fixed(CONFIG_FILE_MAX_BYTES, "`.prikk/config`"),
    )?;
    let text = String::from_utf8(bytes)
        .map_err(|err| format!("`.prikk/config` is not valid utf-8: {err}"))?;
    let mut seen = std::collections::BTreeSet::new();
    let mut pairs = Vec::new();
    for (index, raw_line) in text.lines().enumerate() {
        let line_no = index + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!(
                "`.prikk/config` line {line_no}: {raw_line:?} is not `key = value`"
            )
            .into());
        };
        let key = key.trim().to_string();
        let value = value.trim().to_string();
        if !KNOWN_KEYS.contains(&key.as_str()) {
            return Err(format!("`.prikk/config` line {line_no}: unknown key {key:?}").into());
        }
        if !seen.insert(key.clone()) {
            return Err(format!(
                "`.prikk/config` line {line_no}: key {key:?} is set more than once"
            )
            .into());
        }
        pairs.push((key, value));
    }
    Ok(pairs)
}

fn parse_incoming_max_object_bytes(value: &str) -> Result<u64, String> {
    let parsed: u64 = value.parse().map_err(|_| {
        format!("{INCOMING_MAX_OBJECT_BYTES_KEY} must be a positive integer, got {value:?}")
    })?;
    if parsed == 0 {
        return Err(format!(
            "{INCOMING_MAX_OBJECT_BYTES_KEY} must be greater than zero, got 0"
        ));
    }
    Ok(parsed)
}

/// `incoming.max-object-bytes`'s value from `.prikk/config`, if the file sets it -- `None` when it
/// is absent or does not set this key. A malformed value refuses, naming the line, never a silent
/// fall back to the default (RFC 158 Stage A §3's own rule, restated here for the config path).
pub(crate) fn read_incoming_max_object_bytes(
    layout: &RepositoryLayout,
) -> Result<Option<u64>, CliError> {
    for (key, value) in read_config_lines(layout)? {
        if key == INCOMING_MAX_OBJECT_BYTES_KEY {
            let parsed = parse_incoming_max_object_bytes(&value)
                .map_err(|err| format!("`.prikk/config`: {err}"))?;
            return Ok(Some(parsed));
        }
    }
    Ok(None)
}

/// Resolve the effective per-object bound (RFC 158 Stage A §3's precedence, highest first):
/// `--max-object-bytes` on the command, else `incoming.max-object-bytes` in `.prikk/config` when a
/// repository is open, else `default`. `layout` is `None` for `bundle verify`, which has no
/// repository -- its own refusal must never suggest `prikk config` (§3's own rule), and passing
/// `None` here is what keeps that true without a second copy of the default's wording.
pub(crate) fn resolve_max_object_bytes(
    flag_value: Option<u64>,
    layout: Option<&RepositoryLayout>,
    default: usize,
) -> Result<SizeBound, CliError> {
    if let Some(bytes) = flag_value {
        return Ok(SizeBound::from_flag(bytes));
    }
    if let Some(layout) = layout {
        if let Some(bytes) = read_incoming_max_object_bytes(layout)? {
            return Ok(SizeBound::from_config(bytes));
        }
    }
    Ok(SizeBound::default_object_bound(default, layout.is_some()))
}

pub fn run_config(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let layout = crate::open_repository(root)?;
    let mut iter = args.into_iter();
    match iter.next().as_deref() {
        Some("get") => run_get(&layout, iter.collect()),
        Some("set") => run_set(&layout, iter.collect()),
        Some("unset") => run_unset(&layout, iter.collect()),
        Some("list") => run_list(&layout, iter.collect()),
        Some(other) => Err(CliError::Usage(format!(
            "unknown config subcommand: {other} (expected get, set, unset, or list)"
        ))),
        None => Err(CliError::Usage(
            "config requires a subcommand: get, set, unset, or list".to_string(),
        )),
    }
}

fn require_key_arg(
    mut iter: std::vec::IntoIter<String>,
    command: &str,
) -> Result<String, CliError> {
    let Some(key) = iter.next() else {
        return Err(CliError::Usage(format!("{command} requires a key")));
    };
    if let Some(extra) = iter.next() {
        return Err(CliError::Usage(format!(
            "unknown {command} argument: {extra}"
        )));
    }
    Ok(key)
}

fn known_key_or_refuse(key: &str) -> Result<(), CliError> {
    if KNOWN_KEYS.contains(&key) {
        Ok(())
    } else {
        Err(CliError::Usage(format!("unknown config key: {key}")))
    }
}

fn run_get(layout: &RepositoryLayout, args: Vec<String>) -> Result<(), CliError> {
    let key = require_key_arg(args.into_iter(), "config get")?;
    known_key_or_refuse(&key)?;
    print_effective_value(layout, &key)
}

fn run_list(layout: &RepositoryLayout, args: Vec<String>) -> Result<(), CliError> {
    if let Some(extra) = args.into_iter().next() {
        return Err(CliError::Usage(format!(
            "unknown config list argument: {extra}"
        )));
    }
    for key in KNOWN_KEYS {
        print_effective_value(layout, key)?;
    }
    Ok(())
}

fn print_effective_value(layout: &RepositoryLayout, key: &str) -> Result<(), CliError> {
    debug_assert_eq!(
        key, INCOMING_MAX_OBJECT_BYTES_KEY,
        "only one key exists today"
    );
    match read_incoming_max_object_bytes(layout)? {
        Some(value) => println!("{key} = {value} (`.prikk/config`)"),
        None => println!(
            "{key} = {} (default)",
            prikk_store::DEFAULT_BUNDLE_MAX_OBJECT_BYTES
        ),
    }
    Ok(())
}

fn run_set(layout: &RepositoryLayout, args: Vec<String>) -> Result<(), CliError> {
    let mut iter = args.into_iter();
    let Some(key) = iter.next() else {
        return Err(CliError::Usage(
            "config set requires a key and a value".to_string(),
        ));
    };
    let Some(value) = iter.next() else {
        return Err(CliError::Usage("config set requires a value".to_string()));
    };
    if let Some(extra) = iter.next() {
        return Err(CliError::Usage(format!(
            "unknown config set argument: {extra}"
        )));
    }
    known_key_or_refuse(&key)?;
    // Validated before anything is written -- an invalid value must never reach disk.
    parse_incoming_max_object_bytes(&value).map_err(CliError::from)?;
    write_config_file(layout, &format!("{key} = {value}\n"))?;
    println!("{key} = {value}");
    Ok(())
}

fn run_unset(layout: &RepositoryLayout, args: Vec<String>) -> Result<(), CliError> {
    let key = require_key_arg(args.into_iter(), "config unset")?;
    known_key_or_refuse(&key)?;
    let path = config_path(layout);
    if std::fs::symlink_metadata(&path).is_ok() {
        std::fs::remove_file(&path)
            .map_err(|err| format!("failed to remove {}: {err}", path.display()))?;
    }
    println!("{key} unset (default restored)");
    Ok(())
}

fn write_config_file(layout: &RepositoryLayout, content: &str) -> Result<(), CliError> {
    let path = config_path(layout);
    let parent = path
        .parent()
        .ok_or_else(|| CliError::from("`.prikk/config` has no parent directory".to_string()))?;
    std::fs::create_dir_all(parent)
        .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    crate::durable_output::write_new_file_durably(&path, content.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests;

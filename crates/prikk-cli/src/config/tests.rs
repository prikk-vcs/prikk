//! RFC 158 Stage A handoff §6 controls 10 and 11, run in-process against a real temp repository --
//! `RepositoryLayout::init` needs no subprocess to exercise `run_config`'s own parsing and
//! validation.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::PathBuf;

use prikk_store::RepositoryLayout;

use super::*;

fn temp_repo(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "prikk-cli-rfc158-config-{tag}-{}",
        std::process::id()
    ));
    dir.push(format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    RepositoryLayout::init(dir.clone()).unwrap();
    dir
}

/// Control 10: `set`/`get`/`unset`/`list` round-trip, and the file lands in `.prikk/`, never the
/// worktree.
#[test]
fn set_get_unset_list_round_trip_and_the_file_lands_in_dot_prikk() {
    let root = temp_repo("round-trip");
    let layout = RepositoryLayout::open(&root).unwrap();

    // Default before anything is set.
    assert_eq!(read_incoming_max_object_bytes(&layout).unwrap(), None);

    run_config(
        root.clone(),
        vec![
            "set".to_string(),
            INCOMING_MAX_OBJECT_BYTES_KEY.to_string(),
            "12345".to_string(),
        ],
    )
    .unwrap();
    assert_eq!(
        read_incoming_max_object_bytes(&layout).unwrap(),
        Some(12345)
    );

    let config_file = root.join(".prikk").join(CONFIG_FILE_NAME);
    assert!(config_file.exists(), "the config file lands under .prikk/");
    assert!(
        !root.join(CONFIG_FILE_NAME).exists(),
        "the config file must never land in the worktree, at the repository root"
    );

    run_config(
        root.clone(),
        vec!["get".to_string(), INCOMING_MAX_OBJECT_BYTES_KEY.to_string()],
    )
    .unwrap();
    run_config(root.clone(), vec!["list".to_string()]).unwrap();

    run_config(
        root.clone(),
        vec![
            "unset".to_string(),
            INCOMING_MAX_OBJECT_BYTES_KEY.to_string(),
        ],
    )
    .unwrap();
    assert_eq!(read_incoming_max_object_bytes(&layout).unwrap(), None);
    assert!(
        !config_file.exists(),
        "unset removes the file entirely when it held only the one key"
    );
}

/// Control 11, unknown key: refuses, naming the key, never falling back to the default.
#[test]
fn an_unknown_key_refuses() {
    let root = temp_repo("unknown-key");
    let result = run_config(
        root,
        vec![
            "set".to_string(),
            "not.a.real.key".to_string(),
            "1".to_string(),
        ],
    );
    let err = result.unwrap_err();
    assert!(
        err.message().contains("unknown config key"),
        "{}",
        err.message()
    );
}

/// Control 11, an unknown key written directly into the file (bypassing `config set`'s own
/// validation): refuses, naming the line -- never silently skipped. *Perturbed by hand (§7 item
/// 5): letting the reader `continue` past an unrecognized key instead of refusing makes this go
/// red -- confirmed by hand and reverted, see the report.*
#[test]
fn an_unknown_key_in_the_file_refuses_naming_the_line() {
    let root = temp_repo("unknown-key-in-file");
    let layout = RepositoryLayout::open(&root).unwrap();
    std::fs::write(config_path(&layout), "not.a.real.key = 5\n").unwrap();
    let err = read_incoming_max_object_bytes(&layout).unwrap_err();
    assert!(
        err.message().contains("line 1") && err.message().contains("unknown key"),
        "{}",
        err.message()
    );
}

/// Control 11, a duplicate key in the file: refuses, naming the line.
#[test]
fn a_duplicate_key_in_the_file_refuses_naming_the_line() {
    let root = temp_repo("duplicate-key");
    let layout = RepositoryLayout::open(&root).unwrap();
    std::fs::write(
        config_path(&layout),
        format!("{INCOMING_MAX_OBJECT_BYTES_KEY} = 100\n{INCOMING_MAX_OBJECT_BYTES_KEY} = 200\n"),
    )
    .unwrap();
    let err = read_incoming_max_object_bytes(&layout).unwrap_err();
    assert!(
        err.message().contains("line 2") && err.message().contains("more than once"),
        "{}",
        err.message()
    );
}

/// Control 11, a zero value: refuses, never a silent fall back to the default.
#[test]
fn a_zero_value_refuses() {
    let root = temp_repo("zero-value");
    let layout = RepositoryLayout::open(&root).unwrap();
    std::fs::write(
        config_path(&layout),
        format!("{INCOMING_MAX_OBJECT_BYTES_KEY} = 0\n"),
    )
    .unwrap();
    let err = read_incoming_max_object_bytes(&layout).unwrap_err();
    assert!(
        err.message().contains("greater than zero"),
        "{}",
        err.message()
    );
}

/// Control 11, a non-integer value: refuses, naming what was found.
#[test]
fn a_non_integer_value_refuses() {
    let root = temp_repo("non-integer");
    let layout = RepositoryLayout::open(&root).unwrap();
    std::fs::write(
        config_path(&layout),
        format!("{INCOMING_MAX_OBJECT_BYTES_KEY} = not-a-number\n"),
    )
    .unwrap();
    let err = read_incoming_max_object_bytes(&layout).unwrap_err();
    assert!(
        err.message().contains("must be a positive integer"),
        "{}",
        err.message()
    );
}

/// Control 11, an oversized config file: refuses -- it is read through the same bounded reader
/// every incoming artifact is, and 64 KiB is nowhere near enough room for this to be a real file.
#[test]
fn an_oversized_config_file_refuses() {
    let root = temp_repo("oversized");
    let layout = RepositoryLayout::open(&root).unwrap();
    let oversized = "#".repeat(CONFIG_FILE_MAX_BYTES + 1);
    std::fs::write(config_path(&layout), oversized).unwrap();
    let err = read_incoming_max_object_bytes(&layout).unwrap_err();
    assert!(err.message().contains("over"), "{}", err.message());
}

/// Comments and blank lines are ignored, never treated as an unknown key.
#[test]
fn comments_and_blank_lines_are_ignored() {
    let root = temp_repo("comments");
    let layout = RepositoryLayout::open(&root).unwrap();
    std::fs::write(
        config_path(&layout),
        format!("# a comment\n\n{INCOMING_MAX_OBJECT_BYTES_KEY} = 999\n"),
    )
    .unwrap();
    assert_eq!(read_incoming_max_object_bytes(&layout).unwrap(), Some(999));
}

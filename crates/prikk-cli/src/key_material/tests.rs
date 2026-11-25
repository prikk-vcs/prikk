//! Distinct default key ids, addendum 2 (F2): the key-id file is written before the seed, so a crash
//! between the two writes leaves a loud state rather than a seed silently on the legacy id.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::PathBuf;

use super::{
    KeyIdSource, Role, SeedSource, Unusable, derived_key_id, fail_next_seed_write_for_test,
    key_id_path, status_at, write_new_key,
};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "prikk-key-material-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos())
    ));
    std::fs::create_dir_all(&dir)
        .map_err(|err| err.to_string())
        .ok();
    dir
}

fn write_seed_file(
    seed: &[u8; prikk_crypto::ED25519_KEY_LEN],
    path: &std::path::Path,
) -> Result<(), crate::commands::CliError> {
    std::fs::write(path, format!("{}\n", prikk_hash::to_hex(seed)))
        .map_err(|err| crate::commands::CliError::Failure(err.to_string()))
}

#[test]
fn a_crash_after_the_key_id_file_leaves_a_missing_seed_not_the_legacy_id() {
    // The resolution reads PRIKK_AUTHOR_KEY_ID first; with it set, the key-id file is never consulted
    // and this control would prove nothing.
    assert!(
        std::env::var(Role::Author.key_id_var()).is_err(),
        "run without PRIKK_AUTHOR_KEY_ID set"
    );
    let dir = scratch("crash");
    let seed_path = dir.join("author.seed");
    let seed = [0x5c_u8; prikk_crypto::ED25519_KEY_LEN];

    fail_next_seed_write_for_test();
    let crashed = write_new_key(
        &seed,
        &seed_path,
        || Ok(()),
        || write_seed_file(&seed, &seed_path),
    );
    assert!(crashed.is_err(), "the seam fails the seed write");
    assert!(
        key_id_path(&seed_path).exists(),
        "the key-id file was written first"
    );
    assert!(!seed_path.exists(), "no seed was written");

    let status = status_at(Role::Author, SeedSource::Override, seed_path.clone());
    let Ok(status) = status else {
        panic!("status answers");
    };
    assert!(
        matches!(status.seed, Err(Unusable::OverrideMissing)),
        "the seed reports missing"
    );
    assert_eq!(
        status.key_id_source,
        KeyIdSource::KeyFile(key_id_path(&seed_path)),
        "never the legacy default"
    );
    assert_eq!(status.key_id, derived_key_id(&seed));

    let retry = write_new_key(
        &seed,
        &seed_path,
        || Ok(()),
        || write_seed_file(&seed, &seed_path),
    );
    let Err(crate::commands::CliError::Failure(message)) = retry else {
        panic!("a retry refuses");
    };
    assert!(
        message.contains(&seed_path.display().to_string())
            && message.contains(&key_id_path(&seed_path).display().to_string()),
        "the retry names both paths: {message}"
    );
    assert!(!seed_path.exists(), "the refused retry writes no seed");
    let _ = std::fs::remove_dir_all(dir);
}

// ---- 0.59.0 K6: a key file is published whole and never over one that is there; a torn one is told from a whole or a different one ----------------

use super::{interrupted_key_sentence, is_part_of_a_key_id, is_part_of_a_seed, publish_new_file};

#[test]
fn a_published_file_is_whole_and_a_file_that_is_there_is_never_replaced() {
    let dir = scratch("publish");
    let path = dir.join("author.seed");
    publish_new_file(&path, b"first\n").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"first\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    // A second publication over it is refused, the file is as it was, and no temporary is left.
    let Err(crate::commands::CliError::Failure(message)) = publish_new_file(&path, b"second\n")
    else {
        panic!("a publication over an existing file must refuse");
    };
    assert!(
        message.contains("refusing to overwrite an existing file"),
        "{message}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"first\n");
    let names: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
    assert_eq!(
        names.len(),
        1,
        "the refused publication left a temporary: {names:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn what_is_part_of_a_seed_or_a_key_id_and_what_is_not() {
    let whole_seed = "a".repeat(64);
    assert!(
        is_part_of_a_seed("") && is_part_of_a_seed("ab12") && is_part_of_a_seed(&whole_seed[..63])
    );
    assert!(!is_part_of_a_seed(&whole_seed) && !is_part_of_a_seed(&format!("{whole_seed}\n")));
    assert!(
        !is_part_of_a_seed("not hex at all"),
        "a damaged file is not a torn one"
    );
    let id = "ed25519-0123456789abcdef";
    for cut in 0..id.len() {
        assert!(is_part_of_a_key_id(&id[..cut]), "{cut}");
    }
    assert!(
        !is_part_of_a_key_id(id) && !is_part_of_a_key_id(&format!("{id}\n")),
        "the id without its newline is whole enough"
    );
    assert!(!is_part_of_a_key_id("my-own-id\n") && !is_part_of_a_key_id("ed25519-0123XYZ"));
}

#[test]
fn the_interrupted_sentence_names_what_to_remove_and_is_silent_for_a_whole_pair() {
    let dir = scratch("sentence");
    let seed_path = dir.join("author.seed");
    let seed = [0x42_u8; prikk_crypto::ED25519_KEY_LEN];
    assert!(
        interrupted_key_sentence(&seed_path).is_none(),
        "nothing there"
    );
    write_new_key(
        &seed,
        &seed_path,
        || Ok(()),
        || write_seed_file(&seed, &seed_path),
    )
    .unwrap();
    assert!(
        interrupted_key_sentence(&seed_path).is_none(),
        "a whole pair"
    );
    // The seed cut short: the sentence names both files of the pair.
    std::fs::write(&seed_path, "abcd").unwrap();
    let sentence = interrupted_key_sentence(&seed_path).unwrap();
    assert!(
        sentence.contains("holds 4 bytes, not a whole seed"),
        "{sentence}"
    );
    assert!(
        sentence.contains(&format!(
            "remove `{}` and `{}`",
            seed_path.display(),
            key_id_path(&seed_path).display()
        )),
        "{sentence}"
    );
    // No seed and a whole key id: an interrupted creation, one file to remove.
    std::fs::remove_file(&seed_path).unwrap();
    let sentence = interrupted_key_sentence(&seed_path).unwrap();
    assert!(
        sentence.contains("is here and")
            && sentence.contains(&format!("remove `{}` (", key_id_path(&seed_path).display())),
        "{sentence}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

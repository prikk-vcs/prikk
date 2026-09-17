//! Distinct default key ids, addendum 2 (F2): the key-id file is written before the seed, so a crash
//! between the two writes leaves a loud state rather than a seed silently on the legacy id.

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

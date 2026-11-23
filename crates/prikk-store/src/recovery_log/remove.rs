//! 0.59.0 step G2d: the recovery log's `Remove` entry -- a whole file a repair removed.

use super::*;

/// Save a whole file a repair is about to remove (0.59.0 step G2d). `source` is relative to `.prikk/`; `bytes` are the file's bytes now.
/// Call it before the removal.
pub(crate) fn save_remove(
    layout: &RepositoryLayout,
    source: &str,
    bytes: &[u8],
    label: &str,
) -> Result<RecoveryRef> {
    let entry = Entry {
        kind: Kind::Remove,
        run: [0; 8],
        source: source.to_string(),
        offset: 0,
        label: label.to_string(),
        binary_version: env!("CARGO_PKG_VERSION").to_string(),
        prefix_hash: sha(bytes),
        meaning: Vec::new(),
        removed: bytes.to_vec(),
        new_hash: [0; 32],
    };
    append(layout.repository_mutation_root(), &entry)
}

/// Whether `source` is a ref pointer candidate path (`refs/tmp/<name>`, one component): the one place a restore may recreate a removed file,
/// since the candidates have no fixed names ([`repairable_sources`] cannot list them).
pub(crate) fn is_ref_candidate_source(source: &str) -> bool {
    source.strip_prefix("refs/tmp/").is_some_and(|name| {
        !name.is_empty()
            && !name.contains('/')
            && !name.contains('\\')
            && name != "."
            && name != ".."
    })
}

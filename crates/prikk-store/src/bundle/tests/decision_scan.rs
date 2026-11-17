//! 0.58.0 B′b: the **secondary** witness that `bundle/decision.rs` writes nothing -- a scan of its production text for the names of the
//! repository's writers. **Its limit, stated:** it matches *names*. A writer this list does not name, or one reached through a helper
//! the decision already calls, passes it ("classify by context, not by spelling"). The primary witness is behavioural: the repository is
//! byte-identical after a preview, for every refusal kind and the clean path (`prikk-cli`'s `b_prime_one_decision.rs`,
//! `a_preview_writes_nothing_for_any_row`). **Red control:** add any listed name to `decision.rs` and this goes red.

/// The writers (and lock acquirers) a decision must never name.
const WRITERS: &[&str] = &[
    "write_received_pointer",
    "record_author_key_material",
    "ObjectWriteSession",
    "write_object",
    "append_",
    "publish",
    "ActiveLock",
    "acquire_container_locks",
    "acquire_for_write",
    "write_file_atomically",
    "create_new_file",
    "remove_file",
    "std::fs::write",
    "OpenOptions",
    "MaintainerSigner",
    "maintainer_signature",
];

#[test]
fn the_decision_names_no_writer() {
    let source = include_str!("../decision.rs");
    // Production text only: comments (which name the writers in prose, to say the decision does not use them) are dropped.
    let production: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let found: Vec<&&str> = WRITERS
        .iter()
        .filter(|name| production.contains(**name))
        .collect();
    assert!(
        found.is_empty(),
        "bundle/decision.rs names a writer: {found:?} -- the decision reads only (see its module doc)"
    );
}

// SPDX-License-Identifier: Apache-2.0
//! **The writer inventory (K3 ruling 6).** Every top-level verb of the CLI, each either swept (by tests that exist in `real_kill_sweep.rs`) or named with
//! the reason it is not. It is checked against the CLI's own verb list (`prikk --help`), so **a new verb fails this test until it is placed**.
//!
//! A `NotSwept` reason is a statement, not a promise: where the reason is "not measured", it says so, and the verb is listed under the crash-points
//! row of the ROADMAP rather than left out.

pub enum Status {
    /// Swept in full: the verb's writing paths (those named) have their tests.
    Swept(&'static [&'static str]),
    /// Some of the verb's paths are swept (the tests), the rest are not (the reason).
    Partly(&'static [&'static str], &'static str),
    /// Not swept, and why.
    NotSwept(&'static str),
}

use Status::{NotSwept, Partly, Swept};

const READS: &str = "reads: it writes no history, ref, queue or worktree byte; like the W2 refusal it may take a lock or rewrite a derived cache, whose kill states (a stale lock, a temp file) the sweep measures there (classes L, F and the leftover-temp count) and not again per read command";
const OUTPUT_ONLY: &str = "not measured: it writes a file the user names, outside `.prikk`, and may take a lock; a kill leaves a partial output file, which the durable-output tests (`dc44_bundle_export_durability`, `durable_output`) hold";

pub const VERBS: &[(&str, Status)] = &[
    (
        "setup",
        NotSwept(
            "measured and **unsound at 271 of 273 ordinals** (K5): `setup` runs `init` (K4: sound), then writes the author and maintainer seeds and their key-id files, then adopts the maintainer key. A kill leaves a `.prikk`, and a rerun of `setup` refuses (`already holds a repository`) naming `prikk trust maintainer add`, which needs `--key-id` and a public key from `prikk key public`: **before ordinal ~234 no key exists yet, so nothing the sentence names can work** (a killed `init` is named by `status`, `prikk init`, which does not make keys); from ~234 the key files are written by create-then-write, so a kill leaves an empty or part-written seed or key-id (`maintainer.seed` of 0 bytes at ordinal 242). The way out is deleting `.prikk` and the key files and running `setup` again, which nothing says. The shape is `setup` in `shapes.rs` (with the key directory restored before every run), ignored until a ruling",
        ),
    ),
    ("init", Swept(&["init"])),
    (
        "key",
        NotSwept(
            "`key generate --out` measured and **unsound at 5 of 14 ordinals** (K5): the seed file and its `.key-id` file are created, then written, so a kill leaves an empty or part-written one, and the rerun refuses (`a new seed needs both <out> and <out>.key-id to be absent`) naming no command (nothing was issued: the seed is never printed with `--out`); `key generate` without `--out` prints and writes nothing, `key public` and `key status` read. The shape is `key_generate`, ignored until a ruling",
        ),
    ),
    (
        "config",
        NotSwept("not measured: writes one setting, and no sweep has been built for it"),
    ),
    ("status", NotSwept(READS)),
    ("worktree-status", NotSwept(READS)),
    ("commit", Swept(&["commit"])),
    (
        "uncommit",
        Swept(&["uncommit", "recovery_restore_of_uncommit"]),
    ),
    (
        "mv",
        NotSwept("not measured: one append of a rename declaration"),
    ),
    ("seal", Swept(&["seal", "seal_checkpoint"])),
    ("log", NotSwept(READS)),
    ("show", NotSwept(READS)),
    ("diff", NotSwept(READS)),
    ("tree", NotSwept(READS)),
    ("cat", NotSwept(OUTPUT_ONLY)),
    ("extract", NotSwept(OUTPUT_ONLY)),
    (
        "checkout",
        NotSwept(
            "not measured: writes the worktree and the switch marker (the marker's crash has failpoint rows)",
        ),
    ),
    (
        "branch",
        Partly(
            &[
                "branch_create",
                "switch_refused_w2",
                "w3_branch_create_refusal",
            ],
            "`branch switch` and `branch close` publishing paths are not measured (the switch marker has failpoint rows)",
        ),
    ),
    ("tag", Swept(&["tag_create"])),
    ("ref", Swept(&["ref_complete"])),
    (
        "bundle",
        Partly(
            &["w3_bundle_adopt_refusal"],
            "`bundle import` killed mid-import has its own real-kill row; `bundle export` is an output file",
        ),
    ),
    (
        "archive",
        Partly(
            &["cancel_import", "w3_archive_adopt_refusal"],
            "`archive import` end to end has its own real-kill row; `archive export` is an output file",
        ),
    ),
    (
        "sync",
        Partly(
            &["sync_seal_batch"],
            "`sync accept`, `sync adopt-tag` and the rest are not measured; `sync seal` rows have their own real-kill tests",
        ),
    ),
    (
        "received",
        NotSwept("not measured: removes a received ref's pointer record"),
    ),
    (
        "trust",
        NotSwept("not measured: appends to the trust policy and its generation log"),
    ),
    ("merge-plan", NotSwept(READS)),
    ("merge-evidence", NotSwept(READS)),
    (
        "merge",
        Partly(
            &["w3_merge_refusal"],
            "the publishing path of `merge` is not measured",
        ),
    ),
    ("inverse-plan", NotSwept(READS)),
    ("rollback-preview", NotSwept(READS)),
    (
        "rollback-draft",
        NotSwept("not measured: queues a patch (the append the `commit` sweep measures)"),
    ),
    ("rollback-draft-verify", NotSwept(READS)),
    ("verify", NotSwept(READS)),
    (
        "doctor",
        Swept(&[
            "repair_wal_tail",
            "repair_tails",
            "repair_pointer_index_tail",
            "repair_index",
            "rebuild_pointer_index",
            "rebuild_caches",
            "discard_damaged_commits",
            "restore_queue_target",
            "remove_ref_candidates",
            "recovery_restore_cut",
            "recovery_restore_remove",
            "recovery_clear",
            "cancel_import",
            "recovery_restore_of_uncommit",
            "recovery_restore_of_discard",
            "recovery_restore_of_rebuild_shortened_slot",
            "recovery_restore_of_restore_queue_target",
        ]),
    ),
    (
        "unlock",
        NotSwept(
            "not measured: removes lock files, the one write the sweep's own recovery uses at every ordinal",
        ),
    ),
    (
        "compact",
        Swept(&[
            "compact_pointer_index",
            "compact_received_index",
            "compact_trust_policy",
        ]),
    ),
    (
        "format",
        Swept(&["format_upgrade", "format_upgrade_lost_marker"]),
    ),
];

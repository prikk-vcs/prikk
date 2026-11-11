//! The command registry — RFC 118 stage 1: one declaration of prikk's dispatchable command
//! surface, so `main.rs::run`'s dispatch and `output::help::print_help`'s rendering both *derive*
//! from `COMMANDS` instead of each restating the command list by hand. Meta-arms (`--help`, `-h`,
//! `--version`, `-V`, and the argument-less case) are not commands and stay outside this table,
//! per the RFC 118 prerequisite ruling §1.
//!
//! `help_lines` holds each command's `--help` output pre-formatted, verbatim, exactly as it must
//! appear on the line -- not decomposed into separate form/summary fields with alignment computed
//! at render time. The existing column alignment is hand-tuned per line, not a single fixed-width
//! formula (compare `init`'s ~43-column gap against `trust maintainer add`'s 2-space gap once its
//! own form already overruns that column), and re-deriving it algorithmically risked exactly the
//! "improve the output to make rendering easier" regression the handoff forbids. Storing the
//! already-correct text still removes all command-specific text from `help.rs` itself, which
//! becomes a pure iterate-and-print renderer holding no literal command text of its own -- the
//! requirement the handoff actually states, even though the shape differs from its suggested
//! `Usage { form, summary }` split.

/// RFC 121 §6a's ruled exit-code contract: `Usage` maps to `2` (detected before any repository
/// work begins -- unknown argument, missing required flag, malformed flag value, duplicate flag,
/// or an unrecognized command name), `Failure` maps to `1` (everything else a command refuses to
/// do). **No fourth variant** -- `verify --format json` already carries a structured,
/// three-valued verdict; collapsing it further into an exit code is what RFC 118 exists to
/// prevent.
///
/// `From<String>` defaults to `Failure`, matching every error this crate already produces today
/// (all exit `1` before this contract existed) -- so changing `Command.run`'s return type to this
/// enum is a pure plumbing change here: no command's *behaviour* changes until a call site
/// deliberately constructs `Usage` instead of relying on this default.
#[derive(Debug)]
pub(crate) enum CliError {
    Usage(String),
    Failure(String),
}

impl CliError {
    pub(crate) fn message(&self) -> &str {
        match self {
            CliError::Usage(message) | CliError::Failure(message) => message,
        }
    }

    pub(crate) fn exit_code(&self) -> u8 {
        match self {
            CliError::Usage(_) => 2,
            CliError::Failure(_) => 1,
        }
    }
}

impl From<String> for CliError {
    fn from(message: String) -> Self {
        CliError::Failure(message)
    }
}

/// The groups of the top-level help, in the order it prints them: the order of a day's work (0.57.0 item 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Group {
    Start,
    Record,
    Read,
    Branches,
    Share,
    Trust,
    Merge,
    Undo,
    Check,
}

impl Group {
    /// Every group, in print order.
    pub(crate) const ALL: [Group; 9] = [
        Group::Start,
        Group::Record,
        Group::Read,
        Group::Branches,
        Group::Share,
        Group::Trust,
        Group::Merge,
        Group::Undo,
        Group::Check,
    ];

    /// The heading the top-level help prints.
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Group::Start => "Start",
            Group::Record => "Record your work",
            Group::Read => "Read history and files",
            Group::Branches => "Branches and tags",
            Group::Share => "Share with others",
            Group::Trust => "Trust",
            Group::Merge => "Merge",
            Group::Undo => "Undo sealed work",
            Group::Check => "Check and repair",
        }
    }
}

pub(crate) struct Command {
    pub(crate) name: &'static str,
    /// The group the top-level help lists this command under (0.57.0 item 6). An enum, so a misspelt group cannot compile.
    pub(crate) group: Group,
    /// One line, true of the whole command, that the top-level help shows beside the name. The full synopses stay in
    /// `help_lines`, which `prikk <command> --help` (and `prikk help <command>`) prints.
    pub(crate) summary: &'static str,
    pub(crate) run: fn(Vec<String>) -> std::result::Result<(), CliError>,
    pub(crate) help_lines: &'static [&'static str],
}

/// `init` is the one dispatch arm with a non-`Vec<String>` signature ([`Option<String>`], a single
/// optional path) -- adapted here so the table's `run` field stays uniform, per the prerequisite
/// ruling's "one adapter closure each." RFC 121 §3: this used to silently discard everything past
/// the first positional (`prikk init a b c` created a repo at `a` with no complaint); now anything
/// beyond that one optional path is refused.
fn run_init_adapter(args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut iter = args.into_iter();
    let path = iter.next();
    if let Some(extra) = iter.next() {
        return Err(CliError::Usage(format!("unknown init argument: {extra}")));
    }
    crate::run_init(path)
}

/// `status` is the other non-uniform arm: no arguments at all except RFC 140's `--format json`.
/// RFC 121 §3: this used to accept and silently ignore any argument (`prikk status --nonsense`
/// exited `0`); now any argument other than `--format json` is refused, and `--format` itself
/// accepts no other value -- the same pattern RFC 138's `trust maintainer list`/`check` already
/// use (`main.rs`'s own `--format` arms), copied rather than re-derived (RFC 140 §2).
fn run_status_adapter(args: Vec<String>) -> std::result::Result<(), CliError> {
    use crate::arg_scan::{flag_value, mark_seen};

    let mut format_json = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--format" => {
                let value = flag_value(&mut iter, "status --format")?;
                if value != "json" {
                    return Err(CliError::Usage(format!(
                        "status --format does not support {value:?}"
                    )));
                }
                mark_seen(&mut format_json, "--format")?;
            }
            other => return Err(CliError::Usage(format!("unknown status argument: {other}"))),
        }
    }
    crate::run_status(format_json)
}

/// Order here is the `--help` rendering order (`output::help::print_help` iterates this table
/// directly) -- it is **not** the old `main.rs` match-arm order, which was itself never the order
/// `help.rs` printed in. Dispatch is a name lookup, so table order does not affect it.
pub(crate) const COMMANDS: &[Command] = &[
    Command {
        name: "setup",
        group: Group::Start,
        summary: "Create a repository and set up your signing keys",
        run: crate::run_setup,
        help_lines: &[
            "  prikk setup [repo-path] [--author-seed-out <path>] [--maintainer-seed-out <path>]  Init, put both keys in your key directory (created, or reused if already there), and trust the maintainer key",
        ],
    },
    Command {
        name: "init",
        group: Group::Start,
        summary: "Create an empty repository",
        run: run_init_adapter,
        help_lines: &[
            "  prikk init [path]                         Create a .prikk repository layout",
        ],
    },
    Command {
        name: "key",
        group: Group::Start,
        summary: "Generate a key, show its public half, check signing",
        run: crate::run_key,
        help_lines: &[
            "  prikk key generate [--out <path>]         Generate an Ed25519 seed; --out writes it (mode 0600) and never prints it",
            "  prikk key public [--seed-file <path>] [--role author|maintainer]  Derive a public key from a seed file (default: the role's own file in your key directory)",
            "  prikk key status [path] [--role author|maintainer] [--format json]  Report whether this machine can sign, and with which key -- reads only, signs nothing",
        ],
    },
    Command {
        name: "config",
        group: Group::Start,
        summary: "Read and set settings, and see where each comes from",
        run: crate::run_config,
        help_lines: &[
            "  prikk config get <key>                    Print one key's effective value and where it came from",
            "  prikk config set <key> <value>             Validate and durably write one key",
            "  prikk config unset <key>                   Remove one key (its default applies again)",
            "  prikk config list                          Print every known key, its effective value, and its source",
            "  note: the only key today is incoming.max-object-bytes (RFC 158 Stage A); the file lives at .prikk/config, never the worktree",
        ],
    },
    Command {
        name: "status",
        group: Group::Record,
        summary: "Show the repository and what is waiting to be sealed",
        run: run_status_adapter,
        help_lines: &[
            "  prikk status [--format json]               Check repository and active WAL status",
        ],
    },
    Command {
        name: "worktree-status",
        group: Group::Record,
        summary: "Show how the files on disk differ from recorded history",
        run: crate::run_worktree_status,
        help_lines: &[
            "  prikk worktree-status [path] [--ref REF] [--format json]  Report changes against the replay baseline",
            "  note: .prikkignore at the repository root excludes matching paths from the untracked list",
            "  note: live rename declarations from `prikk mv` are reported here too, both in prose and as a `--format json` field",
        ],
    },
    Command {
        name: "commit",
        group: Group::Record,
        summary: "Record your changes as a patch in the queue",
        run: crate::run_commit,
        help_lines: &[
            "  prikk commit [--from-worktree] [--text-edits] [--ref REF] -m|--message <message>  Append worktree changes",
            "  note: .prikkignore at the repository root excludes matching worktree paths from being authored",
        ],
    },
    Command {
        name: "uncommit",
        group: Group::Record,
        summary: "Take your newest unsealed commit back out of the queue",
        run: crate::uncommit::run_uncommit,
        help_lines: &[
            "  prikk uncommit <patch-id> [--plan-only]   Take back your newest unsealed commit: nothing is wrong, your files in the worktree are not changed, and sealed history is never touched (for that, `prikk rollback-draft`)",
        ],
    },
    Command {
        name: "mv",
        group: Group::Record,
        summary: "Move or declare a rename for the next commit",
        run: crate::mv::run_mv,
        help_lines: &[
            "  prikk mv <old> <new>                      Move and declare a rename, authored into the next commit",
            "  note: if <old> is already moved on disk (a shell `mv`), this records the declaration alone, touching no bytes",
        ],
    },
    Command {
        name: "seal",
        group: Group::Record,
        summary: "Seal the queued patches into a branch",
        run: crate::run_seal,
        help_lines: &[
            "  prikk seal --allow-no-audit [--ref REF] Seal active WAL into a branch ref",
        ],
    },
    Command {
        name: "log",
        group: Group::Read,
        summary: "List the sealed blocks of a ref",
        run: crate::run_log,
        help_lines: &[
            "  prikk log [path] [--limit N] [--ref REF] [--format json]  Show sealed ref history including rollback blocks",
        ],
    },
    Command {
        name: "show",
        group: Group::Read,
        summary: "Show what one block or patch changed",
        run: crate::run_show,
        help_lines: &[
            "  prikk show <block-id|patch-id> [--format json]  Show what a block or patch changed",
        ],
    },
    Command {
        name: "diff",
        group: Group::Read,
        summary: "Compare two points, or a point and the worktree",
        run: crate::diff::run_diff,
        help_lines: &[
            "  prikk diff [path] [--from <ref|block-id>] [--to <ref|block-id>] [--path <p>]... [--format json]  Show what changed between two points, or between a point and the worktree (bare: the current branch against what `commit` would author), with unified hunks for text; binary files show ids and sizes only, and `prikk cat` reads either side; --path filters the output, and the work is whole-tree either way; writes nothing",
        ],
    },
    Command {
        name: "tree",
        group: Group::Read,
        summary: "List the files at a point",
        run: crate::tree::run_tree,
        help_lines: &[
            "  prikk tree [path] [--ref <ref|block-id>] [--prefix <p>] [--format json]  List the files at a point, with mode, size and encoding; --prefix filters by whole path components, and the replay is whole-tree either way",
        ],
    },
    Command {
        name: "cat",
        group: Group::Read,
        summary: "Print or write one file at a point",
        run: crate::cat::run_cat,
        help_lines: &[
            "  prikk cat --path <p> [--ref <ref|block-id>] [--output <file> [--force]] [--max-bytes N] [--format json]  Write one file's bytes at a point, or its metadata with --format json; --max-bytes bounds what is written, not memory; binary content refuses a terminal",
        ],
    },
    Command {
        name: "extract",
        group: Group::Read,
        summary: "Write the files at a point into a new directory",
        run: crate::run_extract,
        help_lines: &[
            "  prikk extract (--ref <ref|block-id>|--claim <claim-id>) <dir> [--allow-full-replay] [--plan-only] [--max-bytes N] [--max-files N] [--format json]  Write the files at a point into a new directory that does not exist yet; anchors only where a writer would (never cat/tree's own anchoring), refusing a point whose nearest admitted snapshot is unreachably far back unless --allow-full-replay is given; --plan-only reports the point's own size and writes nothing; --max-bytes/--max-files refuse the whole point by name before the first byte is written; --claim previews a pending RecognitionClaim instead of a ref or block id",
        ],
    },
    Command {
        name: "checkout",
        group: Group::Read,
        summary: "Plan or write the files at a point into the worktree",
        run: crate::checkout::run_checkout,
        help_lines: &[
            "  prikk checkout --plan-only [path] [--ref <ref|block-id>]  Show a safe checkout plan",
            "  prikk checkout --snapshot-plan [path] [--ref <ref|block-id>]  Validate snapshot manifest paths",
            "  prikk checkout --snapshot-materialize [path] [--ref REF]  Safely write snapshot files; the worktree is provisional until `prikk verify`, unless the block is already replay-verified",
            "  prikk checkout --patch-plan [path] [--ref <ref|block-id>]  Replay supported file-level patches",
            "  prikk checkout --patch-plan --format json --content-path <path> [--ref <ref|block-id>]  Report replayed content for the named paths (repeatable)",
            "  prikk checkout --patch-materialize [path] [--ref REF]  Safely write patch replay files",
            "  prikk checkout --patch-delete-plan [path] [--ref <ref|block-id>]  Plan explicit patch deletions",
            "  prikk checkout --patch-materialize-delete [path] [--ref REF]  Write/delete patch files",
        ],
    },
    Command {
        name: "branch",
        group: Group::Branches,
        summary: "List, create, switch and close branches",
        run: crate::run_branch,
        help_lines: &[
            "  prikk branch [list] [--all] [--format json]  List branches deterministically (name, RefState id); --all also shows closed branches, marked",
            "  prikk branch create heads/<name> [--from REF|BLOCK-ID]  Publish a branch at an existing target (a block id must be reached by a local branch or tag)",
            "  prikk branch close heads/<name>           Close a branch (not delete — pointer, history, and objects stay; reclaims nothing)",
            "  prikk branch switch heads/<name>          Switch the worktree and the current branch (refuses a missing or closed branch, unsealed work for another ref, or a dirty worktree)",
        ],
    },
    Command {
        name: "tag",
        group: Group::Branches,
        summary: "List and create tags",
        run: crate::run_tag,
        help_lines: &[
            "  prikk tag [list] [--format json]          List tags deterministically (name, target block)",
            "  prikk tag create tags/<name> --target <ref|block> [-m|--message <message>]  Publish a tag at a block",
        ],
    },
    Command {
        name: "ref",
        group: Group::Branches,
        summary: "Finish another adopted key's interrupted publication",
        run: crate::run_ref,
        help_lines: &[
            "  prikk ref complete <ref> [--plan-only]    Finish another adopted key's interrupted publication (RFC 165 R4); --plan-only prints the plan and writes nothing",
        ],
    },
    Command {
        name: "bundle",
        group: Group::Share,
        summary: "Export, import or check one ref's history as a file",
        run: crate::run_bundle,
        help_lines: &[
            "  prikk bundle export --ref REF --output <file> [--force]  Write a self-contained history bundle; refuses an existing file unless --force",
            "  prikk bundle import --input <file> [--adopt] [--max-object-bytes N] [--allow-non-fast-forward] [--format json]  Import a bundle as an untrusted received pointer; --adopt lands an absent, adopted-key-signed branch as a real local branch, published under your own maintainer key; refuses a rollback, a divergent rewrite, or an unrelated history against the pointer it would replace unless --allow-non-fast-forward is given",
            "  prikk bundle preview --input <file> [--ref REF] [--max-object-bytes N] [--format json]  Preview a bundle's impact on a local ref; writes nothing",
            "  prikk bundle verify --input <file> [--max-object-bytes N]  Check a bundle offline; writes nothing, needs no repository",
            "  note: --max-object-bytes bounds one object's encoded size, its content plus a 69-byte header -- a file of N bytes needs a bound of at least N + 69",
        ],
    },
    Command {
        name: "archive",
        group: Group::Share,
        summary: "Back up, check or import the whole repository",
        run: crate::run_archive,
        help_lines: &[
            "  prikk archive export <file> [--format json]  Write a whole-repository archive (PREPO001); refuses an existing file",
            "  prikk archive verify <file> [--format json]  Check structure, reference closure, and every signature against the archive's own carried material",
            "  prikk archive import <file> [--adopt] [--allow-non-fast-forward] [--format json]  Land every ref an archive carries; --adopt lands an absent, adopted, verified ref as a real local branch; refuses a landing that is a rollback, diverged, or unrelated history against the pointer it would replace unless --allow-non-fast-forward is given",
        ],
    },
    Command {
        name: "sync",
        group: Group::Share,
        summary: "Exchange patches with another repository, as files",
        run: crate::run_sync,
        help_lines: &[
            "  prikk sync summary --output <file>        Write this repository's PSYNCSU1 sync summary",
            "  prikk sync compare --summary <file>       Compare local refs against a remote summary",
            "  prikk sync have <ref> --output <file>     Write a PSYNCHV1 have-list for one ref",
            "  prikk sync build <ref> --have <file> --output <file> [--force]  Build a PEXCH002 artifact closing the gap",
            "  note: a built sync artifact contains repository content in the clear -- prikk does not encrypt it; move it only over a channel you trust",
            "  prikk sync accept <file> [--max-object-bytes N] [--claims-out <file>] [--force]  Accept a PEXCH002 artifact (prints claim ids; optionally writes them)",
            "  note: --max-object-bytes bounds one object's encoded size, its content plus a 69-byte header -- a file of N bytes needs a bound of at least N + 69",
            "  prikk sync pending [--format json]        List held patches named by a held recognition claim and sealed by no held Block",
            "  note: --format json is sync-pending-v1: each pending patch with the recognition claim(s) naming it and each claim's own signature outcome -- \"invalid\" is an alarm, see commands.md",
            "  prikk sync seal <ref> --claim <id>        Seal one accepted claim's patches into a block",
            "  prikk sync seal <ref> --claims <file>     Seal a batch of claims, ordered by parent block first",
            "  prikk sync tags                           List received tags and their resolution",
            "  prikk sync adopt-tag <name>               Create a local, receiver-signed tag from a received tag",
        ],
    },
    Command {
        name: "received",
        group: Group::Share,
        summary: "Remove a received ref's pointer; its objects stay",
        run: crate::run_received,
        help_lines: &[
            "  prikk received remove <remotes/…> [--plan-only]  Remove a received ref's own pointer; the objects it reached stay, reported as unreached by verify",
        ],
    },
    Command {
        name: "trust",
        group: Group::Trust,
        summary: "Choose whose maintainer keys this repository trusts",
        run: crate::run_trust,
        help_lines: &[
            "  prikk trust maintainer add --key-id ID --public-key HEX [--format json]  Trust one MAINTAINER key",
            "  prikk trust maintainer remove --key-id ID Revoke one MAINTAINER key",
            "  prikk trust maintainer list [--format json]  List every currently adopted key",
            "  prikk trust maintainer check --key-id ID [--format json]  Whether that id is adopted",
        ],
    },
    Command {
        name: "merge-plan",
        group: Group::Merge,
        summary: "Show what a merge would do",
        run: crate::run_merge_plan,
        help_lines: &[
            "  prikk merge-plan --baseline-block ID (--left-block ID|--left-ref REF) (--right-block ID|--right-ref REF) [path]  Show a read-only merge plan",
        ],
    },
    Command {
        name: "merge-evidence",
        group: Group::Merge,
        summary: "Show the evidence a merge rests on",
        run: crate::run_merge_evidence,
        help_lines: &[
            "  prikk merge-evidence --baseline-block ID (--left-block ID|--left-ref REF) (--right-block ID|--right-ref REF) [path]  Show read-only merge evidence",
        ],
    },
    Command {
        name: "merge",
        group: Group::Merge,
        summary: "Seal a merge that is proven safe",
        run: crate::run_merge,
        help_lines: &[
            "  prikk merge --allow-no-audit --baseline-block ID --into REF --from REF [path]  Seal a proven-confluent merge",
        ],
    },
    Command {
        name: "inverse-plan",
        group: Group::Undo,
        summary: "Plan a patch that undoes sealed changes",
        run: crate::run_inverse_plan,
        help_lines: &["  prikk inverse-plan [path] [--ref REF]     Plan an unsigned inverse patch"],
    },
    Command {
        name: "rollback-preview",
        group: Group::Undo,
        summary: "Preview that undo without writing",
        run: crate::run_rollback_preview,
        help_lines: &["  prikk rollback-preview [path] [--ref REF] Preview non-mutating rollback"],
    },
    Command {
        name: "rollback-draft",
        group: Group::Undo,
        summary: "Queue that undo as a patch",
        run: crate::run_rollback_draft,
        help_lines: &[
            "  prikk rollback-draft --append-inverse [path] [--ref REF] -m|--message <message>  Append inverse Patch",
        ],
    },
    Command {
        name: "rollback-draft-verify",
        group: Group::Undo,
        summary: "Check a queued undo patch",
        run: crate::run_rollback_draft_verify,
        help_lines: &[
            "  prikk rollback-draft-verify [path] [--ref REF] Verify active rollback Patch",
        ],
    },
    Command {
        name: "verify",
        group: Group::Check,
        summary: "Check the whole repository",
        run: crate::run_verify,
        help_lines: &[
            "  prikk verify [path] [--stop-on-first-error] [--format json]  Verify objects, WAL, refs, and publication trust",
        ],
    },
    Command {
        name: "doctor",
        group: Group::Check,
        summary: "Diagnose problems, and repair damage on request",
        run: crate::run_doctor,
        help_lines: &[
            "  prikk doctor [path]                       Run health diagnostics",
            "  prikk doctor [path] --repair-wal-tail     Truncate incomplete trailing WAL bytes",
            "  prikk doctor [path] --repair-index        Rebuild the object index from the containers",
            "  prikk doctor [path] --repair-pointer-index-tail  Truncate incomplete trailing pointer-index bytes",
            "  prikk doctor [path] --repair-tails [--plan-only]  Truncate every tail: the WAL, the pointer index, trust keys, trust policy, author keys, the received index, the ref log, and the three generation logs; refuses over any interior damage, same either way; --plan-only previews without writing",
            "  prikk doctor [path] --repair-main-ref     Recognized, always refused: no repair is implemented",
            "  prikk doctor [path] --rebuild-pointer-index [--plan-only]  Rebuild the ref-pointer index from the ref log (RFC 165 R5); refuses over a completable lead, a damaged ref log, or a torn tail in the ref log or the generation log; --plan-only refuses the same way and previews without writing",
            "  prikk doctor [path] --rebuild-caches [--plan-only]  Rebuild every derived cache that has a divergence check: the commit-index from the worktree (reads every file `commit` would scan once) and the lifecycle-state cache from a full replay of the current branch; under the lock `commit` takes; caches, so no recovery-log entry; --plan-only counts what it would read and writes nothing",
            "  prikk doctor [path] --discard-damaged-commits [--plan-only]  Remove an acknowledged commit the active WAL no longer holds soundly, or declare it lost (RFC 166 D5); refuses over anything else; --plan-only previews without writing",
            "  prikk doctor [path] --restore-queue-target --ref <ref> [--not-current-branch] [--plan-only]  Give a queued, un-owned commit its ref back (RFC 166 D5/§14); the ref always comes from you, never from the witness; without a witness it must be your current branch unless you pass --not-current-branch; --plan-only previews without writing",
            "  prikk doctor [path] --recovery-list       List the saved entries, grouped by the repair run that wrote them (RFC 168)",
            "  prikk doctor [path] --recovery-restore <run id> [--plan-only]  Undo one repair run, its steps in reverse order; refuses unless every file still holds what the run left; --plan-only previews without writing",
            "  prikk doctor [path] --recovery-clear [--plan-only]  Remove every saved entry, permanently; --plan-only previews without writing",
            "  prikk doctor [path] --cancel-import <id> [--plan-only]  Cancel an interrupted archive import (RFC 155 §9.5): saves the bytes an interrupted import appended to the recovery log, then cuts every container it touched back to its own starting length; refuses if no import with that id is present; --plan-only previews without writing",
        ],
    },
    Command {
        name: "unlock",
        group: Group::Check,
        summary: "List held locks, or clear a stale one",
        run: crate::run_unlock,
        help_lines: &[
            "  prikk unlock                              List every currently held lock",
            "  prikk unlock --lock <path> [--yes] [--force]  Clear one stale lock: asks to confirm unless --yes; a holder that appears",
            "                                            running is refused unless --force; the words are defined at",
            "                                            https://prikk.org/docs/reference/concurrency-locking.html#what-the-three-liveness-words-mean",
        ],
    },
    Command {
        name: "compact",
        group: Group::Check,
        summary: "Drop stale index and policy records",
        run: crate::run_compact,
        help_lines: &[
            "  prikk compact --pointer-index|--received-index|--trust-policy|--all [--plan-only]",
            "                                             Reclaim stale index/policy records (--plan-only previews only)",
            "  prikk compact --trust-policy|--received-index --keep-slot a|b [--plan-only]",
            "                                             Choose a slot to keep when the generation log is lost",
        ],
    },
    Command {
        name: "format",
        group: Group::Check,
        summary: "Upgrade the repository format in place",
        run: crate::run_format,
        help_lines: &[
            "  prikk format upgrade [path]               Verify, then move a format-6 or format-7 repository to format 8 in place (explicit; no downgrade)",
        ],
    },
];

/// Look up a command by name -- the single dispatch source `main.rs::run` calls into.
pub(crate) fn find(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|command| command.name == name)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod exit_code_tests;

#[cfg(test)]
mod help_tests;

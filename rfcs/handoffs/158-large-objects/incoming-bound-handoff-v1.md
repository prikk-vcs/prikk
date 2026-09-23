# Refuse before reading, then bound each object — RFC 158 Stage A handoff v1

**Live 2026-09-23, and it is next.** Written on the owner's instruction (*"Write the handoff."*). Design:
`rfcs/accepted/158-large-objects.md` §5 and §9 Stage A, **read its Status correction of 2026-09-23 first** — the
RFC's §1 said nothing bounds incoming objects, and that was wrong in a way that changes what this round does first.
**No format change. No schema change. New: one command (`prikk config`), one flag, one repository file.**

**Next after this, in order:** RFC 136 increment 2c, then the gate plan. Neither is live until it is next.

## Why — what is actually true today

Checked at source and on the released 0.46.0 binary by the architect:

- **Every incoming artifact already has a total-size bound** (DC-86): bundle 256 MiB (`PRIKK_BUNDLE_MAX_BYTES`),
  sync exchange artifact 256 MiB (`PRIKK_EXCHANGE_MAX_BYTES`), sync summary 16 MiB (`PRIKK_SYNC_SUMMARY_MAX_BYTES`),
  have-list 64 MiB (a constant, no variable).
- **Each is checked after the whole file is already in memory.** Six entry points call `std::fs::read` and hand
  the bytes to a store function that then compares `bytes.len()`:

  | command | read at |
  |---|---|
  | `bundle import` | `crates/prikk-cli/src/bundle.rs:98` |
  | `bundle preview` | `crates/prikk-cli/src/bundle.rs:144` |
  | `bundle verify` | `crates/prikk-cli/src/bundle.rs:215` |
  | `sync compare` (`--summary`) | `crates/prikk-cli/src/sync.rs:108` |
  | `sync build` (`--have`) | `crates/prikk-cli/src/sync.rs:138` |
  | `sync accept` | `crates/prikk-cli/src/sync.rs:203` |

- **Measured:** a sparse 1 GiB file given to `prikk bundle verify` (0.46.0) peaks at **1,051,060 KB** resident,
  then refuses; a sparse 300 MiB file peaks at 310,492 KB. Peak equals file size. A file larger than memory is an
  out-of-memory kill, not a refusal.
- **The refusal** reads `malformed persisted data: bundle is N bytes, over the configured limit of M bytes`. The
  input is neither malformed nor persisted, and nothing says where M comes from or how to change it.
- **None of the six DC-86 variables is in the user docs.**

So the order of work is: **(1)** refuse before reading, on all six; **(2)** make those refusals say what they
are; **(3)** add the per-object bound and `prikk config`, which is where the owner ruled it lives.

## 1. Refuse before reading — one reader, six callers

One shared function in `prikk-cli` reads an outside artifact under a bound. All six entry points use it, each
passing its own bound. Its contract:

1. **Open the file, then check its size from the open handle's metadata** (not from the path, so the file checked
   is the file read). A regular file over the bound refuses **without reading a byte**.
2. **Then read through `Read::take(bound + 1)`**, and refuse if more than `bound` bytes arrive. Metadata can lie —
   a FIFO reports 0, a file can grow between the check and the read — and this is what catches it. This is RFC 158
   §5.1's *"enforced again while streaming, so a lying declaration fails"*.
3. **Initial capacity is `min(metadata length, bound)`**, never the metadata length alone.
4. A non-regular file is not refused for being non-regular — reading a FIFO is a legitimate way to feed a bundle —
   but it gets no fast path; only step 2 bounds it.

**The store's own `bytes.len()` checks stay** as they are: they are what bounds a library caller, and removing
them is not in scope.

## 2. The refusals say what they are

Every size refusal on these paths — the total bound at each of the six entry points, and the new per-object
bound — names:

- **what was too large** (the artifact, or object *n* in it) and **its size**;
- **the bound applied**, in bytes and in the nearest binary unit (`268435456 bytes (256 MiB)`);
- **where that bound came from** — the default, the named environment variable, the config key, or the flag;
- **how to change it**, as a command or variable the reader can paste.

It does **not** say "malformed" or "persisted". **The exit code does not change** — 0.46.0 exits 1 on the 1 GiB
probe above; keep it, since a consumer's script may test it. Whether that means a new error variant or a CLI-side
refusal is yours to choose — say which, and why.

## 3. The per-object bound

- **Applies to** `bundle import`, `bundle preview`, `bundle verify` and `sync accept` — everything that decodes
  objects from outside. **Not** to a repository's own `commit` (RFC 158 §5.2).
- **Measures an object's encoded size as it travels** — the length prefix of its frame in the artifact. For a
  file that is its content plus a small header; the docs say so in one sentence, so a user who sets 100 MiB is
  not surprised when a file of exactly 100 MiB refuses.
- **Checked on the length prefix, before the frame is copied or decoded** — in `decode_bundle`
  (`crates/prikk-store/src/bundle.rs`, the `read_bytes_u64` in the object loop) and in `decode_envelope_section`
  (`crates/prikk-store/src/patch_exchange/artifact.rs`). Today both copy the frame with `.to_vec()` and then decode
  it.
- **Default: 256 MiB**, equal to the total default, so **nothing that imports on 0.46.0 starts refusing.** At the
  defaults the per-object bound is therefore inert; it acts when an operator lowers it, and when Stage B lets an
  artifact legitimately exceed 256 MiB. Do not change any default in this round.
- **Where it comes from, highest first:**
  1. `--max-object-bytes N` on the command — one invocation;
  2. the repository's `incoming.max-object-bytes` in `prikk config`;
  3. the default.

  `bundle verify` has no repository, so for it only (1) and (3) exist — and its refusal must not suggest
  `prikk config`.
- **The bound is never taken from the input it bounds.** Nothing in a bundle or an exchange artifact can raise
  it, and nothing an import writes can reach the config file. State in the report how you checked the second half.

## 4. `prikk config` — the smallest version that is a real one

RFC 135 §9.1a deferred `prikk config` until *"a first real adopter"*; this is it. Build only what this key needs,
in a shape that a second key does not have to undo.

- **File:** `.prikk/config`, in the repository's own directory, **never in the worktree** — a checked-out or
  imported file must not be able to set a bound. Absent means every key takes its default.
- **Format: hand-built, no new dependency.** One `key = value` per line; blank lines and `#` comments allowed.
  **Ruled by the architect:** RFC 135 §4 named `app-json-settings`/serde as a candidate, but a reader that every
  import consults is attack surface, and one integer key does not justify a dependency. A later key that needs
  structure reopens the question as an `ALLOWED_THIRD_PARTY` decision, as RFC 135 §4 says.
- **Strict, and fails closed:** an unknown key, a duplicate key, a zero, a negative or a non-integer value each
  refuse, naming the line. Never a silent fall back to the default — a typo in a bound must not quietly loosen it.
  The file is itself read through §1's bounded reader (64 KiB is ample).
- **Commands:**
  - `prikk config get <key>` — the effective value and its source: `incoming.max-object-bytes = 268435456 (default)`;
  - `prikk config set <key> <value>` — validates, then writes the whole file atomically (the existing
    `durable_output` writer);
  - `prikk config unset <key>`;
  - `prikk config list` — every known key, effective value, source.

  One key means last-writer-wins on two concurrent `set`s is exact. Say so in the code; a second key makes
  `set` a read-modify-write that needs a lock and a race test, and that is that round's work, not this one's.
- **An older prikk must still open a repository that has this file.** Check it by hand against the 0.46.0 binary
  (`verify`, `worktree-status`, `bundle import`) and report what you ran.
- **The six DC-86 variables stay exactly as they are.** Whether they move into `prikk config` later is recorded in
  RFC 158 as not decided.

## 5. Docs

- `docs/src/reference/commands.md`: `prikk config`, and `--max-object-bytes` on the four commands.
- **Document the six DC-86 variables for the first time**, beside the per-object bound, in one place: what each
  bounds, its default, and that it refuses before reading. Link it from the backup/restore and sync guides where a
  user would meet the refusal.
- One sentence on what is still true until Stage B: an artifact under its bound is still read whole into memory,
  and decoding costs a multiple of that (§8's measurement gives the figure).

## 6. Controls — every one shown failing

Linux-gated where they measure memory; the bundle tests already are. **Measure a child's peak exactly, with
`tests/support/rusage_child.py`'s `getrusage(RUSAGE_CHILDREN)` method, never by sampling `VmHWM`** — sampling can
miss a short peak, and then the perturbation below cannot go red.

**Refuse before reading**
1. **Each of the six entry points**, given a sparse 1 GiB file (`set_len`, no disk cost), refuses with its size
   refusal and a child peak under 64 MiB. Six controls, one per entry point.
2. **The reader itself**, unit-tested over an in-memory `Read` that counts bytes: a declared size over the bound
   reads **zero** bytes; a source that yields more than its declared size is refused at `bound + 1`.
3. **A lying size, end to end:** a FIFO fed `bound + 1` bytes by a writer thread (bound lowered through its
   environment variable) is refused as too large; fed exactly `bound` bytes it passes the reader and fails, if at
   all, for some other reason.
4. **Exactly at the bound:** a real bundle with `PRIKK_BUNDLE_MAX_BYTES` set to its own length imports; set to one
   less, it refuses before reading.

**The per-object bound**

5. With `--max-object-bytes` equal to the bundle's largest frame, `bundle import` succeeds; one less refuses,
   naming the object's size, the bound and `--max-object-bytes` as the source. The same through `sync accept`,
   `bundle preview`, and `bundle verify` **with no repository**.
6. **Source and precedence:** the same refusal with the bound set by `prikk config` names the config key; with
   both set, the flag wins; with neither, it names the default. `bundle verify`'s refusal never mentions
   `prikk config`.
7. **Checked before decoding:** a frame over the bound whose bytes are not a valid envelope refuses with the size
   refusal, not a decode error.
8. **A refused import writes nothing**, in the 0.44.0 sense — objects and containers byte-identical before and
   after (`bundle_import_refuses_before_writing.rs` has the helper). The same for `sync accept`.
9. **A local `commit` is not bounded:** with `incoming.max-object-bytes` set below a file's size, committing that
   file succeeds.

**`prikk config`**

10. `set`/`get`/`unset`/`list` round-trip; the file lands in `.prikk/`, not the worktree.
11. Unknown key, duplicate key, zero, non-integer, and an oversized file each refuse, naming the line; none falls
    back to the default.

**The refusals**

12. Each total-bound refusal names its variable (or the default) and how to change it, contains neither
    "malformed" nor "persisted", and exits with 0.46.0's code.

## 7. Perturbations — revert each by copy or Edit, never `git checkout -- <path>`

1. **Restore `std::fs::read` at one entry point at a time** → that entry point's control 1 goes red, and only it.
   Six perturbations. *This is "the bound checked after reading" from RFC 158 §9, and it is the code as it stands.*
2. **Trust the metadata** — after the size check, read with `read_to_end` instead of `take(bound + 1)` → controls
   2 and 3 go red.
3. **Check the per-object bound after `.to_vec()` and decode** → control 7 goes red (a decode error arrives first).
4. **Swap the precedence** (config over flag) → control 6 goes red.
5. **Let the config reader skip a line it does not recognise** → control 11 goes red.
6. **Remove the store's own `bytes.len()` check** → an existing DC-86 store test goes red. If none does, that is
   a finding: say so and add one.

## 8. Measure, don't gate — one figure for Stage B

On the release build, with the machine as quiet as you can make it (**the architect runs nothing while you
measure**): peak resident of `bundle verify` and `bundle import` for a bundle carrying **one blob of about
255 MiB**, at the defaults. Three samples each, the method of §6. Report the figure and its ratio to the bundle's
size; it goes into §5's docs sentence and into Stage B's starting line. **It gates nothing in this round**, and no
default changes because of it.

## 9. What the report states

- the gates, all fourteen, on the exact final commit;
- each control and the perturbation that turned it red, by name;
- how you checked that nothing an import writes reaches `.prikk/config`;
- what you ran against the 0.46.0 binary for §4's compatibility check;
- §8's figures, samples and load, and anything running you could not stop;
- anything in this handoff that turned out not to be true at source — as RFC 158 §1 did. Say it; do not work
  around it.

## Addendum 1 — 2026-09-24: accepted, three small fixes owed

**The round is ACCEPTED** (`82359ff4`, `033c6fcc`, `312cb031`; review `rfc158-incoming-bound-review-v1`; 14/14 gates
re-run by the architect on `312cb031`). **This addendum is live, and it is next.** No design and no new surface:
one round, with gates on its final commit.

1. **`bound + 1` overflows.** In `read_bounded` (`crates/prikk-cli/src/bounded_read.rs:158`), a bound of
   `u64::MAX` (for example `PRIKK_BUNDLE_MAX_BYTES=18446744073709551615`, which parses) panics in a debug build. In
   a release build it wraps to `take(0)`, so every bundle is refused as `invalid bundle magic`. On 0.46.0 that
   value meant "no practical limit". Use `saturating_add(1)`.
   **Control:** with the variable at `usize::MAX`, a real bundle verifies. **Perturb** back to `+ 1`; the control
   must go red.
2. **Pin the per-object boundary.** Changing `declared_bytes > bound_bytes` to `>=` in `read_bounded_object_frame`
   leaves every test green (the architect ran it: `prikk-store --lib` 1225/1225, `rfc158_incoming_bound` 22/22).
   Control 5 uses bounds of 25,000 and 50, not "the largest frame and one less" as its own doc comment says.
   - Add a store unit test on `read_bounded_object_frame`: a frame of exactly `bound` bytes reads, and one of
     `bound + 1` refuses with `ObjectOverBound`.
   - Either make control 5 do what its comment says, or correct the comment.
   - **Perturb** `>` to `>=`; the new test must go red.
3. **Docs.**
   - `commands.md:199-201` must say plainly that **a file of exactly the bound refuses**, because of the header,
     with the measured overhead *k*. Add a control pinning *k*: a blob of N bytes refuses under a bound of N and
     imports under N + k.
   - `commands.md:183`: four size bounds (one fixed) and three count bounds, six variables — not "six
     total-artifact bounds".
   - `backup-restore.md:317` and `sync.md:127`: the per-object bound is **equal** to the total by default, not
     "smaller".
   - One sentence in the `prikk config` paragraph: `set` rewrites the whole file and `unset` removes it, comments
     included.

**Next after this, in order:** RFC 136 increment 2c, then the gate plan.

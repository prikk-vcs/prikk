# RFC 172 — Who may author, who may seal, and how a repository's trust changes

**Status.** **PROPOSED 2026-11-27 by the architect,** from the 0.59.0 H design round (H1 to H3a2, `rfcs/handoffs/171-who-may-author-who-may-seal/`).
- **Nothing here is built.** Per the owner's ruling of 2026-11-18, H's builds are **0.60.0 at the earliest, after the owner reads the
  round.**
- **Two consumers asked to read the design before anything is built:** planeter (2026-11-21) and inklkunn (2026-11-25). The letter that
  sends it to them is drafted in H3b and held for the owner.
- **Already in 0.59.0, because each is a released defect or describes shipped behaviour** (not part of this proposal):
  - `setup --*-seed-out` and `key generate --out` refuse a missing or unwritable seed directory before anything is written (H2c, H2d,
    H2e);
  - the lock judge reads a holder reaped mid-look as gone (H2d);
  - the page "an author proposes, the receiving maintainer seals", written against today's binary (H3b).

**Two fixed rules hold against every part:**
- **no state that no command clears:** prikk never freezes a repository whose every seed is lost;
- **no claim wider than the mechanism:** every check says what it is checked against.

## 1. What is true today (measured on the 0.59.0 candidate: H1, H2, H3a)

- **Key custody:**
  - `commit` needs the author seed. `seal`, `merge`, `branch create`/`close`, `tag create`, `sync build`, `sync seal`, `bundle import
    --adopt` and `ref complete` (even `--plan-only`) need the maintainer seed. Everything else needs neither.
  - **An author-only machine can commit and queue, and nothing else.**
  - **A same-account process can read any seed of that account:** the separation of author and maintainer is an OS boundary (another
    account, a container), never a prikk check.
- **Delivery:**
  - **Route 1:** the author repository seals under its own maintainer key, which the receiver has not adopted. The author runs `sync
    build`; the receiver runs `sync accept` (no seed), reviews `sync pending`, and runs `sync seal --claims`, which re-seals under the
    receiver's key. **Claims read `Unverifiable`, which is the expected answer.** The receiver's seed-holding steps read only the
    receiver's own repository.
  - **An unsealed queue has no delivery path.**
  - **"Route 3"** is the maintainer sealing a handed-over `.prikk` in place. **It lends the maintainer's key to whatever the author's
    repository holds:** with a block in the copy sealed under the author's own adopted key, the maintainer's `sync build` signed claims for
    it, and the receiver read them as `Sound` under its own key. **Route 3 is refused as a delivery path.**
- **Trust:**
  - **`trust maintainer add` and `remove` are unsigned and need no key.** The policy container holds snapshots of the adopted set, not
    events.
  - A copy cannot tell who adopted a key.
  - **Removing a key fails `verify` for every object it signed** (per sealed generation: a block, a ref-state and a ref-update; plus
    tags and their records). Re-adding the same key restores it.
  - With every seed lost, an unsigned `add` reopens the repository.
  - `verify` fails on received history (`remotes/`) signed by keys the receiver never adopted, while `received refs: N, 0 failed`.

## 2. Authoring and delivery (ruled in H2 and H2b)

1. **Route 1 is the proposal path.**
   - The author repository is made with `setup --author-seed-out <a> --maintainer-seed-out <m>`. That adopts only its own new maintainer
     key and leaves the receiver's key directory untouched.
   - It is used with `PRIKK_AUTHOR_SEED_FILE` and `PRIKK_MAINTAINER_SEED_FILE`.
   - **`key status <repo> --role maintainer` is the witness:** `binding: matches` with the exports, `not-adopted` without them.
   - **The page's rules:**
     - never run a command that reads your maintainer seed in a repository someone else wrote or handed you;
     - never run `setup` for an author repository in the receiving maintainer's key directory without both seed-out flags;
     - the bundle route is not for proposals.
2. **`setup` reusing a key says so:** `reusing the maintainer key <id> from <dir>`. It does not refuse.
3. **The refusals on a machine holding an author key and no maintainer key** gain a clause naming the first step: make a maintainer key
   for this repository with `key generate --role maintainer --out <path>`, then the trust line it prints, then
   `PRIKK_MAINTAINER_SEED_FILE`.
   - **Each sentence is true in every state it is shown in.** An author-only machine and a maintainer who deleted both files read the
     same, so no sentence names "the receiving maintainer".
   - **The not-trusted refusal lists the adopted ids.** `key status` gains one text line in the same state.
4. **`key generate --role author|maintainer`:** next steps per role. With no `--role`, the output is byte-identical to today's.
5. **The author key per patch** is printed by `show`, by `status --format json` and by **`sync pending`** (route 1's review surface), in
   text and JSON (additive).
6. **`ref complete --plan-only` needs no seed.**
   - It prints the configured key id, from its `.key-id` file, and what the real run will need.
   - **It exits 0 where it refused (exit 1) for a missing seed.**
   - The real run still proves the seed.
   - The test is portable: a seed path that is a directory.

## 3. How a repository's trust changes

### A. The chain: every transition recorded, none of them refused

- **An append-only `trust/transitions.container`.** Each record is `{ seq, previous_record_hash, action, signed_by, signature }`; the
  signature, when present, covers the canonical bytes including `previous_record_hash`.
- **Actions:** `Add { key_id, public_key }`, `Remove { key_id }`, `Retire { key_id, point }`, `Rotate { from, to, public_key, point }`.
- **The one-step verbs never sign** (`trust maintainer add`, `remove`, `retire`, `rotate`). They write an **unsigned** record with
  today's arguments, exit codes and output, and they **never read a seed.**
  - *Architect's ruling over H3a2's "sign when an adopted seed is present".* A signed record is a portable authorization (B2). A one-step
    command that signs whenever a seed happens to be present would mint one in any copy the maintainer is handed.
  - **That is route 3 again, and worse:** handed a copy of a forge's `.prikk` and asked to "add my key", the maintainer's signed record
    applies to the forge itself, because the two share a trust tip (B2).
  - **So signatures come only from the explicit command for them, `trust statement` (B2).**
- **The current set** = the root epoch (today's policy, read as written, unsigned) + the chain. **Once a chain exists, the policy
  snapshot is not rewritten.**
  - Every reader goes through one function that derives the set, so a transition is **one durable append**, and the snapshot is no second
    state to fall behind (§4).
  - *Ruling over H3a2's snapshot-as-cache.*
- **`trust maintainer list`** shows the chain, each record marked `unsigned` or `signed by <id>`, and ends with **`trust tip: <hash>`**,
  the hash of the chain's last record.
  - The trust tip of a repository with no chain is the hash of its root epoch, so every repository has one.
  - **"Trust tip" is a deliberate name:** a ref's tip block (what `retire` snapshots, C) is a different thing, and one word for both
    would confuse.
- **`verify`:**
  - **prints nothing about the chain while it is whole.** A line that decides nothing is noise, and consumers parse `verify`'s text.
    `trust maintainer list` shows the chain (architect's verification against the owner's philosophy, 2026-11-28);
  - **two blocking findings:** `PRIKK-TRUST-CHAIN-BROKEN` (a record that does not link) and `PRIKK-TRUST-TRANSITION-INVALID` (a claimed
    signature that does not verify against the key adopted at that point);
  - **an unsigned transition is never a warning:** nothing clears a past transition, so a warning for it would be unactionable.
- **What the chain closes: nothing about route 3, under any option.** The author can adopt the receiver's public key in the author's own
  repository, unsigned, or signed by the author's own key. **A chain checked only from its own root is self-attesting:** whoever writes
  `.prikk` writes the root (external ack 043). **It gives evidence and the two relative checks below, not a lock.**

### B. The only checks that mean something are relative to what the checker already holds

- **B1, a prefix: "this copy's chain extends one I hold."**
  - `archive verify <new> --extends <old.archive>`, or `--extends-trust-tip <hash>` (the trust tip a reader kept from `trust maintainer list`), is
    read-only. It answers `extends yours by N transitions` or `diverges from yours at transition S` (exit 1).
  - **`archive import` runs it against the importing repository's own chain** and prints the result. With no chain of its own it says
    *"nothing to compare against; the carried chain is checked only against itself"*.
- **B2, a key I already trust: a statement.**
  - **`trust statement add|remove|retire|rotate … --trust-tip <hash> --out <file>`** signs with the local maintainer seed. **It opens no
    repository:** the trust tip is given explicitly, read beforehand by `trust maintainer list` (which reads no seed). The seed-holding process
    reads no repository at all.
  - **`trust apply <file>` needs no seed.** It checks:
    - that the signer is adopted at this repository's current trust tip (a retired or removed key cannot sign);
    - that the statement's trust tip **equals** this repository's (no replay into a later state, no reordering).
    It then appends the record, signed.
  - **What a statement means, stated plainly:** with no repository identity (settled), **a statement is valid in every repository whose
    trust tip is `--trust-tip`.** A copy shares its original's trust tip until either moves, so **a statement signed "for a copy" is also a statement
    for the original.** That is why it is made only by this deliberate command, with the trust tip in the maintainer's own hands.
  - **And unrelated repositories can share a trust tip.** Two repositories whose trust records are the same have the same tip, for
    example two set up with the same maintainer key (the documented second-project flow) and no transition since. **A statement made for
    one then applies to the other.** Repositories have no identity (settled), so nothing can tell them apart.
    - **So `trust statement` prints what it covers:** the trust tip, the number of transitions under it, and the adopted keys at it,
      with the words *"applies to every repository whose trust record is this one"*.
    - **`trust apply` prints what it adds** before it appends.
    - The test: two repositories set up from one key directory, one statement, applied to both (architect's verification, 2026-11-28).
  - **For planeter's forge:** apply only statements signed by a key the forge's repository already adopts. That stops a stolen web token
    adding a key. **It does not stop whoever can write the forge's `.prikk`.**
- **What each copy can check:**

  | copy | what it can check |
  |---|---|
  | `archive export` → `import` | carries the chain; **B1 against the importer's own chain** |
  | a `.prikk` copy | is the repository |
  | `bundle` and `sync` artifacts | **carry no chain.** The receiver judges signatures by its own policy, as today |

### C. Retire and rotate from a recorded point; `remove` unchanged

- **The point is a snapshot of ref tips:** per ref, the tip block and tip ref-state, or `--at <ref>=<block>`. **An object signed by the
  retired key counts if and only if it is reachable from the snapshot:**
  - a block that is an ancestor-or-equal of a recorded tip;
  - a ref-state or ref-update in that ref's log up to the recorded ref-state;
  - a tag the snapshot lists.
- **Anything the key signs afterwards does not count:** a new block, a fork, a new tag. **That includes a rollback record:** a ref-state
  after the point that moves a ref back to an old block is not counted, though the block it names is.
- **`retire --key-id K [--at …] [--reason …]`:** an earlier point says that what K signed after it, legitimate work included, stops
  verifying.
  - **The way out for such work, named before anything is written** (architect's verification, 2026-11-28: no state that no command
    clears):
    - after an earlier point, a ref whose history passes through an object K signed after it fails `verify` for good: the object stays in
      that ref's history, and nothing re-signs history;
    - **`retire --plan-only` names every such ref and object** first;
    - **the refusal and `verify`'s way out name the sound-copy sequence from the point:** a new repository made from the history up to
      the point, and the later work re-authored under a trusted key;
    - **the build tests that sequence end to end,** to `verify` 0.
  - **At the default point (the current tips) nothing fails,** and that is the form the help leads with.
- **`rotate --from K1 --to K2 --public-key …`:** one record. A hand-over signed by K1 is `trust statement rotate` plus `trust apply`.
- **`remove` keeps its meaning** ("this key's signatures never count"). It gains `--plan-only`, as does `retire`, printing by object kind
  what would stop verifying (stikk 023).
- **Without the chain** (bundle, sync), a retirement is local to the repository that recorded it.

### D. Received history

- **An object reachable only from `remotes/` and signed by an unadopted key is `PRIKK-TRUST-RECEIVED-UNADOPTED`, info,** with the key id and
  the line that adopts it.
  - **Its sentence says plainly that the history is not verified:** *"received history signed by <key>, which this repository has not
    adopted, is not verified"*. So an exit 0 cannot read as "everything verified".
  - **In `--format json` it has its own array** (`received_unadopted[]`), **never an item of `publication_trust_issues[]`.** That array
    keeps its meaning, "every item blocks" (RFC 174 §4.1, no severity in `verify`), which planeter's delivery rule reads. (Architect's
    verification, 2026-11-28.)
- Anything reachable from a local ref stays blocking.
- **This changes an exit code planeter reads:** exit 1 becomes exit 0. **It is a question to them, not a decision.**

### E. Who first sealed a block

`show <block>` and `log` (text and JSON) print `recognized by <key id> (<outcome>)`, from the claim container that already holds it. **No
claim-format change.** In route 1 the claim signer is the author repository's key.

## 4. One transition's crash cases

**A transition is one append** (the snapshot is not rewritten once a chain exists).

| killed | state | way out |
|---|---|---|
| before the append | unchanged | repeat the command |
| inside the append | a torn tail | **writers refuse and name `doctor --repair-tails`, which truncates it.** Nothing was applied |
| after the append | done | a repeat finds the identical record at the trust tip and writes nothing (idempotent) |
| a damaged record inside the chain | `PRIKK-TRUST-CHAIN-BROKEN` | **writers fail closed;** `doctor` names it. No automatic repair: it would change what is trusted |

**The build's test is the real-kill sweep over the append,** as for every other writer.

## 5. Every consumer case

| case | served by | what stays impossible |
|---|---|---|
| inklkunn 1: rotation, old key not kept adopted | `rotate`; a K1-signed hand-over by statement | **without the chain, the receiver must still adopt K1 to verify old blocks** |
| inklkunn 2: a lost key as a recorded state | `retire --reason lost` (unsigned, or a statement by another adopted key) | **a lost key cannot sign its own retirement** |
| inklkunn 3: a stolen key, from a point on | `retire --at` at the last good block per ref (the rollback rule holds) | **prikk cannot know when the theft was;** legitimate work after the point stops verifying |
| inklkunn 4: permanent removal, re-created and re-signed | the new repository is a new root | **linking old to new: repositories have no identity** |
| planeter 1: `remove` fails `verify` | `retire`; `remove --plan-only`; the wording (0.59.0 step 0 G) | |
| planeter 2: every key lost, frozen? | **never frozen:** an unsigned record is always possible | |
| planeter 3: `verify` on received history | D | the exit code is theirs to accept |
| planeter's forge: keys added only on a signed statement | B2 | **whoever writes the forge's `.prikk`** |

## 6. Format steps, price, output changes

- **Format steps:**
  - **the transitions container,** a repository-format step: an older prikk refuses by the format marker, and `format upgrade` writes
    nothing;
  - **the archive's chain section, a 14th section kind, written only when a chain exists.** An archive of a repository with no chain
    stays readable by older readers.
  - **No claim-format step.**
- **Two stages, so time is spent only where a consumer needs it:**
  - **Stage 1** is §2, C, D and E. It serves every case in §5 that lives inside one repository: planeter 1, 2, 3, Q2 and Q3, and
    inklkunn 1 to 3 for a repository that keeps its own records. **C does not need A:** a retirement is one more kind of record in the
    trust policy container. **That is Stage 1's one format step,** a policy-record kind an older prikk refuses by the format marker.
  - **Stage 2** is A and B: the chain, statements and prefix checks. It serves what crosses repositories: planeter's forge accepting only
    signed statements, and inklkunn's readers checking that a later archive continues an earlier one.
  - **Stage 2 is built only if planeter or inklkunn, having read this, say they will use it.** The letter asks them.
- **The price:** about 1,500 lines across `prikk-store`, `prikk-cli` and `verify` (H3a2 §7, less the snapshot rewrite and the one-step
  signing).
- **Tests:**
  - every action;
  - every tampering;
  - the legacy read (unchanged `verify` and `list`);
  - **the real-kill sweep;**
  - **the rollback case;**
  - a statement for another trust tip, by an unadopted key, and replayed;
  - **`trust statement` opening no repository;**
  - **the one-step verbs opening no seed.**
- **`### Output changes`:**
  - new verbs and flags;
  - `trust maintainer list`'s chain and trust tip;
  - `verify`'s three finding ids, and `received_unadopted[]` in its JSON;
  - **D's exit code;**
  - `show`/`log`'s `recognized by`;
  - the §2 sentences and fields;
  - **`ref complete --plan-only`'s exit code.**

## 7. For the owner: three decisions, each with the architect's recommendation

**The owner, 2026-11-28:** *"Your recommendations are almost approved. However, verify carefully they match my fundamental philosophy."*
**The architect's reading:**
- the three recommendations below are approved, subject to that verification;
- **the verification found four places in this RFC that did not hold, now corrected:**
  - the `verify` chain line (§3A);
  - unrelated repositories sharing a trust tip (§3B);
  - the way out after `retire --at` (§3C);
  - D's place in JSON and its sentence (§3D);
- it also found one dependency in item 2 (below), now stated.
- **The RFC as a whole stays proposed** until the owner accepts it.

Each was reviewed against the owner's philosophy: clean, safe and secure, robust; nothing that confuses; less is more; time; no
unactionable warning; no state that no command clears; no claim wider than the mechanism.

1. **Is a per-task author key ever bound to a person? Recommended: no.** A key stays an artifact, as today: "continuity only, not a trust
   decision".
   - **Security, and the claim it would make:** prikk cannot establish who a person is. A binding would be a label typed by whoever writes
     `.prikk`, and a receiver would read it as proof of authorship: a claim wider than the mechanism.
   - **The settled rule:** repositories and keys carry no identity (*repositories are anonymous*). A person field would reopen that.
   - **Less is more:** what a reviewer needs is the key id per patch (§2.5) and the message. Who stands behind a key is the consumer's own
     record, outside prikk.
   - **What the user sees:** nothing new; the key id in `show`, `status` and `sync pending`.
2. **May an operator freeze a repository's trust deliberately (`trust lock`, after which unsigned records are refused)? Recommended: no.**
   - **A state that no command clears:** a locked repository whose every seed is later lost can never adopt a key again, and nothing
     prikk ships could reopen it. That is the one outcome the first fixed rule forbids.
   - **It protects less than it appears to:** a process that can write `.prikk` can truncate the chain to before the lock, and the result
     is consistent with itself (ack 043). The lock would stop only honest use of prikk's own unsigned verbs, which is the consumer's policy
     to decide, not a repository state.
   - **What serves the need instead:** a forge that wants "signed transitions only" applies only statements (B2), and checks with B1 that
     what it holds extends what it held. B1 reports `extends yours by N transitions (S signed, U unsigned)`, so a consumer can refuse
     unsigned ones by its own rule, with no state in the repository.
   - **This rests on Stage 2** (statements and prefix checks), which is built only on a consumer's word (§6). **If it is not built, nothing
     in prikk serves "signed transitions only",** and the recommendation is still no lock: a forge then keeps that rule by not running the
     unsigned verbs, outside prikk.
   - **What the user sees:** no `trust lock` command, and no repository that can be stuck.
3. **D: `verify` reports received-only history under an unadopted key as info (exit 0 where it exits 1 today). Recommended: yes, and put
   to planeter before the build.**
   - **Clearer:**
     - the import already says the received pointer is not trusted;
     - `received refs: N, 0 failed` already agrees;
     - today's exit 1 tells a user their own repository failed verification over history they never adopted.
   - **Actionable and clearable:** the finding names the key and the adopt line, and goes away on adoption or `received remove`.
   - **Safe:** anything reachable from a local ref stays blocking. A consumer that must gate on "nothing unadopted is present" gates on
     `received_unadopted[]` being empty in `--format json` (§3D).
   - **Not misleading:** its sentence says the received history is not verified, so exit 0 does not read as "everything verified".
   - **The cost:** an exit code changes, and planeter's delivery rule reads it. The letter asks them first; they may keep a stricter gate
     on the id.

**And the staging (§6): Stage 1 in 0.60.0, if the owner accepts this RFC; Stage 2 only on a consumer's word.** Release scheduling is the
owner's.

# Sync

RFC 116 adds `prikk sync`: negotiation-as-artifacts between two repositories, with no network code
in prikk itself. RFC 117 stage 3 extends the same artifact to carry tags, adopted separately from
sealing. See [Security and Signing Setup](security-setup.md) for the maintainer key both sides need
before sealing or adopting anything sync brings in.

```sh
prikk sync summary   --output <file>
prikk sync compare   --summary <file>
prikk sync have      <ref> --output <file>
prikk sync build     <ref> --have <file> --output <file> [--force]
prikk sync accept    <file> [--claims-out <file>] [--force]
prikk sync pending
prikk sync seal      <ref> --claim <id>
prikk sync seal      <ref> --claims <file>
prikk sync tags
prikk sync adopt-tag <name>
```

## The loop, as a person actually runs it

Two repositories, A (has content B wants) and B (wants it), moving files by whatever means the two
operators already have — email, a shared drive, a USB stick. Every step below names the file the
previous step produced.

```
A: prikk sync summary --output summary.bin
B: prikk sync compare --summary summary.bin
B: prikk sync have <ref> --output have.bin
A: prikk sync build <ref> --have have.bin --output artifact.bin
B: prikk sync accept artifact.bin --claims-out claims.txt
B: prikk sync pending                              # optional, observational
B: prikk sync seal <ref> --claims claims.txt
B: prikk sync tags                                 # optional, observational
B: prikk sync adopt-tag <name>                      # per tag, if any arrived
```

**`summary`** publishes every `heads/*` ref A holds, each with its own patch-set digest and count —
a few hundred bytes regardless of history size. **`compare`** reads a summary against B's own refs
and reports each as `in-sync`, `differs`, `remote-only` (B lacks it), or `local-only` (A lacks it).
**`have`** is B's own reachable patch-id list for one ref B wants — the input `build` needs to
compute exactly what B is missing. **`build`** is A's side of that computation: it writes an
artifact carrying the delta, one recognition claim per block the delta touches, and every local tag
whose target lies within the ref's ancestry. **`accept`** verifies and stores everything the
artifact carries — patches, blobs, claims, tags — and reports what it found; it does not seal or
adopt anything by itself. **`pending`** lists patches accept has stored that are not yet reachable
from any of B's own blocks. **`seal`** takes the claim ids `accept` wrote out and turns the accepted
patches into B's own sealed blocks, under B's own maintainer key. **`tags`** lists every tag B has
received but not adopted, with its current signature outcome and whether B's own history can resolve
it yet. **`adopt-tag`** creates B's own local tag for one received tag, once B holds the same patch
set locally.

## What it does — and does not do

**prikk does not move the bytes, and does not encrypt them.** A file `sync build` writes contains
repository content **in the clear**. prikk guarantees integrity and authenticity — every object is
content-addressed and every claim, tag, and sealed publication is signed — never secrecy. The channel
that moves the file is the operator's choice and the operator's responsibility. What travels is
narrower than "everything," though: the artifact carries only the objects the delta and the ancestry
walk actually name, plus **public** key material for the patches' own authors — never a secret key or
seed, which live only in each operator's own environment variables and never appear in any artifact.

**A deletion's own content travels too.** An artifact carries the content each deleted node held, so the
receiver can replay and roll back that deletion. When nothing stored it — a text file whose edits only
ever travelled as spans — `sync build` derives it by replaying the history and carries the result,
checked against the id the deletion names. Before 0.43.0, building an artifact for such a history
refused.

**prikk stays off the network by design, not by omission.** Every check in the accept path already
treats the artifact as untrusted input from an unknown origin; adding a transport would add attack
surface without adding verification strength, so RFC 116 ruled negotiation-as-artifacts first and
network code out of scope for now.

**The receiver seals and adopts under its own key, always.** Accepting an artifact never adopts a
maintainer key, never advances a ref, and never creates a local tag by itself — those are separate,
explicit acts (`seal`, `adopt-tag`), each signed locally.

**An object's id is the hash of its payload, and signatures are not part of the payload.** So a receiver
that seals the same patches, in the same grouping, onto the same parent produces the sender's block id
and the sender's `RefState` id exactly; only the signatures on them differ. (Measured: a receiver
sealing one accepted claim under its own maintainer key got the sender's block and `heads/main`
`RefState` ids.) Sealing the patches in a different grouping, or onto a different tip, gives different
ids. A sender's tag and the receiver's adopted tag are different objects: `adopt-tag` writes a tag object
and `RefState` of its own, naming the same target block.

**One id, several signers, in format 7.** After the receiver has sealed the sender's patches as above,
importing the sender's bundle of that ref merges: each shared object keeps one stored copy carrying both
maintainers' signatures, and the import reports `objects gaining signatures: N`. The same happens in the
other order, when the receiver imports first and seals afterwards. What may join a stored object is
checked first: every incoming signature must verify, or the whole import or exchange is refused with
nothing written; a MAINTAINER signature is kept only if this repository has adopted its key, and is
otherwise dropped and named on a `signature not stored:` line; an AUTHOR signature needs key material
under the usual trust-on-first-use rules. Nothing arrives pre-trusted.

**An object carries at most 4 counted signatures when an import or exchange would store it.** Every
signature counts except a MAINTAINER signature by an adopted key that verifies; above the limit the whole
import or exchange refuses, naming the object, its count and `limit of 4`, and writes nothing. It applies
to new objects and merges alike. Local commands never check it and are never refused. `seal` signs with
an adopted maintainer key, which does not count; your own AUTHOR signatures do, so on an object whose set
they helped fill, another signer's copy is refused. See
[Trust and Threat Model](../reference/trust-threat-model.md). A format-6 repository — one
created by 0.44.0 or earlier and not yet upgraded — still holds one signed copy per id and refuses with
`existing container record for <id> differs from candidate`, naming `prikk format upgrade`.

**Divergence is reported, not treated as damage.** If an accepted patch does not apply to the
receiver's current tip, that means the two histories have moved differently since they last agreed —
ordinary, not corruption.

## Limits

- **No remote-tracking, no named remotes, no discovery.** Every step names a file explicitly; prikk
  remembers nothing about who you last synced with. Each round starts from nothing.
- **Each exchange costs O(history), not O(change).** A have-list is 32 bytes per patch reachable from
  the ref — at 100,000 patches, roughly 3&nbsp;MB, sent on every exchange regardless of how small the
  actual delta turns out to be.
- **Adopting a tag resolves by scanning local blocks, and that scan is superlinear.** Measured at
  roughly 12.6&nbsp;ms over 500 blocks and 86&nbsp;ms over 2000 in a single long branch. Do not expect
  it to stay fast as history grows.
- **The summary covers `heads/*` only.** `remotes/*` never appears in it — received, unsealed
  history is a separate namespace. **Tags are not listed in the summary, but they do travel in the
  build artifact and are adopted separately** (RFC 117) — a repository can receive and adopt tags
  even though `compare` never mentions them.

## Out of scope

Transport, remote-tracking, and tag deletion are not built. Nothing here documents them as
forthcoming.

## Claim-to-Source Anchors

| Claim | Source anchors |
|---|---|
| `sync` has nine subcommands — `summary`, `compare`, `have`, `build`, `accept`, `pending`, `seal`, `tags`, `adopt-tag` — each reading and writing local files only. | [`sync.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/sync.rs) |
| `summary` lists every `heads/*` ref with its patch-set digest and count; `remotes/*` never appears, and `tags/*` is filtered out deliberately. | [`summary.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/sync_negotiation/summary.rs) |
| `compare` reports each ref as `in-sync`, `differs`, `remote-only`, or `local-only`. | [`summary.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/sync_negotiation/summary.rs) |
| A have-list carries 32 bytes (one `ObjectId`) per patch reachable from the ref. | [`have_list.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/sync_negotiation/have_list.rs) |
| `build` computes the delta against a received have-list, signs one recognition claim per block the delta touches, and includes every local tag whose target block lies within the synced ref's ancestry — even when the patch delta itself is empty. | [`sender.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/sync_negotiation/sender.rs) |
| `build` prints a confidentiality notice — the artifact contains repository content in the clear and prikk does not encrypt it — whenever it writes one. | [`sync.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/sync.rs) |
| The exchange artifact (`PEXCH002`) carries patches, blobs, public AUTHOR key material, recognition claims, and Tag objects; author key material is `{key_id, public_key}` only, never a secret key or seed. | [`artifact.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/patch_exchange/artifact.rs), [`author_key_index.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/author_key_index.rs) |
| `accept` verifies and stores everything the artifact carries and reports signature outcomes; it adopts no key, advances no ref, and creates no local tag by itself. | [`accept.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/patch_exchange/accept.rs) |
| `pending` lists accepted patches not yet reachable from any local block. | [`patch_exchange.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/patch_exchange.rs) |
| `seal` turns accepted patches named by claim ids into sealed blocks under the receiver's own maintainer key, ordered by each claim's own signed parent relationships. | [`seal_from_accepted.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/seal_from_accepted.rs), [`recognition_claim.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/recognition_claim.rs) |
| `tags` lists every received-but-unadopted tag with a live signature outcome and current resolution state (`Resolved`, `NotHeld`, or ambiguous); `adopt-tag` resolves one by patch set and creates a **local** tag under the receiver's own key, refusing when the patch set is not yet held or resolves ambiguously. | [`tag_travel.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/tag_travel.rs) |
| Tag resolution by patch set is a full local-block scan, measured superlinear (~12.6 ms at 500 blocks, ~86 ms at 2000, one long branch). | [`patch_set_digest.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/patch_set_digest.rs), [RFC 117](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/117-tag-sync.md) |
| Negotiation-as-artifacts is the ruled next increment; prikk stays off the network and `prikk-store` stays bytes-in/bytes-out by design. | [RFC 116](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/116-sync-negotiation-and-transport.md) |
| A received Tag object is stored and reportable; sync never mints a local tag — adoption is a separate, explicit, receiver-signed act. | [RFC 117](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/117-tag-sync.md), [`tag_travel.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/tag_travel.rs) |
| Transport, remote-tracking, and tag deletion are not implemented. | [`sync.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/sync.rs), [RFC 116](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/116-sync-negotiation-and-transport.md) |

## Provenance

This guide covers RFC 116 (stages 1 through 7) and RFC 117 stage 3. It is documentation-only and
does not change CLI behavior, artifact format, signing, trust, or repository state.

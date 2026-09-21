# RFC 158 — Large objects: bounded, streamed, chunked, and reclaimable

**Status.** **PROPOSED 2026-09-21 by the architect**, on the owner's word after the measurement below: *"Record it to
make schedule and manage release cycles. We will have to carefully design for 'finally clean, safe and secure, robust
and sophisticated design'. Usability, function, data structure and life cycle are prioritized to initial cost."*

**The architect's reading of that instruction, stated so it can be corrected:** this RFC is designed for the end state,
not for the cheapest patch. Where a choice trades implementation cost against how the feature is used, what it can do,
how the data is shaped, or how content is kept and reclaimed over a repository's life, **the cost side loses**. It is
still delivered in stages, because each stage must be shippable and reviewable — but the stage boundaries are chosen so
that no stage has to be undone by the next.

Author-review independence: the architect proposes and will review; §9's controls compensate, each shown to fail.
**Scheduling and acceptance are the owner's** (§8 proposes an order).

## 1. Measured, not assumed — released 0.45.0 asset, 2026-09-21

A scratch repository, random binary content, peak resident set of the child process:

| Action | Result |
|---|---|
| commit a 32 MiB file | `.prikk` 33 MB — **no compression** |
| change **one byte**, commit again | `.prikk` 65 MB — **the whole file is stored again** |
| add a byte-identical 32 MiB copy | `.prikk` unchanged — **content addressing dedupes whole files** |
| `bundle export` of that history | 64 MB — **every version travels** |
| commit a 128 MiB file | **peak 644 MB ≈ 5× the file** |
| `verify` | 517 MB ≈ 4× |
| `seal`, `checkout --patch-materialize`, `show` | ≈ 260 MB ≈ 2× |

At source: `BlobPayload { blob_kind, content: Vec<u8>, declared_size }`
(`crates/prikk-object/src/payload/blob.rs:41`) holds the whole file in memory; the workspace has **no compression
dependency**; a binary change is a `ReplaceBinary` naming a new whole blob.

**Two consequences.**
1. **A product limit.** A 100 MB asset revised twenty times costs 2 GB, and a 1 GiB file needs roughly 5 GiB of RAM to
   commit.
2. **A robustness hole with a security edge.** Nothing bounds an object's size anywhere — not on commit, not on
   `bundle import`, not in `bundle verify`, which needs no repository. A bundle naming a multi-gigabyte blob is decoded
   into memory by the receiver. The failure is an out-of-memory kill, not a refusal: **the machine decides, not prikk.**

## 2. What this must be true of when it is finished

In the owner's order of priority.

**Usability.** A person with a 500 MB asset has a supported path, and is never told only "no". Every refusal names the
size it saw, the bound it applied, and where the bound comes from. Nothing about chunking or storage layout appears in
ordinary output; `commit`, `checkout`, `show`, `diff`, `tree` and `cat` read the same to a user as today.

**Function.** Large files commit, seal, verify, check out, bundle, sync and merge, with content unchanged byte for
byte. A repository holding them stays fully verifiable offline.

**Data structure.** The stored shape is deliberate and documented:
- a blob is a **manifest of content-addressed chunks**, not one opaque payload;
- the manifest carries the total size and the chunk list, so a reader knows the size before reading content;
- chunk boundaries are **content-defined**, so an insertion near the start does not rewrite every later chunk;
- every existing id keeps its meaning: a blob id is still the id of the file's content.

**Lifecycle.** A repository must be able to **stop paying** for content it no longer needs, without weakening what
`verify` can prove. That means an explicit, refusing-by-default reclamation verb, a defined notion of what is
reachable, and a documented record of what was reclaimed. Nothing is ever removed implicitly.

**Security and robustness.** Untrusted input cannot make prikk allocate without bound; a declared size is never
trusted before it is checked; a partial or interrupted write leaves the repository exactly as it was; and every chunk
is verified against its id when read, so a damaged or substituted chunk is caught, not served.

**Initial cost is the last consideration**, by the owner's ruling.

## 3. The data structure

**Blob schema 2 — a chunk manifest.**

```
BlobPayload v2 { blob_kind, total_size: u64, chunks: [ChunkRef] }
ChunkRef      { chunk_id: ObjectId, size: u32 }
Chunk (new object type) { bytes }
```

- **The blob id keeps its meaning**: it is the id of the file's content, derived from the canonical manifest, which is
  itself a function of the content alone. Two identical files still share one blob id, as measured in §1.
- **A chunk is an object** like any other: content-addressed, stored in its own container, deduped by id. An unchanged
  region shared between two versions of a file is stored once.
- **Boundaries are content-defined**, by a rolling hash over a window with a documented minimum, target and maximum
  chunk size. The algorithm is written in this workspace, deterministic and dependency-free, and its parameters are
  frozen by the schema: **two runs of any prikk version produce the same chunks for the same bytes**, which is what
  keeps ids stable.
- **Small content stays one chunk.** Below the minimum size, a file is a single-chunk manifest, so ordinary source
  files pay one extra indirection and nothing else.
- **Compression is not in schema 2.** It is a separate later question (§7), because a compressed chunk changes what a
  chunk id means and adds a decompression bound to every read.

**This is a repository-format change**, so it lands as **format 8** under RFC 114, reached by the same explicit
`prikk format upgrade` that format 7 uses (RFC 114 §5.2a), never automatically, with the same "older prikk cannot open
it" rule. An upgrade **converts nothing**: existing schema-1 blobs stay readable for ever, and only new content is
written as schema 2. A reader must therefore handle both, permanently.

## 4. Streaming

No operation may hold a whole file in memory.

- **Writing:** the worktree file is read in chunk-sized pieces; each chunk is hashed, written, and dropped; the
  manifest accumulates ids. Peak memory is O(chunk size + manifest), not O(file).
- **Reading:** `checkout` materialization, `bundle export`, `sync build` and `cat` write chunk by chunk to their
  destination, verifying each chunk against its id as it goes.
- **A destination file is written to a temporary sibling and renamed**, so an interrupted read leaves no partial file
  where a whole one is expected.
- **`verify`** checks a manifest by streaming its chunks and comparing each id; it never materializes the file.
- The measured 5×, 4× and 2× multipliers of §1 become a constant, and the RFC 133 instrument gains a binary row.

## 5. Bounds — what refuses, and who decides

1. **A per-object bound applies to anything arriving from outside**: `bundle import`, `sync accept`, and
   `bundle verify`, which has no repository and must still refuse before allocating. The bound is checked against
   the **declared** size first, so nothing large is allocated to discover it is large, and then enforced again while
   streaming, so a lying declaration fails.
2. **A repository's own commit is not bounded by default.** A person committing their own 2 GiB file is not an attack;
   with §4's streaming it is a supported operation. Usability first.
3. **The bound is a documented default that an operator can raise or lower**, and the refusal names the current value
   and how to change it. prikk has no configuration file yet (ROADMAP §D, RFC 135 §9.1), so this RFC needs one
   decision from the owner: an environment variable now, or `prikk config` finally built (§8, question 1).
4. **Every refusal writes nothing**, in the 0.44.0 sense: it happens before the first write.

## 6. Lifecycle — reclaiming what is no longer needed

Today nothing is ever removed: containers are append-only, and no verb reclaims space. With large objects that becomes
the dominant cost, so this RFC designs reclamation rather than leaving it to a later emergency.

- **Reachability is defined**: a chunk is reachable if any blob manifest reachable from any ref, any received pointer,
  any tag, the active WAL, or any rollback draft names it. Anything else is unreachable.
- **`prikk reclaim` (name to be ruled) is explicit, refuses by default, and never runs implicitly.** It plans first
  (`--plan-only`, like `checkout`), names every object it would remove and the space it would free, and requires a
  second, explicit act to proceed.
- **It refuses while anything is in flight**: an active WAL with records, an incomplete publication, a provisional
  worktree, or a failing `verify`.
- **It writes a record of what it removed** — ids and sizes — so `doctor` can explain a later "missing object" as a
  reclamation rather than damage, which is the difference between a diagnosis and a mystery.
- **What it must never break:** signatures stay verifiable, history stays walkable, and a reclaimed repository still
  passes `verify`. A chunk a *received* pointer needs is reachable; relaying history is not a reason to lose it.
- **Out of scope here, and said so:** any policy that decides *for* the user what is old enough to remove.

## 7. Deliberately not in this RFC

- **Compression** (§3). It changes what a chunk id covers and needs a decompression bound; it is a separate RFC, after
  chunking is measured.
- **An external or remote content store** — the true LFS analogue. prikk's transport is a file (RFC 116 §1), and a
  fetch-on-demand store would touch trust, offline verification and every read path. Chunking is the prerequisite for
  it, and this RFC is designed so that a later store can address chunks without changing the manifest.
- **Rename or similarity detection over binaries.**

## 8. What the owner rules

1. **Where the bound lives** (§5.3): an environment variable now, or `prikk config` built as part of this work.
   The architect recommends **building `prikk config`**: a durable per-repository bound is exactly the "first real
   adopter" trigger RFC 135 §9.1a names, and an env-only bound is invisible to the next person in the repository.
2. **The name of the reclamation verb** (§6): `prikk reclaim`, or another word.
3. **Scheduling.** The architect proposes:

   | release | content |
   |---|---|
   | **0.46.0** | unchanged: `diff`, `tree`, `cat` |
   | **0.47.0 "depth"** | RFC 136 increment 2c as scheduled, **plus Stage A of this RFC**: the import/verify bound and its refusals. Small, self-contained, and it closes the out-of-memory hole against untrusted input |
   | **0.48.0 "large objects"** | Stage B (streaming, no format change) and Stage C (format 8: chunk manifests, with the explicit upgrade) |
   | **0.49.0** | Stage D (reclamation) — and RFC 155, then RFC 154, move behind it |

   Stage A before Stage B is deliberate: a refusal that names a bound is honest on the day it ships, and streaming
   changes nothing a user sees.

## 9. Stages and controls

Each stage is one round, reviewed before the next. Every control must be shown failing.

**Stage A — the bound (no format change).**
- Controls: a bundle declaring a blob above the bound refuses, naming size and bound, **before any allocation of that
  size** (measured peak stays flat); the same through `sync accept`; `bundle verify` refuses with no repository; a
  declared size that lies is caught while streaming; a bundle at exactly the bound imports; a local `commit` of the
  same size is **not** refused.
- Perturbations: the bound checked after reading; the declared size trusted.

**Stage B — streaming (no format change).**
- Controls: commit, verify, checkout, export and import of a file **large enough that the old multiplier would show**
  (the measurement of §1, repeated) with peak memory bounded by a constant, not by the file; content byte-identical;
  an interrupted write leaves no partial destination file.
- Perturbation: one path buffers the whole file again — the memory control fails.

**Stage C — chunk manifests, format 8.**
- Controls: a one-byte change in a large file stores **only the affected chunks** (measured, against §1's full copy);
  identical files still share one blob id; chunk boundaries are stable across runs and across platforms; a schema-1
  blob written by 0.47.0 still reads after the upgrade; 0.47.0 refuses a format-8 repository naming the upgrade; a
  damaged chunk is caught by id on read and reported by `verify`; a bundle carries only the chunks the receiver lacks.
- Perturbations: fixed-size chunking (the insertion control fails); the chunk id unverified on read.

**Stage D — reclamation.**
- Controls: an unreachable chunk is planned and removed; a chunk reachable only from a received pointer, a tag, a
  rollback draft or the WAL is **not**; `verify` passes after reclamation; `doctor` explains a reclaimed object;
  reclamation refuses while a WAL is active, a publication is incomplete, or the worktree is provisional; a killed
  reclamation leaves the repository verifiable.
- Perturbations: each reachability source dropped in turn — each must fail its own control.

**Docs:** a `guide/large-files.md` written for the person with the 500 MB asset; `data-model.md` and
`repository-layout.md` for the manifest; `release-compatibility.md` for format 8; `non-goals.md` corrected, since
"no large-object handling" stops being true.

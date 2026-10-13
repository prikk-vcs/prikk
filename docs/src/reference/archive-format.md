# The archive format, `PREPO001`

`prikk archive export <file>` writes a whole repository, verbatim, into one file (RFC 155).
`prikk archive verify <file>` reads one back, streamed, and checks structure, reference closure, and
every signature against the archive's own carried key material. `prikk archive import <file>
[--adopt]` reads the same file and lands it. This page documents the byte layout all three commands
use — the format itself, not the command surface (see [commands](commands.md) for that).

## Shape

```
PREPO001 (8 bytes, the whole file's own magic)
section 1
section 2
...
section N
manifest
end trailer (56 bytes, fixed)
```

Nothing is random-accessed to write it: every section streams forward, in fixed-size chunks, from
one on-disk source file to the output. A reader finds the manifest without scanning the whole file:
seek to `length - 56`, read the end trailer, then seek to the manifest's own offset.

## One section per carried file

| field | bytes | notes |
|---|---:|---|
| magic | 8 | always `PRPOSEC1` |
| kind | 2 | a fixed numeric code (below) — never a carried path |
| `body_len` | 8 | known from a stat before any body byte is read |
| body | `body_len` | verbatim bytes of one source file, or a resolved family's sound prefix |
| checksum | 32 | SHA-256 of `magic ‖ kind ‖ body_len ‖ body`, **after** the body — a trailer, not a header, so the encoder never needs the whole body in hand before writing the header |

**An object-container section classifies its own frames exactly as the live store does** (RFC 162
rule 2), held equal to that real decoder by a dedicated test rather than by a shared function call:
a complete frame — a full header and its full claimed body are both present — that fails its own
checksum is damage, named by `archive verify` and refused by `archive import`. An *incomplete*
frame — a crash tail in the source repository's own live container, or bytes added since, carried
verbatim because object containers travel damage included — is named but never refused on: the
source repository's own `prikk verify` only ever warns about this shape, so a backup of that
repository must still restore. **The overrun case**: a frame whose checksum fails is still
*incomplete*, not damage, when a later, genuinely sound frame starts inside the byte range its own
header claimed — that only happens when the first frame was torn (a crash mid-append) and a later,
real write finished past it, exactly as the live store's own resync-and-continue reading already
handles for its own container file. The same bytes get the same outcome whether a crash or a hand
wrote them.

### Section kinds

| code | kind | content |
|---:|---|---|
| 0 | Patch container | verbatim, live slot's bytes (object containers never compact — always slot `a`) |
| 1 | Block container | verbatim |
| 2 | RefState container | verbatim |
| 3 | Tag container | verbatim |
| 4 | Attestation container | verbatim |
| 5 | Blob container | verbatim |
| 6 | RecognitionClaim container | verbatim |
| 7 | Ref log | verbatim, slot `a` (never compacted) |
| 8 | Ref pointer index | **resolved** content only — no slot letter, no generation log |
| 9 | Received index | **resolved** content only |
| 10 | Trust policy | **resolved** content only — carried as an inert record, never adopted on import |
| 11 | Maintainer keys | verbatim — carried as an inert record, never adopted on import |
| 12 | Author keys | verbatim — recorded on import, conflicts refused and named |

Every repository this format carries from writes exactly 13 sections, in the order above
(`ArchiveSectionKind`'s own fixed list — `crates/prikk-store/src/archive.rs`).

**Damage in an object container, the ref log, the maintainer record, or the author-key material
travels verbatim** — the archive is a faithful copy, damage included, and `archive verify` names it
rather than refusing the export. **A slotted family that cannot be resolved** (an ambiguous lost
generation log, or genuine interior damage in the resolved content) **refuses the whole export
before anything is written**, naming the existing `doctor`/`verify` texts for that state.

## The manifest

Written once, after every section, so every section's own checksum is already known:

| field | bytes |
|---|---:|
| magic | 8 (`PRPOMAN1`) |
| `repository_format` | 4 |
| `tool_version_len` | 2 |
| `tool_version` | `tool_version_len` |
| `section_count` | 8 |
| entries | `50 × section_count` |

Each entry: `kind(2) ‖ offset(8) ‖ length(8) ‖ checksum(32)` — the section's own header-to-trailer
span in the file, and its checksum, so a verifier can check every section's structural soundness
without decoding the section headers itself.

## The end trailer

Fixed 56 bytes, always the file's last 56: `magic(8, "PRPOEND1") ‖ manifest_offset(8) ‖
manifest_length(8) ‖ manifest_checksum(32)`.

`manifest_checksum` is SHA-256 of the manifest body (`magic ‖ repository_format ‖
tool_version_len ‖ tool_version ‖ section_count ‖ entries`) — the manifest cannot checksum itself
in its own body, so the checksum lives in the trailer instead, written last, after the manifest
body is already known in full (RFC 155 implementation Part V2: the manifest itself was not
integrity-checked before this, so a flipped byte inside it — `repository_format`, in particular —
passed `archive verify` as consistent).

## `archive verify`

Reads the end trailer and the manifest first (seek, never scan), then checks:

- **the manifest's own checksum**, before any of its fields (including `repository_format`) are
  read as anything but raw bytes — a mismatch refuses the whole archive by name;
- **the manifest's own `repository_format`** is one this build actually knows how to read (today:
  6 or 7) — an archive legitimately written by some other build, naming a format number this one
  predates, is refused by name rather than silently misread;
- **structure** — every section's own checksum matches the manifest's recorded one, the fixed set
  of thirteen section kinds is present exactly once each (an unknown, duplicated, or missing kind
  refuses before any section body is read), and the manifest's own declared layout reaches exactly
  the archive's real byte length;
- **reference closure**, in two passes over the seven object sections: the first builds a set of
  every carried object's own id (32 bytes each, never the object's bytes); the second re-streams and
  checks every reference a `Block`/`Patch`/`Tag`/`Attestation`/`RefState`/`RecognitionClaim` makes
  against that set, naming any that is absent;
- **every signature**, against the archive's own carried author-key and maintainer material — small
  key material, decoded once into memory, never the local repository's. A signature by a key this
  archive carries, that fails to verify, is damage, named, and refuses the whole archive — the one
  case this check actually catches: a tamperer who left the signature's own key in place but
  altered the signed bytes. A signature by a key this archive does **not** carry — including every
  signature at all, if the carried key section is empty or was swapped for something else — is
  named *"not checked"* and leaves the archive internally consistent; it is not a weaker version of
  the real check, because the real check happens only after import, against the **receiving**
  repository's own adopted set (R2). Stripping or swapping the carried key section therefore proves
  nothing either way about an archive's own trustworthiness, by design: `archive verify` answers
  only *"is this archive internally consistent with itself,"* never *"should you trust it."* The
  report always shows both counts together (*"signatures checked: N, not checked: M"*, in text and
  `--format json`), and names what a nonzero `not_checked` count means, so a reader never has to
  guess whether 0 of 16 or 16 of 16 signatures were actually checked against carried material.

It reports *internally consistent*, **never** *trusted*: trust comes only from a receiver's own
adopted set, checked after import. That note is printed — in text and in `--format json` — only
when the archive genuinely is internally consistent; a damaged or unresolvable archive gets its
damage lines and the error, never a note beside them that contradicts them. The carried maintainer
keys are listed, labeled *"carried, not adopted."* It writes nothing, on any exit path — no scratch
file, no scratch directory.

The manifest's own declared `repository_format` number (checked above, against what this build
knows how to read) is purely informative past that check: `archive import` always writes the
**receiving** repository's own real, current format, never the carried number. A format-6 archive
built from format-7 bytes, or any other mismatch between the carried number and the carried
content's own real shape, changes nothing about what import actually writes.

## Carry-forward

A later `PREPO` version keeps this magic's reader forever (RFC 114's stability contract): the build
that ships the next bump must still read a `PREPO001` file written by the last build before it,
proven by a fixture built from the *old* encoder — `decode_bundle`'s own precedent
(`crates/prikk-store/src/bundle.rs`), not hand-crafted bytes.

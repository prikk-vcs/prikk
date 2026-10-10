# The archive format, `PREPO001`

`prikk archive export <file>` writes a whole repository, verbatim, into one file (RFC 155). This
page documents the byte layout `export_archive` writes and a future `archive verify`/`archive
import` will read — the format itself, not the command surface (see
[commands](commands.md) for that).

## Shape

```
PREPO001 (8 bytes, the whole file's own magic)
section 1
section 2
...
section N
manifest
end trailer (24 bytes, fixed)
```

Nothing is random-accessed to write it: every section streams forward, in fixed-size chunks, from
one on-disk source file to the output. A reader finds the manifest without scanning the whole file:
seek to `length - 24`, read the end trailer, then seek to the manifest's own offset.

## One section per carried file

| field | bytes | notes |
|---|---:|---|
| magic | 8 | always `PRPOSEC1` |
| kind | 2 | a fixed numeric code (below) — never a carried path |
| `body_len` | 8 | known from a stat before any body byte is read |
| body | `body_len` | verbatim bytes of one source file, or a resolved family's sound prefix |
| checksum | 32 | SHA-256 of `magic ‖ kind ‖ body_len ‖ body`, **after** the body — a trailer, not a header, so the encoder never needs the whole body in hand before writing the header |

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
travels verbatim** — the archive is a faithful copy, damage included, and a reader's own `verify`
(a later part) names it rather than refusing the export. **A slotted family that cannot be
resolved** (an ambiguous lost generation log, or genuine interior damage in the resolved content)
**refuses the whole export before anything is written**, naming the existing `doctor`/`verify`
texts for that state.

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

Fixed 24 bytes, always the file's last 24: `magic(8, "PRPOEND1") ‖ manifest_offset(8) ‖
manifest_length(8)`.

## Carry-forward

A later `PREPO` version keeps this magic's reader forever (RFC 114's stability contract): the build
that ships the next bump must still read a `PREPO001` file written by the last build before it,
proven by a fixture built from the *old* encoder — `decode_bundle`'s own precedent
(`crates/prikk-store/src/bundle.rs`), not hand-crafted bytes.

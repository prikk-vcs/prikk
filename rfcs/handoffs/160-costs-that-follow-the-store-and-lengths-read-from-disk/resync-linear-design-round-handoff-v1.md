# Resynchronisation is linear — RFC 167 design round, handoff v1

**Live 2026-10-05, and it is next.** 0.49.0 step 3 (RFC 166) is closed: review
`.git-exclude/reviewed/rfc166-round-2-review-v2.md`.
- **Why it is filed here:** M5 is RFC 160 F3's scan, and RFC 160's runaway-guards round deferred its fix to 0.49.0.
  RFC 167 is proposed, and a proposed RFC carries no handoffs.
- **This is a design round: measure, prototype, report, then stop. No product code lands.**

**Read first:**
- `rfcs/accepted/167-resynchronisation-is-linear.md`, all of it;
- RFC 160 F3 and its M5 notes;
- `.git-exclude/reviewed/runaway-guards-review-v1.md` and `-v2.md` (the budget sketch, the two shapes);
- external review 014's M5 section, with its generator:
  `.git-exclude/upstream/external-architect/receive/014-review-before-0-48-0-findings-and-answers/reproduce/`.

**The rules carried from RFC 166's rounds:**
- **A cut is a question to the architect before delivery,** at ×2 of a unit's budget.
- **Every measurement:**
  - names its binary: release, `cargo build --release -p prikk --locked`, sha256 stated;
  - **is taken on `/home`, with `stat -f -c %T` printed;**
  - prints its calibration line first (exit status, duration);
  - **compares against `main`**, not against the new binary with something switched off.
- **Every run inside an R1 scope,** with a memory ceiling and a timeout. A hostile input can run for hours: set the
  timeout first.
- **Before writing a claim about what code does, read the code** and cite the line.
- **The prototype is kept** in `/home/nabbisen/Desktop/prikk/scratch-167/proto`, with its own `CARGO_TARGET_DIR`,
  until RFC 167 is accepted.
- **No selector or environment switch that falls back silently.** If prototypes need one, an unknown value refuses.

## 1. What to answer

**RFC 167 §5, all seven questions.** Each is answered from source and backed by a measurement.
1. **Q1, the cost per reader:**
   - one table: reader × shape (A, B) × size (32 KiB, 256 KiB, 2 MiB, 64 MiB) → bytes hashed and seconds, on `main`;
   - **64 MiB may take hours on `main`:** measure up to the size that fits a 10-minute timeout, and state the fit;
   - for each reader, the commands that can bring it hostile input from outside the repository.
2. **Q2, the budget:**
   - prototype it in `frame_resync`, shared by all ten readers;
   - the same table on the prototype;
   - **every call site's handling of "undetermined", read from source and listed.**
3. **Q3, the WAL's witness:** from source, and prototyped if the answer is yes.
4. **Q4, honest data:**
   - the existing suite, the RFC 133 corpus, `matrix.py` against `prikk-83a42498`'s run, and a repository whose
     committed files contain thousands of frame magics;
   - **the honest crash tail full of magics,** with and without the budget. Build it from a real commit whose
     content is full of `PWALR001` and container magics, torn by a real kill; do not construct the bytes by hand.
5. **Q5, the way out,** per reader.
6. **Q6, the command-level guard:** a `verify` row with a control (a second decode added) that turns it red.
7. **Q7, the records.**

**List first any case where no option meets C1–C6.** The architect rules on those before anything else.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| U1 | Q1: the cost table on `main` | 60 min |
| U2 | Q2 and Q3: the budget prototype, the witness question, every call site | 120 min |
| U3 | Q4 and Q5: honest data, the crash tail full of magics, the ways out | 90 min |
| U4 | Q6 and Q7 | 45 min |

**Nothing else runs while timings do.**

## 2. Report

- Answers to Q1–Q7, with the tables, each option laid out, and **no option chosen**: the architect rules.
- Each unit's real start and end.
- The primary tree is clean.
- **Report:** `.git-exclude/review-request/rfc167-design-round-report-v1.md`.

**Design round ACCEPTED 2026-10-05** (review `rfc167-design-round-review-v1`). RFC 167 is rewritten as a design for the
owner's reading. **No implementation handoff until the owner accepts it.**

# Prikk Measurement Corpus

Repository-internal tooling for RFC 139 (the measurement corpus). Not part of the shipped product;
not built by default (`tools/corpus` is a workspace member but not a `default-members` one).

A **profile** is a small, human-readable TOML document describing a real history's *shape* — never
its content, never its paths (RFC 139 §4). [`plan`](src/plan.rs) turns a profile into an **action
manifest** — the ordered sequence of filesystem operations a build would perform — computed without
executing anything. [`execute`](src/execute.rs) drives a real `prikk` binary to turn a manifest into
a real, throwaway repository, the same CLI surface a user would drive (RFC 139 §7). This crate also
holds the profile format and the **extractor** that derives one from already-captured `git log`/`git
ls-tree` text; it never spawns `git` itself.

## Build-cost curve

`tests/build_cost_curve.rs` is RFC 139 §6's own measurement instrument — wall clock and peak RSS
against sealed depth, using `profiles/prikk-self.toml`. `#[ignore]`d by default (expensive); run
deliberately with `--ignored`. See its own module doc, and
`rfcs/handoffs/139-measurement-corpus/build-cost-curve-report-v1.md` for the most recent measured
curve.

## Re-deriving `profiles/prikk-self.toml`

The profile's own `provenance.extraction_commands` names the exact commands, verbatim. From the
repository root, at the revision named in `provenance.revision`:

```console
git log --pretty=format:'@@%H' --name-status --no-merges -n 600 > /tmp/prikk-self-log.txt
git ls-tree -r -l <revision> > /tmp/prikk-self-ls-tree.txt
```

Then run the extractor against those two files and the committed context recipe:

```console
cargo run --locked -p prikk-corpus --bin extract-profile -- \
  /tmp/prikk-self-log.txt /tmp/prikk-self-ls-tree.txt \
  tools/corpus/profiles/prikk-self.context.toml \
  --out /tmp/prikk-self.toml
diff /tmp/prikk-self.toml tools/corpus/profiles/prikk-self.toml
```

An identical `diff` confirms the committed profile matches what the recorded commands actually
produce today.

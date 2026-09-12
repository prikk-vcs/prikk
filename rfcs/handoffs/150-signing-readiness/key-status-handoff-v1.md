# RFC 150 — `prikk key status`: handoff v1

**HELD until the owner accepts RFC 150.** Scheduled for 0.41.0 if accepted; the retired-variable
detection removal (0.41 item 0) lands in the same release and not before this.

## 1. The change

`prikk key status [path] [--role author|maintainer] [--format json]` per RFC 150 §2, both roles by
default. Implement on `key_material` — `seed_path`, the mode rule, the retired-variable check, the
decode — as *queries* that return a status rather than an error, so `commit`/`seal` and `key status`
cannot drift: one function computes `source`/`usable`/`reason`; the signing path calls it and turns a
not-usable status into the existing refusal messages. `binding` reuses the same lookups `commit`
(author key record) and `seal` (trust store) already perform. JSON emitter hand-built,
`"schema_version": "key-status-v1"` first. `key public`'s missing-file message routes through the same
query (RFC 150 §3). Synopsis in `commands.rs` and `commands.md`.

## 2. Controls

1. **Both directions of stikk's table**: keys in the directory, nothing exported → `usable: true`,
   `source: key-directory`, exit 0; stale `PRIKK_AUTHOR_SEED` set → `legacy_variable_set: true` and the
   same fields still computed, exit 0.
2. Override set and missing → `source: seed-file-override`, `usable: false`, `reason: override-missing`,
   `path` names the override, the default file **not** named.
3. Mode `0644` → `usable: false`, `reason` names the mode.
4. **Binding**: author fresh id → `unrecorded`; after one commit → `matches`; a different seed under
   that id → `mismatch` **and** `commit` with that seed still refuses (assert both, from one fixture);
   maintainer unadopted → `not-adopted`; adopted → `matches`; different seed → `mismatch` and `seal`
   refuses.
5. `--format json` succeeds in every state prose does; `jq` the `schema_version`; prose and JSON agree
   on `usable` per role (assert from one run).
6. **Perturb** the shared query so `commit` and `key status` disagree — one test must fail. If none
   does, the two are not sharing the computation.
7. Full gate set; the isolation seam applies to every test here; cross-target from the diff
   (`key_material.rs` is platform-conditional — run both).
8. Docs: `security-setup.md` gains the command as the way to ask "can I sign here"; `troubleshooting.md`
   points the three RFC 148 refusals at it; CHANGELOG `### Added`, naming stikk as the consumer.

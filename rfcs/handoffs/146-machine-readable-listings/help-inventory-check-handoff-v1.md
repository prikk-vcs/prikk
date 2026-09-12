# RFC 146 §8e companion — the help inventory and the docs inventory must agree

**Ruled 2026-09-12** at the 0.41.0 prep review. **Live; small; before the 0.42.0 prep.**

One test beside `rfc146_help_synopsis_advertises_format_json.rs`, reusing its `invocation_path` parser:
the set of invocation heads in `prikk --help` (`prikk <cmd> [<sub>]`) equals the set in
`docs/src/reference/commands.md`. Symmetric difference must be empty; the failure names both sides. It
would have caught `bundle preview`'s two-release absence from the inventory and `--version`'s.

Controls: perturb both directions (delete a line from `commands.md`; add a fake one) — each fails by
name; assert more than twenty heads were parsed so an empty parse cannot pass. Full gate set; no `cfg`.
Not in scope: argument shapes (that stays the parser check in the release-prep sweep) or descriptions.

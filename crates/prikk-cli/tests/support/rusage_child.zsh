#!/usr/bin/env zsh
# RFC 133 §6d.3 -- exact peak RSS of one child process, via zsh's TIMEFMT %M (sourced from the same
# getrusage(RUSAGE_CHILDREN) syscall rusage_child.py already uses), with a spawning process an order
# of magnitude smaller than Python's.
#
# WHY THIS SCRIPT EXISTS ALONGSIDE rusage_child.py, NOT INSTEAD OF IT: the §6d.1 review correctly
# found the object-index probe's readings implausibly low below N=64,000 and hypothesised the
# measured child binary was too large -- reusing the whole compiled test binary (via self-reexec)
# carries every linked crate and the full libtest harness, a real cost. §6d.3's own handoff
# accordingly asked for a minimal companion binary (this repository now has one:
# `rusage-object-index-probe`, gated behind the `rusage-probe` feature).
#
# THAT FIX ALONE DOES NOT WORK -- confirmed empirically, not assumed, before committing to this
# script: spawning the new minimal binary through rusage_child.py *still* reads ~11-12 MiB, and so
# does spawning /usr/bin/true, and so does spawning a bare `fn main(){}` Rust binary compiled with no
# dependencies at all. The floor tracks Python's OWN resident size at fork time (~10-12 MiB measured
# directly via `resource.getrusage(RUSAGE_SELF)` immediately before the fork), not anything about
# the child. This is a real, known Linux characteristic of fork()+exec(): a freshly-exec'd child's
# own peak-RSS accounting is bounded below by whatever the *parent's* RSS was at the moment of
# fork() (the child's mm briefly shares the parent's pages via copy-on-write before exec() replaces
# the address space, and the kernel's hiwater_rss tracking can latch onto that). So the measured
# child's own content was never the problem below N=64,000; the size of whatever spawns it is.
#
# zsh's own idle RSS is roughly 1.6-2 MiB (measured the same way, via `TIMEFMT` against
# `/usr/bin/true`) -- about a sixth of Python's, small enough that even the N=100 point's ~14 KiB
# structure and the N=1,000 point's ~130 KiB one clear it by a comfortable margin, verified against
# the standing control this round adds (§3's own resident->=serialized-size assertion).
#
# This is a NEW environmental prerequisite (`zsh`, alongside the existing `python3`/Linux-only
# ones), used only by the one probe that needs a floor this small; every other measurement in this
# file keeps using `rusage_child.py` unchanged, per the handoff's own "do not re-run the other two
# rounds' series" instruction -- their signal sizes (tens of MiB and up) are not distorted by an
# 11 MiB floor the way this probe's few-hundred-KiB-to-low-single-digit-MiB signal was.
#
# Usage: rusage_child.zsh <cwd> <binary> [args...]
# Prints exactly one line to stdout: the child's peak RSS in KiB. Exits non-zero, with the child's
# own stderr forwarded, if the child itself failed. `set -e` is deliberately not used at the top
# level: under it, zsh exits the whole script the instant the timed block itself returns non-zero,
# before this script's own failure-handling ever runs -- confirmed by hitting exactly that during
# development, not assumed safe.

cwd="$1"
binary="$2"
shift 2

report_file=$(mktemp)
stderr_file=$(mktemp)
trap 'rm -f "$report_file" "$stderr_file"' EXIT

TIMEFMT='%M'
exit_status=0
{
  time ( cd "$cwd" && exec "$binary" "$@" 2>"$stderr_file" )
} 2>"$report_file" || exit_status=$?

if [[ $exit_status -ne 0 ]]; then
  cat "$stderr_file" >&2
  exit $exit_status
fi

cat "$report_file"

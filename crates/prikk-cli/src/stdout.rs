//! RFC 121 §2.1: a closed stdout is not an error.
//!
//! `std::println!`/`std::print!` panic when the write returns `EPIPE` (`ErrorKind::BrokenPipe`),
//! which is Rust's default `SIGPIPE` disposition (`SIG_IGN`) turning what would otherwise silently
//! kill the process into an `io::Error` instead. This is the ordinary shape of `| head`, quitting
//! `less` early, or `| grep -q .` -- the consumer asked for less output than there was and got it,
//! which is success (`0`), not a panic and a backtrace hint.
//!
//! Every `print!`/`println!` call site in this crate should route through the macros here instead
//! of the standard library's own, via `use crate::stdout::{print, println};` at the top of the
//! file: importing an item named `println`/`print` into scope shadows the language prelude's macros
//! of the same name for that file, so no call site itself needs to change -- only the import at the
//! top, which is what keeps this a small diff over ~400 call sites rather than a mechanical rewrite
//! of every one of them.

use std::io::{self, ErrorKind, Write as _};

macro_rules! println {
    () => {
        $crate::stdout::write_line(::std::string::String::new())
    };
    ($($arg:tt)*) => {
        $crate::stdout::write_line(::std::format!($($arg)*))
    };
}

macro_rules! print {
    () => {
        $crate::stdout::write_str(::std::string::String::new())
    };
    ($($arg:tt)*) => {
        $crate::stdout::write_str(::std::format!($($arg)*))
    };
}

pub(crate) use print;
pub(crate) use println;

/// What a failed stdout write means: silence (the reader is gone and does not care), or a real
/// failure that must not be swallowed alongside it. Kept separate from acting on the outcome so the
/// classification itself -- the part that decides whether this fix is narrow -- is unit-testable
/// without touching a real file descriptor or `std::process::exit`.
enum WriteOutcome {
    Ok,
    ClosedPipe,
    Failed(io::Error),
}

fn classify(result: io::Result<()>) -> WriteOutcome {
    match result {
        Ok(()) => WriteOutcome::Ok,
        Err(err) if err.kind() == ErrorKind::BrokenPipe => WriteOutcome::ClosedPipe,
        Err(err) => WriteOutcome::Failed(err),
    }
}

/// Writes one line to stdout. See the module doc for the `BrokenPipe` behaviour.
pub(crate) fn write_line(line: String) {
    write_and_handle(|stdout| writeln!(stdout, "{line}"));
}

/// As [`write_line`], without the trailing newline.
pub(crate) fn write_str(text: String) {
    write_and_handle(|stdout| write!(stdout, "{text}"));
}

/// Writes raw bytes to stdout, flushed (RFC 157 §4's `prikk cat`, the one command whose output is a
/// file's own bytes rather than a rendered line). The same `BrokenPipe` behaviour as [`write_line`]:
/// a reader that stopped reading is not this process's failure.
pub(crate) fn write_bytes(bytes: &[u8]) {
    write_and_handle(|stdout| stdout.write_all(bytes).and_then(|()| stdout.flush()));
}

fn write_and_handle(write: impl FnOnce(&mut io::StdoutLock<'_>) -> io::Result<()>) {
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    match classify(write(&mut lock)) {
        WriteOutcome::Ok => {}
        // A closed reader is not this process's failure -- exit exactly as if the requested output
        // had been produced in full and the process ended normally.
        WriteOutcome::ClosedPipe => std::process::exit(0),
        // Anything else (e.g. `ENOSPC` on a redirected stdout) is a genuine failure and must not be
        // swallowed by the same arm that swallows `BrokenPipe`. RFC 121 §6a's exit-code contract
        // rules 0/1/2 as the whole vocabulary -- a panic's exit 101 fell outside it, so this now
        // reports and exits 1 like every other operational failure, instead of a panic banner and a
        // backtrace hint. The result of the `writeln!` below is deliberately ignored, not just the
        // write that failed above: when stdout and stderr fail together (e.g. both redirected to a
        // full device), `eprintln!` itself panics on the write failure, which is the AUD-09 case --
        // the two failures share a cause rather than being independent, so a working stderr cannot be
        // assumed just because the stdout write failed for an unrelated reason. There is no third
        // stream to report the failure of the failure report on, and the contract only requires the
        // exit code stay inside 0/1/2 even when the message cannot be delivered. Deliberately
        // `std::process::exit`, not a threaded `Result` -- this runs from macro expansions at ~400
        // call sites, exactly why `ClosedPipe` already exits in place instead of propagating.
        WriteOutcome::Failed(err) => {
            let _ = writeln!(io::stderr(), "error: failed printing to stdout: {err}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
#[path = "stdout/tests.rs"]
mod tests;

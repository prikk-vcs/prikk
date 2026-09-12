//! Repository-internal release-policy verification commands.

#![forbid(unsafe_code)]

mod args;
mod boundary;
mod command_scan;
mod error;
mod installer;
mod json;
mod oracle;
mod policy;
mod reference;
mod release_evidence;
mod release_notes;
mod schema;
mod size;
mod time;

use std::process::ExitCode;

fn main() -> ExitCode {
    match args::run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

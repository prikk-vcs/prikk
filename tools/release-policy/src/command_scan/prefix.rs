//! Closed command-prefix grammar for governed shell and workflow procedures.

pub(super) fn command_head(tokens: &[String]) -> Result<Option<(usize, &String)>, &'static str> {
    let mut index = match tokens {
        [sequence, run, ..] if sequence == "-" && run == "run" => 2,
        [run, ..] if run == "run" => 1,
        _ => 0,
    };
    loop {
        let Some(token) = tokens.get(index) else {
            return Ok(None);
        };
        if assignment(token) {
            index += 1;
            continue;
        }
        match token.as_str() {
            "env" => index = env_tail(tokens, index + 1)?,
            "command" => index = command_tail(tokens, index + 1)?,
            _ => return Ok(Some((index, token))),
        }
    }
}

pub(super) fn inert_head(token: &str) -> bool {
    matches!(
        token.rsplit('/').next().unwrap_or(token),
        "echo"
            | "false"
            | "printf"
            | "test"
            | "true"
            | "url"
            // DC-70, repaired per architect review (B1): only commands that cannot themselves
            // execute another program belong here. `tar` (--to-command/-I), `rustc` (proc
            // macros/build scripts), and `gh` (a general-purpose API client, and the exact
            // command that publishes a release) all can, so they are NOT inert — see the
            // exact-match entries in procedure.rs instead.
            | "cd"
            | "mkdir"
            | "cp"
            // RFC 148: CI writes its fixture signing seeds to `0600` files, because prikk no
            // longer reads a seed from the environment and refuses a group-readable seed file.
            // `chmod` belongs in this list by the same test as every entry above it -- it changes
            // a mode and cannot execute another program under any arguments.
            | "chmod"
            | "sha256sum"
            // RFC 107 Stage 2: macOS's checksum tool -- `sha256sum` is not confirmed present on
            // the runner image, `shasum` is base-OS regardless
            // (`RFC-107-stage-2-report-ruling-v1.md` §2). Same category as `sha256sum` above: a
            // hashing utility with no capability to invoke another program under any arguments.
            | "shasum"
            // DC-71: prikk itself. Verified, not assumed — `grep -rn "std::process::Command"
            // crates/` returns no matches anywhere in the workspace, so the binary this CI job
            // exercises against a read-only fixture has no capability to execute another program
            // (the README's own text names "audit plugins" — the one feature that would add
            // this — as not yet implemented).
            | "prikk"
    )
}

pub(super) fn dynamic(token: &str) -> bool {
    token.contains('$')
}

pub(super) fn dynamic_cargo(token: &str) -> bool {
    dynamic(token) && token.to_ascii_lowercase().contains("cargo")
}

pub(super) fn opaque_execution(tokens: &[String], index: usize, head: &str) -> bool {
    let executable = head.rsplit('/').next().unwrap_or(head);
    let tail = tokens.get(index + 1..).unwrap_or_default();
    executable == "eval" && !tail.is_empty()
        || matches!(executable, "sh" | "bash" | "dash" | "ksh" | "zsh")
            && tail.iter().any(|token| {
                token
                    .strip_prefix('-')
                    .is_some_and(|options| !options.starts_with('-') && options.contains('c'))
            })
}

fn env_tail(tokens: &[String], mut index: usize) -> Result<usize, &'static str> {
    while let Some(token) = tokens.get(index) {
        match token.as_str() {
            "--" => return Ok(index + 1),
            "-i" | "--ignore-environment" | "-0" | "--null" | "-v" | "--debug" => index += 1,
            "-u" | "--unset" | "-C" | "--chdir" => {
                if tokens.get(index + 1).is_none() {
                    return Err("incomplete-env-option");
                }
                index += 2;
            }
            _ if attached_env_option(token) || assignment(token) => index += 1,
            _ if token.starts_with('-') => return Err("unsupported-env-option"),
            _ => return Ok(index),
        }
    }
    Err("missing-env-command")
}

fn attached_env_option(token: &str) -> bool {
    ["-u", "-C"]
        .iter()
        .any(|prefix| token.starts_with(prefix) && token.len() > prefix.len())
        || ["--unset=", "--chdir="]
            .iter()
            .any(|prefix| token.starts_with(prefix) && token.len() > prefix.len())
}

fn command_tail(tokens: &[String], index: usize) -> Result<usize, &'static str> {
    match tokens.get(index).map(String::as_str) {
        Some("--" | "-p") => Ok(index + 1),
        Some(token) if token.starts_with('-') => Err("unsupported-command-option"),
        Some(_) => Ok(index),
        None => Err("missing-command-wrapper-target"),
    }
}

fn assignment(token: &str) -> bool {
    token.split_once('=').is_some_and(|(name, _)| {
        let mut characters = name.chars();
        characters
            .next()
            .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
            && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
    })
}

use std::path::PathBuf;

use crate::boundary;
use crate::error::{Error, Result};
use crate::installer;
use crate::oracle;
use crate::policy;
use crate::reference;
use crate::release_evidence;
use crate::release_notes;
use crate::size;

pub(crate) fn run(arguments: Vec<String>) -> Result<()> {
    let (command, rest) = arguments.split_first().ok_or_else(|| Error::new(usage()))?;
    let root = repository_root()?;
    match command.as_str() {
        "check" if rest.is_empty() => policy::run_check(&root),
        "oracle-check" => oracle_check(&root, rest),
        "boundary-check" => boundary_check(&root, rest),
        "reference-check" => reference_check(&root, rest),
        "size-check" => size_check(&root, rest),
        "produce-release-evidence" => produce_release_evidence_command(&root, rest),
        "release-notes" => release_notes_command(&root, rest),
        "generate-installer" => generate_installer_command(rest),
        "-h" | "--help" | "help" if rest.is_empty() => {
            println!("{}", usage());
            Ok(())
        }
        _ => Err(Error::new(usage())),
    }
}

fn oracle_check(root: &std::path::Path, arguments: &[String]) -> Result<()> {
    let self_test = parse_json_mode(arguments, true)?;
    let report = oracle::verify_repository(root, self_test)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if report.valid {
        Ok(())
    } else {
        Err(Error::new("oracle verification failed"))
    }
}

fn boundary_check(root: &std::path::Path, arguments: &[String]) -> Result<()> {
    // RFC 131 §6e step 0: `--graph` emits the coupling graph instead of the verdict. It checks
    // nothing and can fail nothing -- a separate flag rather than extra fields on the verdict, so
    // the gate's own output stays exactly what every caller already parses.
    if arguments.iter().any(|argument| argument == "--graph") {
        let rest: Vec<String> = arguments
            .iter()
            .filter(|argument| *argument != "--graph")
            .cloned()
            .collect();
        parse_json_mode(&rest, false)?;
        let report = boundary::graph(root).map_err(Error::new)?;
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    parse_json_mode(arguments, false)?;
    let report = boundary::run(root)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if report.valid {
        Ok(())
    } else {
        Err(Error::new("workspace boundary verification failed"))
    }
}

/// RFC 130 §8: a production file over the line must be a recorded decision.
fn size_check(root: &std::path::Path, arguments: &[String]) -> Result<()> {
    parse_json_mode(arguments, false)?;
    let report = size::run(root)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if report.valid {
        Ok(())
    } else {
        Err(Error::new("release-policy file size verification failed"))
    }
}

fn reference_check(root: &std::path::Path, arguments: &[String]) -> Result<()> {
    parse_json_mode(arguments, false)?;
    let report = reference::run(root)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if report.valid {
        Ok(())
    } else {
        Err(Error::new("release-policy reference verification failed"))
    }
}

fn produce_release_evidence_command(root: &std::path::Path, arguments: &[String]) -> Result<()> {
    let mut observations_path = None;
    let mut prior_path = None;
    let mut expect_prior_sha256 = None;
    let mut out_path = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments.get(index).map(String::as_str) {
            Some("--observations") => {
                observations_path = Some(next_value(arguments, &mut index, "--observations")?);
            }
            Some("--prior") => {
                prior_path = Some(next_value(arguments, &mut index, "--prior")?);
            }
            Some("--expect-prior-sha256") => {
                expect_prior_sha256 =
                    Some(next_value(arguments, &mut index, "--expect-prior-sha256")?);
            }
            Some("--out") => {
                out_path = Some(next_value(arguments, &mut index, "--out")?);
            }
            _ => return Err(Error::new(usage())),
        }
    }
    let observations_path = observations_path.ok_or_else(|| Error::new(usage()))?;
    let prior = match (prior_path, expect_prior_sha256) {
        (Some(path), Some(expected_sha256)) => Some(release_evidence::PriorLink {
            path: PathBuf::from(path),
            expected_sha256,
        }),
        (None, None) => None,
        _ => {
            return Err(Error::new(
                "produce-release-evidence: --prior and --expect-prior-sha256 must be given together",
            ));
        }
    };
    let observations =
        release_evidence::load_observations(std::path::Path::new(&observations_path))?;
    let document = release_evidence::produce(root, observations, prior.as_ref())?;
    let rendered = serde_json::to_string_pretty(&document)?;
    match out_path {
        Some(path) => std::fs::write(path, rendered)?,
        None => println!("{rendered}"),
    }
    Ok(())
}

fn next_value(arguments: &[String], index: &mut usize, flag: &str) -> Result<String> {
    let value = arguments
        .get(*index + 1)
        .ok_or_else(|| Error::new(format!("{flag} requires a value")))?
        .clone();
    *index += 2;
    Ok(value)
}

fn release_notes_command(root: &std::path::Path, arguments: &[String]) -> Result<()> {
    let [tag, dist_dir] = arguments else {
        return Err(Error::new(usage()));
    };
    let notes = release_notes::assemble(root, tag, std::path::Path::new(dist_dir))?;
    print!("{notes}");
    Ok(())
}

fn generate_installer_command(arguments: &[String]) -> Result<()> {
    let [dist_dir] = arguments else {
        return Err(Error::new(usage()));
    };
    installer::generate(std::path::Path::new(dist_dir))
}

fn parse_json_mode(arguments: &[String], self_test_allowed: bool) -> Result<bool> {
    let mut self_test = false;
    let mut index = 0;
    while index < arguments.len() {
        match arguments.get(index).map(String::as_str) {
            Some("--format") if arguments.get(index + 1).map(String::as_str) == Some("json") => {
                index += 2;
            }
            Some("--self-test") if self_test_allowed => {
                self_test = true;
                index += 1;
            }
            _ => return Err(Error::new(usage())),
        }
    }
    Ok(self_test)
}

fn repository_root() -> Result<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .ok_or_else(|| Error::new("tool manifest is not below the repository root"))?
        .to_path_buf();
    Ok(root)
}

fn usage() -> &'static str {
    "usage: prikk-release-policy <check|oracle-check|boundary-check|reference-check|size-check> [--format json] [--self-test]\n       prikk-release-policy boundary-check --graph\n       prikk-release-policy produce-release-evidence --observations <path> [--prior <path> --expect-prior-sha256 <hex>] [--out <path>]\n       prikk-release-policy release-notes <tag> <dist-dir>\n       prikk-release-policy generate-installer <dist-dir>"
}

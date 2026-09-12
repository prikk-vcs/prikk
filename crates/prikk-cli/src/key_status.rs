//! `prikk key status` — can I sign here, and with which key? (RFC 150)
//!
//! **Reads, never signs, never writes, and never prints a seed.** It answers the three questions a
//! front-end needs before offering to commit: is key material present and usable, which key id is in
//! effect, and does that id already bind to different material in this repository. Public material
//! only — the boundary stikk's `C-I1e` protects.
//!
//! **Exit 0 in every not-ready state.** Absence degrades, it does not fail (RFC 140 §7b, RFC 142
//! §6b): a caller asking "can I sign?" and getting "no, because the seed is missing" has had its
//! question answered, and a non-zero exit would make that indistinguishable from a broken
//! repository. `1` is for a real failure — an unreadable repository, an I/O error; `2` for usage.

use std::path::PathBuf;

use prikk_store::{AuthorKeyBinding, RepositoryLayout};

use crate::arg_scan::{SetOnce, flag_value, mark_seen, unknown_argument};
use crate::commands::CliError;
use crate::key_material::{KeyStatus, Role};
use crate::stdout::println;

/// How a role's key id relates to what this repository has already recorded. RFC 150 §2's
/// `binding`, computed only when there is a repository to ask.
enum Binding {
    Author(AuthorKeyBinding),
    MaintainerNotAdopted,
    MaintainerMatches,
    MaintainerMismatch,
}

impl Binding {
    fn as_str(&self) -> &'static str {
        match self {
            Binding::Author(AuthorKeyBinding::Unrecorded) => "unrecorded",
            Binding::Author(AuthorKeyBinding::Matches) | Binding::MaintainerMatches => "matches",
            Binding::Author(AuthorKeyBinding::Mismatch) | Binding::MaintainerMismatch => "mismatch",
            Binding::MaintainerNotAdopted => "not-adopted",
        }
    }
}

pub fn run_key_status(args: Vec<String>) -> std::result::Result<(), CliError> {
    let parsed = parse_args(args)?;
    // A repository is optional: `key status` answers about *key material*, which exists whether or
    // not you are standing in a repository. Only `binding` needs one, and it degrades to absent.
    let layout = RepositoryLayout::new(&parsed.root)
        .ok()
        .filter(|layout| layout.prikk_dir().exists());

    let roles: Vec<Role> = match parsed.role {
        Some(role) => vec![role],
        None => vec![Role::Author, Role::Maintainer],
    };

    let mut statuses = Vec::new();
    for role in roles {
        let status = crate::key_material::status(role)?;
        let binding = layout
            .as_ref()
            .map(|layout| binding_for(layout, &status))
            .transpose()?
            .flatten();
        statuses.push((status, binding));
    }

    if parsed.format_json {
        print_json(&statuses);
    } else {
        print_prose(&statuses);
    }
    Ok(())
}

/// The binding, using the same lookups the signing paths use: `author_key_binding` is the query
/// `check_author_key_conflict` itself is expressed in terms of, and the maintainer side reads the
/// same adopted-key policy `seal`'s own trust gate reads.
///
/// `None` when there is no public key to compare — an unusable seed has none, and "what does this
/// unreadable key bind to" is not a question with an answer.
fn binding_for(
    layout: &RepositoryLayout,
    status: &KeyStatus,
) -> std::result::Result<Option<Binding>, CliError> {
    let Ok(seed) = status.seed.as_ref() else {
        return Ok(None);
    };
    let public_key = prikk_crypto::Ed25519KeyPair::from_seed(seed).public_key_bytes();
    match status.role {
        Role::Author => {
            let binding = prikk_store::author_key_binding(layout, &status.key_id, public_key)
                .map_err(|err| CliError::Failure(err.to_string()))?;
            Ok(Some(Binding::Author(binding)))
        }
        Role::Maintainer => {
            let policy = prikk_store::load_maintainer_trust_policy_or_empty(layout)
                .map_err(|err| CliError::Failure(err.to_string()))?;
            let adopted: Vec<_> = policy
                .keys
                .iter()
                .filter(|key| key.key_id == status.key_id)
                .collect();
            Ok(Some(if adopted.is_empty() {
                Binding::MaintainerNotAdopted
            } else if adopted.iter().any(|key| key.public_key == public_key) {
                Binding::MaintainerMatches
            } else {
                Binding::MaintainerMismatch
            }))
        }
    }
}

fn print_prose(statuses: &[(KeyStatus, Option<Binding>)]) {
    for (index, (status, binding)) in statuses.iter().enumerate() {
        if index > 0 {
            println!();
        }
        println!("role: {}", status.role.label());
        println!("source: {}", status.source.as_str());
        println!("path: {}", status.path.display());
        println!("usable: {}", status.usable());
        if let Err(reason) = &status.seed {
            println!("reason: {}", reason.code());
        }
        println!(
            "key id: {} ({})",
            status.key_id,
            if status.key_id_from_environment {
                "environment"
            } else {
                "default"
            }
        );
        if let Some(public_key) = status.public_key_hex() {
            println!("public key: {public_key}");
        }
        match binding {
            Some(binding) => println!("binding: {}", binding.as_str()),
            None => println!("binding: <not computed>"),
        }
    }
}

fn print_json(statuses: &[(KeyStatus, Option<Binding>)]) {
    use crate::output::verification::escape_json_string;

    let mut json = String::new();
    json.push_str("{\n");
    json.push_str("  \"schema_version\": \"key-status-v1\",\n");
    json.push_str("  \"roles\": [");
    for (index, (status, binding)) in statuses.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str("\n    {\n");
        json.push_str(&format!(
            "      \"role\": {},\n",
            escape_json_string(status.role.label())
        ));
        json.push_str(&format!(
            "      \"source\": {},\n",
            escape_json_string(status.source.as_str())
        ));
        json.push_str(&format!(
            "      \"path\": {},\n",
            escape_json_string(&status.path.display().to_string())
        ));
        json.push_str(&format!("      \"usable\": {},\n", status.usable()));
        match &status.seed {
            Err(reason) => json.push_str(&format!(
                "      \"reason\": {},\n",
                escape_json_string(&reason.code())
            )),
            Ok(_) => json.push_str("      \"reason\": null,\n"),
        }
        json.push_str(&format!(
            "      \"key_id\": {},\n",
            escape_json_string(&status.key_id)
        ));
        json.push_str(&format!(
            "      \"key_id_source\": {},\n",
            escape_json_string(if status.key_id_from_environment {
                "environment"
            } else {
                "default"
            })
        ));
        match status.public_key_hex() {
            Some(hex) => json.push_str(&format!(
                "      \"public_key\": {},\n",
                escape_json_string(&hex)
            )),
            None => json.push_str("      \"public_key\": null,\n"),
        }
        match binding {
            Some(binding) => json.push_str(&format!(
                "      \"binding\": {}\n",
                escape_json_string(binding.as_str())
            )),
            None => json.push_str("      \"binding\": null\n"),
        }
        json.push_str("    }");
    }
    if !statuses.is_empty() {
        json.push_str("\n  ");
    }
    json.push_str("]\n}");
    println!("{json}");
}

struct Args {
    root: PathBuf,
    role: Option<Role>,
    format_json: bool,
}

fn parse_args(args: Vec<String>) -> std::result::Result<Args, CliError> {
    let mut role = None;
    let mut format_json = false;
    let mut path: Option<String> = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--role" => {
                let value = flag_value(&mut iter, "key status --role")?;
                let parsed = match value.as_str() {
                    "author" => Role::Author,
                    "maintainer" => Role::Maintainer,
                    other => {
                        return Err(CliError::Usage(format!(
                            "key status --role does not support {other:?} (expected author or \
                             maintainer)"
                        )));
                    }
                };
                role.set_once("--role", parsed)?;
            }
            "--format" => {
                let value = flag_value(&mut iter, "key status --format")?;
                if value != "json" {
                    return Err(CliError::Usage(format!(
                        "key status --format does not support {value:?}"
                    )));
                }
                mark_seen(&mut format_json, "--format")?;
            }
            other if other.starts_with('-') => {
                return Err(unknown_argument("key status", other));
            }
            _ => {
                if path.is_some() {
                    return Err(CliError::Usage(
                        "key status accepts at most one path".to_string(),
                    ));
                }
                path = Some(arg);
            }
        }
    }
    Ok(Args {
        root: crate::args::optional_path_or_current(path)?,
        role,
        format_json,
    })
}

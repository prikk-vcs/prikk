//! `prikk setup` — one command over a first-class sequence (RFC 135 §3/§9.8.2).
//!
//! `key generate`, `trust maintainer add`, and `init` stay first-class and documented — that is
//! the sequence a reader follows to *understand* what this command does. `setup` composes them
//! for someone who wants a working repository now, without weakening any of the five binding
//! properties the individual commands already establish:
//!
//! 1. One command reaches a working repository.
//! 2. prikk invents no location for a secret -- the user names every output path.
//! 3. No secret reaches scrollback when the user provides an output path for it.
//! 4. The trust decision is shown -- registering a maintainer key is a trust act, and this
//!    composition may remove the *typing*, never the *seeing*.
//! 5. Seeds never on argv. Paths are fine.

use std::path::{Path, PathBuf};

use prikk_store::{
    RepositoryLayout, add_trusted_maintainer, load_maintainer_trust_policy_or_empty,
};

use crate::arg_scan::{SetOnce, flag_value, unknown_argument};
use crate::commands::CliError;
use crate::key::write_seed_to_path;
use crate::stdout::println;
use prikk_crypto::Ed25519KeyPair;

const AUTHOR_KEY_ID: &str = "author";
const MAINTAINER_KEY_ID: &str = "maintainer";

/// One generated seed's disposition. RFC 148 removed the third case that used to exist — printing
/// the hex — because there is no longer an environment variable to paste it into. A seed is written
/// to a file, always; the only question is which file.
enum SeedOutput {
    /// The default key directory's own file. Nothing to export: every new shell finds it.
    DefaultDirectory(PathBuf),
    /// A path the user named with `--author-seed-out`/`--maintainer-seed-out`. The corresponding
    /// `PRIKK_*_SEED_FILE` line is printed, because prikk will not look here on its own.
    UserNamed(PathBuf),
}

impl SeedOutput {
    fn path(&self) -> &Path {
        match self {
            SeedOutput::DefaultDirectory(path) | SeedOutput::UserNamed(path) => path,
        }
    }
}

pub fn run_setup(args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut path = None;
    let mut author_seed_out = None;
    let mut maintainer_seed_out = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--author-seed-out" => {
                let value = flag_value(&mut iter, "setup --author-seed-out")?;
                author_seed_out.set_once("--author-seed-out", PathBuf::from(value))?;
            }
            "--maintainer-seed-out" => {
                let value = flag_value(&mut iter, "setup --maintainer-seed-out")?;
                maintainer_seed_out.set_once("--maintainer-seed-out", PathBuf::from(value))?;
            }
            other if other.starts_with('-') => return Err(unknown_argument("setup", other)),
            _ => {
                if path.is_some() {
                    return Err(CliError::Usage(
                        "setup accepts at most one repository path".to_string(),
                    ));
                }
                path = Some(arg);
            }
        }
    }
    let root = match path {
        Some(path) => PathBuf::from(path),
        None => crate::args::current_dir()?,
    };

    // RFC 135 §(c) -- "a flow, not storage" -- never said what a *re-run* does, and the answer was
    // the worst available one: `RepositoryLayout::init` is idempotent, so it succeeded, "initialized
    // Prikk repository at ..." printed, two fresh keys were minted, any `--*-seed-out` file was
    // written, and only then did the trust step collide with the key already adopted here. A command
    // that prints "initialized" and then fails has started something it should not have. Worse, the
    // seeds it left on disk belong to keys **no repository adopted** -- a user has every reason to
    // think those are their keys.
    //
    // So the check is *before* `create_dir_all`, ahead of every write and every line of output: if
    // this directory already holds a repository, nothing at all runs. A caller-fixable state, and
    // the message names both ways out rather than only reporting the collision.
    let existing = root.join(".prikk");
    if existing.exists() {
        return Err(CliError::Failure(
            prikk_error::PrikkError::Precondition(format!(
                "{} already holds a repository; to use your existing keys here run `prikk trust \
                 maintainer add` (see `prikk key public`), or pick a different directory for a new \
                 project",
                root.display()
            ))
            .to_string(),
        ));
    }

    // RFC 148 §2c: the **second** check, and it must also come before anything is created. With
    // keys already in the key directory, the previous version printed "initialized Prikk repository
    // at ..." and then failed on `refusing to overwrite an existing file: .../author.seed`, leaving
    // a repository with no maintainer adopted -- the same half-run shape the check above exists to
    // prevent, reintroduced one release later by a failure mode that check could not see.
    //
    // Deciding here, before `create_dir_all`, is what makes the refusal arm honest: a missing
    // counterpart seed stops the command with nothing written and nothing printed.
    let existing_keys = classify_existing_keys(&author_seed_out, &maintainer_seed_out)?;

    // Property 1: one command reaches a working repository, without the user running anything
    // else first -- `RepositoryLayout::init` itself does not create a missing leading directory
    // (the same is true of plain `prikk init`), so `setup` must, or naming a path that does not
    // yet exist would silently reintroduce a step this command exists to remove.
    std::fs::create_dir_all(&root)
        .map_err(|err| format!("failed to create {}: {err}", root.display()))?;

    let layout = RepositoryLayout::init(root.clone()).map_err(|err| err.to_string())?;
    println!(
        "initialized Prikk repository at {}",
        root.join(".prikk").display()
    );

    // RFC 148: with no flags, both seeds land in the default key directory and nothing is printed.
    // The directory is created before either key is generated, so a failure to create it does not
    // leave one seed written and one lost.
    let key_dir = match (&author_seed_out, &maintainer_seed_out) {
        (Some(_), Some(_)) => None,
        _ => Some(crate::key_material::ensure_key_dir()?),
    };

    let author_output = match existing_keys.author_is_reused {
        // RFC 148 §2c: a second project reuses the keys you already have. Nothing is generated and
        // nothing is written -- `setup` here is `init` plus the trust act, which is the only step a
        // new repository actually needs when the keys already exist.
        true => SeedOutput::DefaultDirectory(crate::key_material::seed_path(
            crate::key_material::Role::Author,
        )?),
        false => {
            let seed = Ed25519KeyPair::generate_seed().map_err(|err| err.to_string())?;
            write_role_seed(
                &seed,
                author_seed_out,
                key_dir.as_deref(),
                crate::key_material::Role::Author,
            )?
        }
    };

    let (maintainer_output, maintainer_public_key_hex) =
        match &existing_keys.maintainer_public_key_hex {
            // Already derived, and already mode-checked, during classification above.
            Some(hex) => (
                SeedOutput::DefaultDirectory(crate::key_material::seed_path(
                    crate::key_material::Role::Maintainer,
                )?),
                hex.clone(),
            ),
            None => {
                let seed = Ed25519KeyPair::generate_seed().map_err(|err| err.to_string())?;
                let public_key = Ed25519KeyPair::from_seed(&seed).public_key_bytes();
                let hex = prikk_hash::to_hex(&public_key);
                let output = write_role_seed(
                    &seed,
                    maintainer_seed_out,
                    key_dir.as_deref(),
                    crate::key_material::Role::Maintainer,
                )?;
                (output, hex)
            }
        };

    // Property 4: the trust decision is shown, not performed invisibly -- this is the one step in
    // the composed sequence that is a trust act, and `setup` must print it exactly as `trust
    // maintainer add` itself would, not fold it silently into "repository ready."
    let (adopted, _newly_added) =
        add_trusted_maintainer(&layout, MAINTAINER_KEY_ID, &maintainer_public_key_hex)
            .map_err(|err| err.to_string())?;
    println!("trusted maintainer key: {}", adopted.key_id);
    // RFC 138 §4.2 carried-defects B: a derived count, not the `policy: required=1` literal that
    // used to print here -- see `main.rs`'s identical fix at the `trust maintainer add` site for
    // why.
    let policy = load_maintainer_trust_policy_or_empty(&layout).map_err(|err| err.to_string())?;
    println!("adopted maintainer keys: {}", policy.keys.len());

    println!();
    match (&author_output, &maintainer_output) {
        (SeedOutput::DefaultDirectory(author), SeedOutput::DefaultDirectory(_)) => {
            // The whole point of RFC 148: no export block, because there is nothing to export.
            // "using" rather than "your keys are in" when nothing was minted -- a user who just ran
            // this in a second project should be able to tell, from one word, that no new key was
            // created and their existing one is now trusted here too.
            let dir = author.parent().unwrap_or(author);
            let verb = if existing_keys.any_reused() {
                "using your keys in"
            } else {
                "your keys are in"
            };
            println!("{verb} {}", dir.display());
            println!("every new shell finds them -- nothing to export");
        }
        _ => {
            println!("set these before committing:");
            print_seed_file_line(
                crate::key_material::Role::Author,
                &author_output,
                AUTHOR_KEY_ID,
            );
            print_seed_file_line(
                crate::key_material::Role::Maintainer,
                &maintainer_output,
                MAINTAINER_KEY_ID,
            );
        }
    }
    println!();
    println!("next steps:");
    println!("  prikk commit -m \"<message>\"");
    println!(
        "  prikk seal --allow-no-audit  # no audit trust policy is configured yet; see \
         `prikk seal --help`"
    );
    Ok(())
}

/// Which roles already have a seed in the key directory, and therefore reuse it (RFC 148 §2c).
struct ExistingKeys {
    author_is_reused: bool,
    maintainer_is_reused: bool,
    /// The reused maintainer key's public half, derived **during classification** — that is, before
    /// `init` — so that a seed failing the mode rule refuses with nothing created. Deriving it later
    /// would print "initialized Prikk repository at ..." and *then* refuse, which is the exact
    /// half-run this whole check exists to prevent.
    maintainer_public_key_hex: Option<String>,
}

impl ExistingKeys {
    fn any_reused(&self) -> bool {
        self.author_is_reused || self.maintainer_is_reused
    }
}

/// Decide, **before anything is created**, what each role does: reuse the key directory's seed, or
/// mint a new one.
///
/// A role given `--*-seed-out` always mints to that path, as it always has — the user named a
/// destination, so there is nothing to reuse and nothing to refuse.
///
/// A role left to the default follows the key directory: present means reuse, absent means mint.
/// **Exactly one of the two present is refused outright**, and that is the case worth explaining:
/// minting the missing one would silently pair a brand-new key with an existing one the user may
/// have adopted elsewhere, and reusing the present one alone would leave the other role unusable.
/// Neither is a guess `setup` should make, so it names the missing file and the one command that
/// creates it.
fn classify_existing_keys(
    author_seed_out: &Option<PathBuf>,
    maintainer_seed_out: &Option<PathBuf>,
) -> std::result::Result<ExistingKeys, CliError> {
    let author_defaulted = author_seed_out.is_none();
    let maintainer_defaulted = maintainer_seed_out.is_none();
    if !author_defaulted && !maintainer_defaulted {
        return Ok(ExistingKeys {
            author_is_reused: false,
            maintainer_is_reused: false,
            maintainer_public_key_hex: None,
        });
    }

    let author_path = crate::key_material::seed_path(crate::key_material::Role::Author)?;
    let maintainer_path = crate::key_material::seed_path(crate::key_material::Role::Maintainer)?;
    let author_present = author_defaulted && author_path.exists();
    let maintainer_present = maintainer_defaulted && maintainer_path.exists();

    // Only a run where *both* roles default can be half-present; if one role was given a path, the
    // other's presence is simply reuse-or-mint on its own.
    if author_defaulted && maintainer_defaulted && author_present != maintainer_present {
        let missing = if author_present {
            &maintainer_path
        } else {
            &author_path
        };
        return Err(CliError::Failure(
            prikk_error::PrikkError::Precondition(format!(
                "your key directory has one seed but not the other: {} is missing. Create it with \
                 `prikk key generate --out {}`, then run `prikk setup` again",
                missing.display(),
                missing.display()
            ))
            .to_string(),
        ));
    }

    // Read every seed that will be reused, here and now. `key_material::read_seed` is what applies
    // the mode rule and the retired-variable refusal, so reading through it is what makes a reused
    // key clear the same checks a used key clears -- and doing it *before* the caller creates
    // anything is what keeps a refusal from arriving after "initialized".
    if author_present {
        let _ = crate::key_material::read_seed(crate::key_material::Role::Author)?;
    }
    let maintainer_public_key_hex = match maintainer_present {
        true => {
            let seed = crate::key_material::read_seed(crate::key_material::Role::Maintainer)?;
            let public_key = Ed25519KeyPair::from_seed(&seed).public_key_bytes();
            Some(prikk_hash::to_hex(&public_key))
        }
        false => None,
    };

    Ok(ExistingKeys {
        author_is_reused: author_present,
        maintainer_is_reused: maintainer_present,
        maintainer_public_key_hex,
    })
}

/// Write one role's seed to the path the user named, or to the default key directory.
fn write_role_seed(
    seed: &[u8; prikk_crypto::ED25519_KEY_LEN],
    user_named: Option<PathBuf>,
    key_dir: Option<&Path>,
    role: crate::key_material::Role,
) -> std::result::Result<SeedOutput, CliError> {
    match user_named {
        Some(path) => {
            write_seed_to_path(seed, &path)?;
            Ok(SeedOutput::UserNamed(path))
        }
        None => {
            let dir = key_dir.ok_or_else(|| {
                CliError::Failure("internal: no key directory for a default seed".to_string())
            })?;
            let path = dir.join(role.seed_file_name());
            crate::key::write_seed_to_key_dir(seed, &path)?;
            Ok(SeedOutput::DefaultDirectory(path))
        }
    }
}

/// Print the `PRIKK_*_SEED_FILE` line for a seed prikk will not find on its own.
fn print_seed_file_line(role: crate::key_material::Role, output: &SeedOutput, key_id: &str) {
    println!("  export {}=\"{key_id}\"", role.key_id_var());
    match output {
        // A default-directory seed needs no variable at all; saying so beside a sibling that does
        // is clearer than silence.
        SeedOutput::DefaultDirectory(path) => {
            println!(
                "  # {} is found automatically at {}",
                role.label(),
                path.display()
            );
        }
        SeedOutput::UserNamed(path) => {
            println!("  export {}=\"{}\"", role.seed_file_var(), path.display());
        }
    }
    let _ = output.path();
}

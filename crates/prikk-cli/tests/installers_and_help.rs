//! Installers and help, grouped (0.55.0 G2 Part B). Each module below was
//! its own top-level test binary before this round; the file is unchanged, only its location
//! and this grouping file's `mod` line are new. See the module's own file under
//! `tests/installers_and_help/` for its own doc comment.

#[path = "installers_and_help/beginners_tutorial.rs"]
mod beginners_tutorial;
#[path = "installers_and_help/dc44_backup_restore_page.rs"]
mod dc44_backup_restore_page;
#[path = "installers_and_help/rfc116_stage7_cross_platform_artifact.rs"]
mod rfc116_stage7_cross_platform_artifact;
#[path = "installers_and_help/rfc121_command_help.rs"]
mod rfc121_command_help;
#[path = "installers_and_help/rfc146_help_inventory_matches_commands_md.rs"]
mod rfc146_help_inventory_matches_commands_md;
#[path = "installers_and_help/rfc146_help_synopsis_advertises_format_json.rs"]
mod rfc146_help_synopsis_advertises_format_json;

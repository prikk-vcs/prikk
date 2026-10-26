//! Archive and bundle, grouped (0.55.0 G2 Part B). Each module below was
//! its own top-level test binary before this round; the file is unchanged, only its location
//! and this grouping file's `mod` line are new. See the module's own file under
//! `tests/archive_and_bundle/` for its own doc comment.

#[path = "archive_and_bundle/bundle_import_fifo_input.rs"]
mod bundle_import_fifo_input;
#[path = "archive_and_bundle/bundle_import_real_kill.rs"]
mod bundle_import_real_kill;
#[path = "archive_and_bundle/bundle_import_refuses_before_writing.rs"]
mod bundle_import_refuses_before_writing;
#[path = "archive_and_bundle/dc44_bundle_export_durability.rs"]
mod dc44_bundle_export_durability;
#[path = "archive_and_bundle/dc44_bundle_manifest.rs"]
mod dc44_bundle_manifest;
#[path = "archive_and_bundle/dc44_bundle_offline_verify.rs"]
mod dc44_bundle_offline_verify;
#[path = "archive_and_bundle/dc44_sync_output_durability.rs"]
mod dc44_sync_output_durability;
#[path = "archive_and_bundle/dc78_bundle_exchange.rs"]
mod dc78_bundle_exchange;
#[path = "archive_and_bundle/dc78_export_after_deleting_an_edited_file.rs"]
mod dc78_export_after_deleting_an_edited_file;
#[path = "archive_and_bundle/rfc144_bundle_preview.rs"]
mod rfc144_bundle_preview;
#[path = "archive_and_bundle/rfc155_archive_export_memory.rs"]
mod rfc155_archive_export_memory;
#[path = "archive_and_bundle/rfc155_part_ib_carry_forward_smoke.rs"]
mod rfc155_part_ib_carry_forward_smoke;
#[path = "archive_and_bundle/rfc155_part_ie_overrun_cli.rs"]
mod rfc155_part_ie_overrun_cli;
#[path = "archive_and_bundle/rfc155_part_ie_trust_note.rs"]
mod rfc155_part_ie_trust_note;
#[path = "archive_and_bundle/rfc155_part_if_trust_note.rs"]
mod rfc155_part_if_trust_note;
#[path = "archive_and_bundle/step0_c1_closure_shapes.rs"]
mod step0_c1_closure_shapes;
#[path = "support/mod.rs"]
mod support;

//! Node-model application and path/rename safety, grouped (0.55.0 G2 Part B). Each module below
//! was its own top-level test binary before this round; the file is unchanged, only its location
//! and this grouping file's `mod` line are new. See `tests/node_model_and_rename/<name>.rs` for
//! each module's own doc comment.

#[path = "node_model_and_rename/dc64_baseline_cache.rs"]
mod dc64_baseline_cache;
#[path = "node_model_and_rename/dc72_path_safety_collisions.rs"]
mod dc72_path_safety_collisions;
#[path = "node_model_and_rename/dc73_node_model_apply.rs"]
mod dc73_node_model_apply;
#[path = "node_model_and_rename/rfc144_increment3_piece1_mv.rs"]
mod rfc144_increment3_piece1_mv;
#[path = "node_model_and_rename/rfc144_increment3_piece2_move_hint.rs"]
mod rfc144_increment3_piece2_move_hint;
#[path = "support/mod.rs"]
mod support;

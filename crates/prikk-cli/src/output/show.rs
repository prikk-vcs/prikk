//! Output for `prikk show` (RFC 142): what a block or patch changed.
//!
//! RFC 142 §6: renders `EditText`'s own before/after span, never a synthesized line-oriented
//! diff -- no hunk header, no line-number gutter, in either the prose or the JSON form.

use prikk_store::{ShowBlobContent, ShowDeletePreimage, ShowOperationContent, ShowPathResolution};
use prikk_store::{ShowOperation, ShowPatch};

use crate::stdout::println;

use super::verification::escape_json_string;

fn print_path(label: &str, path: &ShowPathResolution) {
    match path {
        ShowPathResolution::Path(path) => println!("    {label}: {path}"),
        ShowPathResolution::Unresolved { node_id } => {
            println!("    {label}: <unresolved node {node_id}>");
        }
    }
}

fn print_text(label: &str, bytes: &[u8]) {
    println!("    {label}:");
    println!("{}", String::from_utf8_lossy(bytes));
}

fn print_blob_content(label: &str, content: &ShowBlobContent) {
    match content {
        ShowBlobContent::Text(bytes) => print_text(label, bytes),
        ShowBlobContent::Binary { blob_id, size } => {
            println!("    {label}: <binary blob {blob_id}, {size} bytes>");
        }
        ShowBlobContent::Unavailable { blob_id } => {
            println!("    {label}: <unavailable blob {blob_id}>");
        }
    }
}

fn print_operation(index: usize, operation: &ShowOperation) {
    println!("  operation {}: {}", index + 1, operation.kind);
    match &operation.content {
        ShowOperationContent::CreateFile { content, mode } => {
            let [path] = operation.paths.as_slice() else {
                println!("    (expected exactly one path)");
                return;
            };
            print_path("path", path);
            println!("    mode: {:o}", mode & 0o7777);
            print_blob_content("content", content);
        }
        ShowOperationContent::DeleteNode { preimage } => {
            let [path] = operation.paths.as_slice() else {
                println!("    (expected exactly one path)");
                return;
            };
            print_path("path", path);
            match preimage {
                ShowDeletePreimage::File(content) => print_blob_content("content", content),
                ShowDeletePreimage::Symlink { old_target } => {
                    println!("    target: {old_target}");
                }
            }
        }
        ShowOperationContent::EditText {
            old_span_text,
            replacement_text,
        } => {
            let [path] = operation.paths.as_slice() else {
                println!("    (expected exactly one path)");
                return;
            };
            print_path("path", path);
            print_text("old", old_span_text);
            print_text("new", replacement_text);
        }
        ShowOperationContent::ReplaceBinary { old, new } => {
            let [path] = operation.paths.as_slice() else {
                println!("    (expected exactly one path)");
                return;
            };
            print_path("path", path);
            print_blob_content("old", old);
            print_blob_content("new", new);
        }
        ShowOperationContent::RenamePath => {
            let [old_path, new_path] = operation.paths.as_slice() else {
                println!("    (expected exactly two paths)");
                return;
            };
            print_path("old path", old_path);
            print_path("new path", new_path);
        }
        ShowOperationContent::ChangePerm { old_mode, new_mode } => {
            let [path] = operation.paths.as_slice() else {
                println!("    (expected exactly one path)");
                return;
            };
            print_path("path", path);
            println!(
                "    mode: {:o} -> {:o}",
                old_mode & 0o7777,
                new_mode & 0o7777
            );
        }
        ShowOperationContent::CreateSymlink { target } => {
            let [path] = operation.paths.as_slice() else {
                println!("    (expected exactly one path)");
                return;
            };
            print_path("path", path);
            println!("    target: {target}");
        }
    }
}

/// Print `prikk show`'s prose form.
pub(crate) fn print_show(patches: &[ShowPatch]) {
    println!("patches: {}", patches.len());
    for patch in patches {
        println!();
        println!("patch {}", patch.patch_id);
        for (index, operation) in patch.operations.iter().enumerate() {
            print_operation(index, operation);
        }
    }
}

fn push_path(json: &mut String, path: &ShowPathResolution) {
    match path {
        ShowPathResolution::Path(path) => {
            json.push_str("{\"path\": ");
            json.push_str(&escape_json_string(path));
            json.push('}');
        }
        ShowPathResolution::Unresolved { node_id } => {
            json.push_str("{\"unresolved_node_id\": ");
            json.push_str(&escape_json_string(node_id));
            json.push('}');
        }
    }
}

fn push_blob_content(json: &mut String, content: &ShowBlobContent) {
    match content {
        ShowBlobContent::Text(bytes) => {
            json.push_str("{\"kind\": \"text\", \"text\": ");
            json.push_str(&escape_json_string(&String::from_utf8_lossy(bytes)));
            json.push('}');
        }
        ShowBlobContent::Binary { blob_id, size } => {
            json.push_str("{\"kind\": \"binary\", \"blob_id\": ");
            json.push_str(&escape_json_string(&blob_id.to_string()));
            json.push_str(&format!(", \"size\": {size}}}"));
        }
        ShowBlobContent::Unavailable { blob_id } => {
            json.push_str("{\"kind\": \"unavailable\", \"blob_id\": ");
            json.push_str(&escape_json_string(&blob_id.to_string()));
            json.push('}');
        }
    }
}

fn push_operation(json: &mut String, operation: &ShowOperation) {
    json.push_str("{\"kind\": ");
    json.push_str(&escape_json_string(operation.kind));
    json.push_str(", \"paths\": [");
    for (index, path) in operation.paths.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        push_path(json, path);
    }
    json.push_str("], \"content\": ");
    match &operation.content {
        ShowOperationContent::CreateFile { content, mode } => {
            json.push_str("{\"kind\": \"create-file\", \"content\": ");
            push_blob_content(json, content);
            json.push_str(&format!(", \"mode\": {mode}}}"));
        }
        ShowOperationContent::DeleteNode { preimage } => {
            json.push_str("{\"kind\": \"delete-node\", \"preimage\": ");
            match preimage {
                ShowDeletePreimage::File(content) => {
                    json.push_str("{\"kind\": \"file\", \"content\": ");
                    push_blob_content(json, content);
                    json.push('}');
                }
                ShowDeletePreimage::Symlink { old_target } => {
                    json.push_str("{\"kind\": \"symlink\", \"old_target\": ");
                    json.push_str(&escape_json_string(old_target));
                    json.push('}');
                }
            }
            json.push('}');
        }
        ShowOperationContent::EditText {
            old_span_text,
            replacement_text,
        } => {
            json.push_str("{\"kind\": \"edit-text\", \"old_span_text\": ");
            json.push_str(&escape_json_string(&String::from_utf8_lossy(old_span_text)));
            json.push_str(", \"replacement_text\": ");
            json.push_str(&escape_json_string(&String::from_utf8_lossy(
                replacement_text,
            )));
            json.push('}');
        }
        ShowOperationContent::ReplaceBinary { old, new } => {
            json.push_str("{\"kind\": \"replace-binary\", \"old\": ");
            push_blob_content(json, old);
            json.push_str(", \"new\": ");
            push_blob_content(json, new);
            json.push('}');
        }
        ShowOperationContent::RenamePath => {
            json.push_str("{\"kind\": \"rename-path\"}");
        }
        ShowOperationContent::ChangePerm { old_mode, new_mode } => {
            json.push_str(&format!(
                "{{\"kind\": \"change-perm\", \"old_mode\": {old_mode}, \"new_mode\": {new_mode}}}"
            ));
        }
        ShowOperationContent::CreateSymlink { target } => {
            json.push_str("{\"kind\": \"create-symlink\", \"target\": ");
            json.push_str(&escape_json_string(target));
            json.push('}');
        }
    }
    json.push('}');
}

/// Print `prikk show --format json`: `show-report-v1`, settling the format for `show` and nothing
/// else (RFC 142 §5, scoped the same way RFC 138 §7.2 scoped its own).
pub(crate) fn print_show_json(patches: &[ShowPatch]) {
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str("  \"schema_version\": \"show-report-v1\",\n");
    json.push_str("  \"patches\": [");
    for (index, patch) in patches.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str("\n    {\"patch_id\": ");
        json.push_str(&escape_json_string(&patch.patch_id.to_string()));
        json.push_str(", \"operations\": [");
        for (op_index, operation) in patch.operations.iter().enumerate() {
            if op_index > 0 {
                json.push(',');
            }
            json.push_str("\n      ");
            push_operation(&mut json, operation);
        }
        if !patch.operations.is_empty() {
            json.push_str("\n    ");
        }
        json.push_str("]}");
    }
    if !patches.is_empty() {
        json.push_str("\n  ");
    }
    json.push_str("]\n");
    json.push('}');
    println!("{json}");
}

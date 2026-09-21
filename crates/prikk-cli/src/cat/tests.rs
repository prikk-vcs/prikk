//! The terminal decision, controlled directly: RFC 157 §4's one security rule, without a pseudo-terminal.

use prikk_store::PointEntryEncoding;

use super::binary_to_terminal_refusal;

#[test]
fn binary_refuses_a_terminal_and_names_the_route() {
    let Some(refusal) = binary_to_terminal_refusal(PointEntryEncoding::Binary, true, "src/big.bin")
    else {
        panic!("binary content to a terminal must refuse");
    };
    assert!(refusal.contains("src/big.bin is binary"), "{refusal}");
    assert!(refusal.contains("prikk cat --output <file>"), "{refusal}");
    assert!(refusal.contains("nothing was written"), "{refusal}");
}

#[test]
fn binary_to_a_pipe_is_written() {
    assert!(binary_to_terminal_refusal(PointEntryEncoding::Binary, false, "src/big.bin").is_none());
}

#[test]
fn text_is_written_either_way() {
    assert!(binary_to_terminal_refusal(PointEntryEncoding::Text, true, "src/main.rs").is_none());
    assert!(binary_to_terminal_refusal(PointEntryEncoding::Text, false, "src/main.rs").is_none());
}

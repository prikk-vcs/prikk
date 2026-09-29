//! RFC 163 §2 — the shared write-side tail guard.
//!
//! **Before an append, a writer confirms under its lock that the file ends at its last sound record.
//! If it does not, it refuses before it writes anything.** One function, called at every scope-B
//! append site (the pointer index, trust keys, trust policy, author keys, the received index) after
//! whatever read that site already performs before appending -- never a second whole read of its own.
//! What counts as "the tail" is RFC 162 rule 3's: everything after the last sound record, when no
//! sound record follows, whatever its shape.

use prikk_error::{PrikkError, Result};

/// Refuse before an append if `trailing_partial_bytes` (from a replay the caller already performed) is
/// nonzero. `file_label` names the file in the refusal; `tail_offset` is the byte offset where the
/// last sound record ends, from the same replay; `way_out` names the repair or manual recovery step.
/// A no-op when the file already ends cleanly.
pub(crate) fn require_no_unclean_tail(
    file_label: &str,
    trailing_partial_bytes: usize,
    tail_offset: usize,
    way_out: &str,
) -> Result<()> {
    if trailing_partial_bytes == 0 {
        return Ok(());
    }
    Err(PrikkError::Integrity(format!(
        "{file_label} has an incomplete tail at byte offset {tail_offset} ({trailing_partial_bytes} \
         byte(s) follow); {way_out}"
    )))
}

#[cfg(test)]
mod tests {
    use super::require_no_unclean_tail;

    #[test]
    fn a_clean_tail_is_a_no_op() {
        assert!(require_no_unclean_tail("the file", 0, 123, "do something").is_ok());
    }

    #[test]
    fn an_unclean_tail_refuses_and_names_the_offset_and_byte_count() {
        let Err(error) = require_no_unclean_tail("the pointer index", 7, 584, "run doctor") else {
            panic!("a nonzero trailing_partial_bytes must refuse");
        };
        let message = error.to_string();
        assert!(message.contains("the pointer index"), "{message}");
        assert!(message.contains("584"), "{message}");
        assert!(message.contains("7 byte"), "{message}");
        assert!(message.contains("run doctor"), "{message}");
    }
}

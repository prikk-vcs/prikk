//! Merge execution tests.

use prikk_error::PrikkError;
use prikk_object::ObjectId;

use super::ensure_into_ref_unmoved;

/// DC-75 two-edits handoff §6: a ref another writer advanced between evidence and seal is a lock
/// conflict to retry, not damage and not a precondition.
#[test]
fn an_into_ref_advanced_during_evidence_is_a_lock_conflict() {
    let read = ObjectId::from_bytes([1; 32]);
    let advanced = ObjectId::from_bytes([2; 32]);
    assert!(ensure_into_ref_unmoved("heads/main", read, read).is_ok());
    match ensure_into_ref_unmoved("heads/main", advanced, read) {
        Err(PrikkError::LockConflict(message)) => assert_eq!(
            message,
            "ref heads/main advanced during merge evidence gathering; retry"
        ),
        other => panic!("expected a lock conflict, got {other:?}"),
    }
}

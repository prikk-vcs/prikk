use super::*;

/// `Display` is unchanged from the wrapped `PrikkError`'s own -- stderr text this round must not
/// touch.
#[test]
fn display_forwards_to_the_wrapped_prikk_error_unchanged() {
    let inner = PrikkError::Precondition("recognition claim deadbeef does not exist".to_string());
    let wrapped = ExtractError::classified(ExtractRefusalKind::ClaimNotFound, inner.clone());
    assert_eq!(wrapped.to_string(), inner.to_string());
}

/// An error that reaches `ExtractError` through `?` (never through `classified`) carries no
/// classification -- the CLI's own fallback then classifies by `PrikkError`'s own variant.
#[test]
fn plain_conversion_carries_no_classification() {
    let inner = PrikkError::Integrity("a stored object is damaged".to_string());
    let wrapped: ExtractError = inner.clone().into();
    assert_eq!(wrapped.kind, None);
    assert_eq!(wrapped.error, inner);
}

/// The inverse conversion recovers exactly the ordinary error, so a caller that still wants
/// `prikk_error::Result` (an existing test's own `?`, or `.to_string()`) sees nothing new.
#[test]
fn converts_back_to_the_ordinary_prikk_error() {
    let inner = PrikkError::Precondition("boom".to_string());
    let wrapped = ExtractError::classified(ExtractRefusalKind::CaseCollision, inner.clone());
    let recovered: PrikkError = wrapped.into();
    assert_eq!(recovered, inner);
}

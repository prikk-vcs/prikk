use super::resync_to_next_magic;

const MAGIC: &[u8] = b"MAGIC123";

#[test]
fn finds_magic_at_the_scan_start() {
    let bytes = b"MAGIC123trailing".to_vec();
    assert_eq!(resync_to_next_magic(&bytes, 0, MAGIC), Some(0));
}

#[test]
fn finds_magic_after_a_prefix() {
    let mut bytes = vec![0xAA; 5];
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(b"trailing");
    assert_eq!(resync_to_next_magic(&bytes, 0, MAGIC), Some(5));
}

#[test]
fn scan_start_after_a_match_finds_the_next_one() {
    let mut bytes = MAGIC.to_vec();
    bytes.extend_from_slice(b"body");
    bytes.extend_from_slice(MAGIC);
    let second = resync_to_next_magic(&bytes, 1, MAGIC);
    assert_eq!(second, Some(MAGIC.len() + 4));
}

#[test]
fn returns_none_when_magic_is_entirely_absent() {
    let bytes = vec![0xAA; 64];
    assert_eq!(resync_to_next_magic(&bytes, 0, MAGIC), None);
}

#[test]
fn returns_none_when_fewer_than_magic_len_bytes_remain() {
    let bytes = vec![0xAA; 3];
    assert_eq!(resync_to_next_magic(&bytes, 0, MAGIC), None);
}

#[test]
fn empty_buffer_returns_none() {
    assert_eq!(resync_to_next_magic(&[], 0, MAGIC), None);
}

#[test]
fn start_past_the_buffer_end_returns_none_rather_than_panicking() {
    let bytes = vec![0xAA; 4];
    assert_eq!(resync_to_next_magic(&bytes, 100, MAGIC), None);
}

//! RFC 131 §2 / RFC 130 §2.3's "foundation" candidate, adopted here: `layout`, `fsutil`,
//! `byte_cursor`, `file_codec`, `frame_resync`, `container`, `index`, `generation` share one role --
//! a wide, one-directional base every layer above reads from, none of them reaching back up.

pub(crate) mod byte_cursor;
pub(crate) mod container;
pub(crate) mod file_codec;
pub(crate) mod frame_resync;
pub(crate) mod fsutil;
pub(crate) mod generation;
pub(crate) mod index;
pub(crate) mod layout;

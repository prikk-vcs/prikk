//! Checked byte cursor for deterministic file codecs.

use prikk_error::{PrikkError, Result};

/// Cursor that only reads through checked ranges.
pub(crate) struct ByteCursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> ByteCursor<'a> {
    /// Create a cursor for bytes.
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    /// Return true when all bytes were consumed.
    pub(crate) fn is_finished(&self) -> bool {
        self.pos == self.bytes.len()
    }

    /// Read a fixed-size array.
    pub(crate) fn read_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let slice = self.read_exact(N)?;
        let mut out = [0_u8; N];
        out.copy_from_slice(slice);
        Ok(out)
    }

    /// Read a u16 in big-endian order.
    pub(crate) fn read_u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.read_array::<2>()?))
    }

    /// Read a u32 in big-endian order.
    pub(crate) fn read_u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.read_array::<4>()?))
    }

    /// Read a u64 in big-endian order.
    pub(crate) fn read_u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.read_array::<8>()?))
    }

    /// Read a UTF-8 string prefixed by a u16 length.
    pub(crate) fn read_string_u16(&mut self) -> Result<String> {
        let len = usize::from(self.read_u16()?);
        let bytes = self.read_exact(len)?.to_vec();
        String::from_utf8(bytes)
            .map_err(|err| PrikkError::MalformedData(format!("invalid utf-8 string: {err}")))
    }

    /// Read bytes prefixed by a u32 length.
    pub(crate) fn read_bytes_u32(&mut self) -> Result<Vec<u8>> {
        let len = usize::try_from(self.read_u32()?)
            .map_err(|_| PrikkError::MalformedData("u32 length does not fit usize".to_string()))?;
        Ok(self.read_exact(len)?.to_vec())
    }

    /// Read bytes prefixed by a u64 length.
    pub(crate) fn read_bytes_u64(&mut self) -> Result<Vec<u8>> {
        let len = usize::try_from(self.read_u64()?)
            .map_err(|_| PrikkError::MalformedData("u64 length does not fit usize".to_string()))?;
        Ok(self.read_exact(len)?.to_vec())
    }

    /// Read exactly len bytes.
    pub(crate) fn read_exact(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| PrikkError::MalformedData("record length overflow".to_string()))?;
        let Some(slice) = self.bytes.get(self.pos..end) else {
            return Err(PrikkError::MalformedData(
                "unexpected end of record".to_string(),
            ));
        };
        self.pos = end;
        Ok(slice)
    }
}

//! Attestation payload types.

use prikk_error::{PrikkError, Result};

use crate::{CanonicalEncode, CanonicalWriter, ObjectId};

/// Attestation status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum AttestationStatus {
    /// Passed policy.
    Pass = 1,
    /// Warning.
    Warn = 2,
    /// Failed policy.
    Fail = 3,
    /// Locally quarantined.
    Quarantine = 4,
}

impl AttestationStatus {
    /// Stable code.
    #[must_use]
    pub const fn code(self) -> u16 {
        self as u16
    }

    /// Parse a stable code.
    pub fn from_code(code: u16) -> Result<Self> {
        match code {
            1 => Ok(Self::Pass),
            2 => Ok(Self::Warn),
            3 => Ok(Self::Fail),
            4 => Ok(Self::Quarantine),
            other => Err(PrikkError::MalformedData(format!(
                "unknown attestation status code: {other}"
            ))),
        }
    }
}

/// Plugin result entry, sorted by plugin ID. FDD-03 §9.10. The canonical sort key
/// is `plugin_id` only (see `results_sorted_by_plugin_id`), so a full-record
/// ordering is deliberately not derived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginResultEntry {
    /// Plugin ID.
    pub plugin_id: String,
    /// Plugin version.
    pub plugin_version: String,
    /// Status.
    pub status: AttestationStatus,
    /// Report hash (not an object id).
    pub report_hash: Vec<u8>,
    /// Number of findings reported.
    pub finding_count: u32,
}

impl CanonicalEncode for PluginResultEntry {
    fn encode_canonical(&self, writer: &mut CanonicalWriter) -> Result<()> {
        writer.field_string(1, &self.plugin_id)?;
        writer.field_string(2, &self.plugin_version)?;
        writer.field_enum_u16(3, self.status.code())?;
        writer.field_bytes(4, &self.report_hash)?;
        writer.field_u32(5, self.finding_count)?;
        Ok(())
    }
}

/// Attestation payload. FDD-03 §9.9.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttestationPayload {
    /// Target block ID.
    pub target_block_id: ObjectId,
    /// Policy version string.
    pub policy_version: String,
    /// Plugin-set hash.
    pub plugin_set_hash: Vec<u8>,
    /// Results sorted by plugin ID.
    pub results: Vec<PluginResultEntry>,
    /// Overall status.
    pub status: AttestationStatus,
    /// Authoritative attestation creation timestamp.
    pub created_at: u64,
    /// True if this result can be reproduced offline from stored inputs.
    pub is_reproducible_offline: bool,
}

/// FDD-03 §9.9 sort key for `results`: strictly ascending by `plugin_id` UTF-8
/// bytes. Strictness also forbids duplicate `plugin_id` values. No secondary key is
/// used in v1, so later fields never participate in canonical ordering.
fn results_sorted_by_plugin_id(results: &[PluginResultEntry]) -> bool {
    results.windows(2).all(|pair| match pair {
        [a, b] => a.plugin_id.as_bytes() < b.plugin_id.as_bytes(),
        _ => true,
    })
}

impl CanonicalEncode for AttestationPayload {
    fn encode_canonical(&self, writer: &mut CanonicalWriter) -> Result<()> {
        if !results_sorted_by_plugin_id(&self.results) {
            return Err(PrikkError::CanonicalEncoding(
                "plugin results must be strictly ordered and unique by plugin_id".to_string(),
            ));
        }
        writer.field_object_id(1, &self.target_block_id)?;
        writer.field_string(2, &self.policy_version)?;
        writer.field_bytes(3, &self.plugin_set_hash)?;
        writer.repeated_record_list(4, &self.results)?;
        writer.field_enum_u16(5, self.status.code())?;
        writer.field_u64(6, self.created_at)?;
        writer.field_bool(7, self.is_reproducible_offline)?;
        Ok(())
    }
}

impl AttestationPayload {
    /// Decode an Attestation payload from Prikk canonical TLV bytes (0.50.0 step 2 Part B: the
    /// decoder step 5 round 2 found missing -- `verify/reachability.rs`'s own frontier walk cannot
    /// push `target_block_id` without one). No format change: this reads the same wire bytes
    /// `encode_canonical` above has always written.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self> {
        let mut cursor = AttestationCursor::new(bytes);
        let mut target_block_id = None;
        let mut policy_version = None;
        let mut plugin_set_hash = None;
        let mut results = Vec::new();
        let mut status = None;
        let mut created_at = None;
        let mut is_reproducible_offline = None;
        while let Some(field) = cursor.next_field()? {
            match field.tag {
                1 => {
                    if target_block_id.is_some() {
                        return Err(PrikkError::MalformedData(
                            "duplicate Attestation target_block_id field".to_string(),
                        ));
                    }
                    target_block_id = Some(field.read_object_id()?);
                }
                2 => {
                    if policy_version.is_some() {
                        return Err(PrikkError::MalformedData(
                            "duplicate Attestation policy_version field".to_string(),
                        ));
                    }
                    policy_version = Some(field.read_string()?);
                }
                3 => {
                    if plugin_set_hash.is_some() {
                        return Err(PrikkError::MalformedData(
                            "duplicate Attestation plugin_set_hash field".to_string(),
                        ));
                    }
                    plugin_set_hash = Some(field.read_bytes()?);
                }
                4 => results.push(PluginResultEntry::decode_canonical(field.read_record()?)?),
                5 => {
                    if status.is_some() {
                        return Err(PrikkError::MalformedData(
                            "duplicate Attestation status field".to_string(),
                        ));
                    }
                    status = Some(AttestationStatus::from_code(field.read_enum_u16()?)?);
                }
                6 => {
                    if created_at.is_some() {
                        return Err(PrikkError::MalformedData(
                            "duplicate Attestation created_at field".to_string(),
                        ));
                    }
                    created_at = Some(field.read_u64()?);
                }
                7 => {
                    if is_reproducible_offline.is_some() {
                        return Err(PrikkError::MalformedData(
                            "duplicate Attestation is_reproducible_offline field".to_string(),
                        ));
                    }
                    is_reproducible_offline = Some(field.read_bool()?);
                }
                other => {
                    return Err(PrikkError::MalformedData(format!(
                        "unknown Attestation field tag: {other}"
                    )));
                }
            }
        }
        if !results_sorted_by_plugin_id(&results) {
            return Err(PrikkError::MalformedData(
                "plugin results must be strictly ordered and unique by plugin_id".to_string(),
            ));
        }
        Ok(Self {
            target_block_id: target_block_id.ok_or_else(|| {
                PrikkError::MalformedData("Attestation missing target_block_id".to_string())
            })?,
            policy_version: policy_version.ok_or_else(|| {
                PrikkError::MalformedData("Attestation missing policy_version".to_string())
            })?,
            plugin_set_hash: plugin_set_hash.ok_or_else(|| {
                PrikkError::MalformedData("Attestation missing plugin_set_hash".to_string())
            })?,
            results,
            status: status.ok_or_else(|| {
                PrikkError::MalformedData("Attestation missing status".to_string())
            })?,
            created_at: created_at.ok_or_else(|| {
                PrikkError::MalformedData("Attestation missing created_at".to_string())
            })?,
            is_reproducible_offline: is_reproducible_offline.ok_or_else(|| {
                PrikkError::MalformedData("Attestation missing is_reproducible_offline".to_string())
            })?,
        })
    }
}

impl PluginResultEntry {
    /// Decode one plugin-result record from the bytes a `RecordListItem` field's own value carries
    /// (a fully nested canonical TLV blob, the same shape `CanonicalWriter::field_record_list_item`
    /// writes).
    fn decode_canonical(bytes: &[u8]) -> Result<Self> {
        let mut cursor = AttestationCursor::new(bytes);
        let mut plugin_id = None;
        let mut plugin_version = None;
        let mut status = None;
        let mut report_hash = None;
        let mut finding_count = None;
        while let Some(field) = cursor.next_field()? {
            match field.tag {
                1 => {
                    if plugin_id.is_some() {
                        return Err(PrikkError::MalformedData(
                            "duplicate PluginResultEntry plugin_id field".to_string(),
                        ));
                    }
                    plugin_id = Some(field.read_string()?);
                }
                2 => {
                    if plugin_version.is_some() {
                        return Err(PrikkError::MalformedData(
                            "duplicate PluginResultEntry plugin_version field".to_string(),
                        ));
                    }
                    plugin_version = Some(field.read_string()?);
                }
                3 => {
                    if status.is_some() {
                        return Err(PrikkError::MalformedData(
                            "duplicate PluginResultEntry status field".to_string(),
                        ));
                    }
                    status = Some(AttestationStatus::from_code(field.read_enum_u16()?)?);
                }
                4 => {
                    if report_hash.is_some() {
                        return Err(PrikkError::MalformedData(
                            "duplicate PluginResultEntry report_hash field".to_string(),
                        ));
                    }
                    report_hash = Some(field.read_bytes()?);
                }
                5 => {
                    if finding_count.is_some() {
                        return Err(PrikkError::MalformedData(
                            "duplicate PluginResultEntry finding_count field".to_string(),
                        ));
                    }
                    finding_count = Some(field.read_u32()?);
                }
                other => {
                    return Err(PrikkError::MalformedData(format!(
                        "unknown PluginResultEntry field tag: {other}"
                    )));
                }
            }
        }
        Ok(Self {
            plugin_id: plugin_id.ok_or_else(|| {
                PrikkError::MalformedData("PluginResultEntry missing plugin_id".to_string())
            })?,
            plugin_version: plugin_version.ok_or_else(|| {
                PrikkError::MalformedData("PluginResultEntry missing plugin_version".to_string())
            })?,
            status: status.ok_or_else(|| {
                PrikkError::MalformedData("PluginResultEntry missing status".to_string())
            })?,
            report_hash: report_hash.ok_or_else(|| {
                PrikkError::MalformedData("PluginResultEntry missing report_hash".to_string())
            })?,
            finding_count: finding_count.ok_or_else(|| {
                PrikkError::MalformedData("PluginResultEntry missing finding_count".to_string())
            })?,
        })
    }
}

struct AttestationCursor<'a> {
    bytes: &'a [u8],
    pos: usize,
    last_tag: Option<u16>,
}

impl<'a> AttestationCursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            pos: 0,
            last_tag: None,
        }
    }

    fn next_field(&mut self) -> Result<Option<AttestationField<'a>>> {
        if self.pos == self.bytes.len() {
            return Ok(None);
        }
        let tag = u16::from_be_bytes(self.read_array::<2>()?);
        if tag == 0 {
            return Err(PrikkError::MalformedData(
                "field tag 0 is reserved".to_string(),
            ));
        }
        if let Some(last) = self.last_tag {
            if tag < last {
                return Err(PrikkError::MalformedData(format!(
                    "field tag order violation: {tag} after {last}"
                )));
            }
        }
        self.last_tag = Some(tag);
        let wire_type = self.read_u8()?;
        let len = usize::try_from(u64::from_be_bytes(self.read_array::<8>()?)).map_err(|_| {
            PrikkError::MalformedData("canonical field length does not fit usize".to_string())
        })?;
        let value = self.read_exact(len)?;
        Ok(Some(AttestationField {
            tag,
            wire_type,
            value,
        }))
    }

    fn read_u8(&mut self) -> Result<u8> {
        let value = self.read_exact(1)?;
        let Some(byte) = value.first() else {
            return Err(PrikkError::MalformedData(
                "unexpected empty byte".to_string(),
            ));
        };
        Ok(*byte)
    }

    fn read_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let bytes = self.read_exact(N)?;
        let mut out = [0_u8; N];
        out.copy_from_slice(bytes);
        Ok(out)
    }

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| PrikkError::MalformedData("canonical range overflow".to_string()))?;
        let Some(slice) = self.bytes.get(self.pos..end) else {
            return Err(PrikkError::MalformedData(
                "unexpected end of canonical payload".to_string(),
            ));
        };
        self.pos = end;
        Ok(slice)
    }
}

struct AttestationField<'a> {
    tag: u16,
    wire_type: u8,
    value: &'a [u8],
}

impl<'a> AttestationField<'a> {
    fn read_string(&self) -> Result<String> {
        self.require_wire(crate::canonical::WireType::String)?;
        String::from_utf8(self.value.to_vec())
            .map_err(|err| PrikkError::MalformedData(format!("invalid UTF-8 string: {err}")))
    }

    fn read_bytes(&self) -> Result<Vec<u8>> {
        self.require_wire(crate::canonical::WireType::Bytes)?;
        Ok(self.value.to_vec())
    }

    fn read_bool(&self) -> Result<bool> {
        self.require_wire(crate::canonical::WireType::Bool)?;
        match self.value {
            [0] => Ok(false),
            [1] => Ok(true),
            _ => Err(PrikkError::MalformedData(format!(
                "field {} is not a valid bool",
                self.tag
            ))),
        }
    }

    fn read_u32(&self) -> Result<u32> {
        self.require_wire(crate::canonical::WireType::U32)?;
        Ok(u32::from_be_bytes(self.read_array::<4>()?))
    }

    fn read_u64(&self) -> Result<u64> {
        self.require_wire(crate::canonical::WireType::U64)?;
        Ok(u64::from_be_bytes(self.read_array::<8>()?))
    }

    fn read_enum_u16(&self) -> Result<u16> {
        self.require_wire(crate::canonical::WireType::EnumU16)?;
        Ok(u16::from_be_bytes(self.read_array::<2>()?))
    }

    fn read_object_id(&self) -> Result<ObjectId> {
        self.require_wire(crate::canonical::WireType::ObjectId)?;
        Ok(ObjectId::from_bytes(self.read_array::<32>()?))
    }

    /// The nested canonical bytes a `RecordListItem` field's own value carries.
    fn read_record(&self) -> Result<&'a [u8]> {
        self.require_wire(crate::canonical::WireType::RecordListItem)?;
        Ok(self.value)
    }

    fn require_wire(&self, expected: crate::canonical::WireType) -> Result<()> {
        if self.wire_type == expected as u8 {
            return Ok(());
        }
        Err(PrikkError::MalformedData(format!(
            "field {} has wrong wire type: expected {}, got {}",
            self.tag, expected as u8, self.wire_type
        )))
    }

    fn read_array<const N: usize>(&self) -> Result<[u8; N]> {
        if self.value.len() != N {
            return Err(PrikkError::MalformedData(format!(
                "field {} expected {N} bytes, got {}",
                self.tag,
                self.value.len()
            )));
        }
        let mut out = [0_u8; N];
        out.copy_from_slice(self.value);
        Ok(out)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::CanonicalWriter;

    fn sample() -> AttestationPayload {
        AttestationPayload {
            target_block_id: ObjectId::from_bytes([3; 32]),
            policy_version: "v1".to_string(),
            plugin_set_hash: vec![1, 2, 3],
            results: vec![
                PluginResultEntry {
                    plugin_id: "a".to_string(),
                    plugin_version: "1.0".to_string(),
                    status: AttestationStatus::Pass,
                    report_hash: vec![9, 9],
                    finding_count: 0,
                },
                PluginResultEntry {
                    plugin_id: "b".to_string(),
                    plugin_version: "2.0".to_string(),
                    status: AttestationStatus::Warn,
                    report_hash: vec![8],
                    finding_count: 2,
                },
            ],
            status: AttestationStatus::Warn,
            created_at: 0,
            is_reproducible_offline: true,
        }
    }

    #[test]
    fn encode_then_decode_round_trips() {
        let payload = sample();
        let mut writer = CanonicalWriter::new();
        payload.encode_canonical(&mut writer).expect("encode");
        let bytes = writer.finish();
        let decoded = AttestationPayload::decode_canonical(&bytes).expect("decode");
        assert_eq!(decoded, payload);
    }

    #[test]
    fn decode_refuses_a_duplicate_target_block_id_field() {
        let payload = sample();
        let mut writer = CanonicalWriter::new();
        payload.encode_canonical(&mut writer).expect("encode");
        let mut bytes = writer.finish();
        // Field 1 (target_block_id) re-appended after field 7: a tag-order violation, which
        // `AttestationCursor` must refuse rather than silently accept a second value.
        bytes.extend_from_slice(&1_u16.to_be_bytes());
        bytes.push(crate::canonical::WireType::ObjectId as u8);
        bytes.extend_from_slice(&32_u64.to_be_bytes());
        bytes.extend_from_slice(&[0; 32]);
        assert!(AttestationPayload::decode_canonical(&bytes).is_err());
    }

    #[test]
    fn decode_refuses_truncated_bytes() {
        let payload = sample();
        let mut writer = CanonicalWriter::new();
        payload.encode_canonical(&mut writer).expect("encode");
        let bytes = writer.finish();
        assert!(AttestationPayload::decode_canonical(&bytes[..bytes.len() - 1]).is_err());
    }
}

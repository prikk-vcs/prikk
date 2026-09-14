use super::*;

/// Builds a real repository on disk: real signed objects, blocks, and refs, published through the
/// same primitives an ordinary repository uses, with `.prikk/FORMAT` flipped to `target_format`
/// (`b"1\n"` or `b"2\n"`) only as the final step. RFC 103 design-v1.md §5 acceptance criterion 2 /
/// RFC 102 Stage 3, design-v1.md §12.1 both require the open-time rejection proven against a fixture
/// like this, not a hand-built one -- one real fixture builder shared by both retired formats.
pub(crate) fn build_legacy_fixture(
    root: &Path,
    active: ActiveFixture,
    target_format: &[u8],
) -> TestResult {
    let layout = RepositoryLayout::init(root.to_path_buf())?;
    let author = Ed25519AuthorSigner::from_seed("legacy-author", &[0x36; 32])?;
    let maintainer = Ed25519MaintainerSigner::from_seed(MAINTAINER_KEY_ID, &[0x35; 32])?;
    add_trusted_maintainer(
        &layout,
        MAINTAINER_KEY_ID,
        &hex(&maintainer.public_key_bytes()),
    )?;

    let readme_blob = write_blob(&layout, b"hello\n", &maintainer)?;
    let old_blob = write_blob(&layout, b"old\n", &maintainer)?;
    let extra_blob = write_blob(&layout, b"extra\n", &maintainer)?;
    let root_patch = write_patch(
        &layout,
        vec![
            operation(
                1,
                OperationKind::CreateFile(CreateFile {
                    path: "README.md".to_string(),
                    node_id: NodeId::from_bytes([0x70; 32]),
                    blob_id: readme_blob,
                    mode: 0o100644,
                }),
            ),
            operation(
                2,
                OperationKind::CreateFile(CreateFile {
                    path: "old.txt".to_string(),
                    node_id: NodeId::from_bytes([0x71; 32]),
                    blob_id: old_blob,
                    mode: 0o100644,
                }),
            ),
        ],
        PatchPurpose::Normal,
        &author,
    )?;

    // RFC 136 §10.1a: the left block's own state, after its patch. This repository is refused at
    // open, so nothing reads the manifest; it stays so the retired-format fixture keeps a snapshot.
    let snapshot = SnapshotManifest {
        entries: vec![
            StateRootEntry {
                path: prikk_store::RepoPath::parse("README.md")?,
                node_id: NodeId::from_bytes([0x70; 32]),
                kind: NodeKind::TextFile,
                mode: 0o100644,
                content: StateRootContent::Blob(readme_blob),
            },
            StateRootEntry {
                path: prikk_store::RepoPath::parse("extra.txt")?,
                node_id: NodeId::from_bytes([0x72; 32]),
                kind: NodeKind::TextFile,
                mode: 0o100644,
                content: StateRootContent::Blob(extra_blob),
            },
        ],
    };
    let snapshot_blob = write_snapshot(&layout, snapshot.encode()?, &maintainer)?;
    let left_patch = write_patch(
        &layout,
        vec![
            operation(
                1,
                OperationKind::DeleteNode(DeleteNode {
                    path: "old.txt".to_string(),
                    node_id: NodeId::from_bytes([0x71; 32]),
                    old_node_kind: NodeKind::TextFile,
                    preimage: DeleteNodePreimage::File {
                        old_blob_id: old_blob,
                        old_mode: 0o100644,
                    },
                }),
            ),
            operation(
                2,
                OperationKind::CreateFile(CreateFile {
                    path: "extra.txt".to_string(),
                    node_id: NodeId::from_bytes([0x72; 32]),
                    blob_id: extra_blob,
                    mode: 0o100644,
                }),
            ),
        ],
        PatchPurpose::Normal,
        &author,
    )?;
    let right_patch = write_patch(
        &layout,
        vec![operation(
            1,
            OperationKind::ChangePerm(ChangePerm {
                node_id: NodeId::from_bytes([0x70; 32]),
                old_mode: 0o100644,
                new_mode: 0o100755,
            }),
        )],
        PatchPurpose::Normal,
        &author,
    )?;

    let root_block = write_block(
        &layout,
        BlockKind::Root,
        Vec::new(),
        vec![root_patch],
        None,
        &maintainer,
    )?;
    let left_block = write_block(
        &layout,
        BlockKind::Normal,
        vec![root_block],
        vec![left_patch],
        Some(snapshot_blob),
        &maintainer,
    )?;
    // Written for repository realism (a second branch point off `root_block`) but not otherwise
    // read by this fixture -- see `mod.rs`, its id used to be exposed for merge-evidence coverage
    // that lived in the now-removed bounded-legacy-mode command matrix.
    let _right_block = write_block(
        &layout,
        BlockKind::Normal,
        vec![root_block],
        vec![right_patch],
        None,
        &maintainer,
    )?;
    let (state_id, update) = write_main_ref(&layout, left_block, &maintainer)?;
    codec::write_ref_pointer(&layout, "heads/main", state_id)?;
    codec::write_ref_log(&layout, "heads/main", &update)?;

    let active_patch = match active {
        ActiveFixture::RollbackDraft => {
            write_rollback_draft(&layout, old_blob, extra_blob, &author)?
        }
        ActiveFixture::InterruptedPublication => left_patch,
    };
    let active_envelope = read_envelope_for_wal(&layout, active_patch)?;
    write_active_ref_metadata(&layout, "heads/main")?;
    codec::write_wal(&layout, &active_envelope)?;
    if matches!(active, ActiveFixture::InterruptedPublication) {
        codec::remove_pointer(&layout, "heads/main")?;
    }

    std::fs::write(root.join("README.md"), b"hello\n")?;
    std::fs::write(root.join("old.txt"), b"old\n")?;
    std::fs::write(layout.format_path(), target_format)?;
    Ok(())
}

pub(crate) fn build_current_format_strict_wal_fixture(
    root: &Path,
    failure: StrictFailure,
) -> TestResult {
    let layout = RepositoryLayout::init(root.to_path_buf())?;
    let payload = PatchPayload {
        operations: vec![operation(
            1,
            OperationKind::CreateFile(CreateFile {
                path: "strict.txt".to_string(),
                node_id: NodeId::from_bytes([0x55; 32]),
                blob_id: ObjectId::from_bytes([0x66; 32]),
                mode: 0o100644,
            }),
        )],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let signature = |key_id: &str, byte: u8| Signature {
        algorithm: SignatureAlgorithm::Ed25519,
        key_id: key_id.to_string(),
        signature_bytes: vec![byte; 64],
        created_at: 0,
        signer_role: SignerRole::Author,
    };
    let signatures = match failure {
        StrictFailure::MalformedLength => {
            let mut malformed = signature("malformed", 1);
            malformed.signature_bytes.truncate(63);
            vec![malformed]
        }
        StrictFailure::Duplicate => {
            let duplicate = signature("duplicate", 2);
            vec![duplicate.clone(), duplicate]
        }
        StrictFailure::InvertedOrder => vec![signature("z", 3), signature("a", 1)],
    };
    let envelope = ObjectEnvelope {
        object_type: ObjectType::Patch,
        schema_version: 1,
        canonical_payload: payload.to_canonical_bytes()?,
        signatures,
    };
    codec::write_wal(&layout, &envelope)
}

fn write_blob(
    layout: &RepositoryLayout,
    content: &[u8],
    signer: &impl MaintainerSigner,
) -> TestResult<ObjectId> {
    let payload = BlobPayload::new(BlobKind::Text, content.to_vec());
    write_maintainer_envelope(
        layout,
        ObjectType::Blob,
        payload.to_canonical_bytes()?,
        signer,
    )
}

fn write_snapshot(
    layout: &RepositoryLayout,
    content: Vec<u8>,
    signer: &impl MaintainerSigner,
) -> TestResult<ObjectId> {
    let payload = BlobPayload::new(BlobKind::Snapshot, content);
    write_maintainer_envelope(
        layout,
        ObjectType::Blob,
        payload.to_canonical_bytes()?,
        signer,
    )
}

fn write_patch(
    layout: &RepositoryLayout,
    operations: Vec<Operation>,
    purpose: PatchPurpose,
    signer: &impl AuthorSigner,
) -> TestResult<ObjectId> {
    let payload = PatchPayload {
        operations,
        intent: None,
        preconditions: Vec::new(),
        purpose,
        message: None,
    };
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes()?);
    envelope.add_signature(author_signature(signer, envelope.object_id())?)?;
    codec::write_object(layout, &envelope)
}

fn write_block(
    layout: &RepositoryLayout,
    kind: BlockKind,
    parents: Vec<ObjectId>,
    patches: Vec<ObjectId>,
    snapshot: Option<ObjectId>,
    signer: &impl MaintainerSigner,
) -> TestResult<ObjectId> {
    let payload = BlockPayload {
        parent_block_ids: parents,
        kind,
        patch_ids: patches,
        state_merkle_root: MerkleRoot([0; 32]),
        snapshot_blob_ref: snapshot,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    write_maintainer_envelope(
        layout,
        ObjectType::Block,
        payload.to_canonical_bytes()?,
        signer,
    )
}

fn write_main_ref(
    layout: &RepositoryLayout,
    block_id: ObjectId,
    signer: &impl MaintainerSigner,
) -> TestResult<(ObjectId, ObjectEnvelope)> {
    let state = RefStatePayload {
        ref_name: "heads/main".to_string(),
        kind: RefKind::Branch,
        target_object_id: block_id,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let state_id = write_maintainer_envelope(
        layout,
        ObjectType::RefState,
        state.to_canonical_bytes()?,
        signer,
    )?;
    let update = RefUpdatePayload {
        ref_name: "heads/main".to_string(),
        old_ref_state_id: None,
        new_ref_state_id: state_id,
        new_target_object_id: block_id,
        update_seq: 1,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::RefUpdate, 1, update.to_canonical_bytes()?);
    envelope.add_signature(maintainer_signature(
        signer,
        ObjectType::RefUpdate,
        envelope.object_id(),
    )?)?;
    Ok((state_id, envelope))
}

fn write_rollback_draft(
    layout: &RepositoryLayout,
    old_blob: ObjectId,
    extra_blob: ObjectId,
    signer: &impl AuthorSigner,
) -> TestResult<ObjectId> {
    write_patch(
        layout,
        vec![
            Operation {
                op_seq: 1,
                op_id: Some("inverse-delete-extra.txt".to_string()),
                preconditions: Vec::new(),
                kind: OperationKind::DeleteNode(DeleteNode {
                    path: "extra.txt".to_string(),
                    node_id: NodeId::from_bytes([0x72; 32]),
                    old_node_kind: NodeKind::TextFile,
                    preimage: DeleteNodePreimage::File {
                        old_blob_id: extra_blob,
                        old_mode: 0o100644,
                    },
                }),
            },
            Operation {
                op_seq: 2,
                op_id: Some("inverse-create-old.txt".to_string()),
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "old.txt".to_string(),
                    node_id: NodeId::from_bytes([0x71; 32]),
                    blob_id: old_blob,
                    mode: 0o100644,
                }),
            },
        ],
        PatchPurpose::RollbackDraft,
        signer,
    )
}

fn read_envelope_for_wal(
    layout: &RepositoryLayout,
    patch_id: ObjectId,
) -> TestResult<ObjectEnvelope> {
    decode_envelope(&std::fs::read(
        layout.object_path(ObjectType::Patch, patch_id),
    )?)
}

fn decode_envelope(bytes: &[u8]) -> TestResult<ObjectEnvelope> {
    // Fixture envelopes are retained separately so WAL construction can reuse exact bytes. Decode
    // through a temporary format-2 repository is intentionally avoided; this parser mirrors POBJ0001.
    let mut cursor = 8_usize;
    let object_type = ObjectType::from_code(read_u16(bytes, &mut cursor)?)?;
    let schema_version = read_u32(bytes, &mut cursor)?;
    let payload_len = usize::try_from(read_u64(bytes, &mut cursor)?)?;
    let canonical_payload = take(bytes, &mut cursor, payload_len)?.to_vec();
    let signature_count = read_u32(bytes, &mut cursor)?;
    let mut signatures = Vec::new();
    for _ in 0..signature_count {
        let algorithm = prikk_object::SignatureAlgorithm::from_code(read_u16(bytes, &mut cursor)?)?;
        let role = prikk_object::SignerRole::from_code(read_u16(bytes, &mut cursor)?)?;
        let key_len = usize::from(read_u16(bytes, &mut cursor)?);
        let key_id = String::from_utf8(take(bytes, &mut cursor, key_len)?.to_vec())?;
        let created_at = read_u64(bytes, &mut cursor)?;
        let signature_len = usize::try_from(read_u32(bytes, &mut cursor)?)?;
        let signature_bytes = take(bytes, &mut cursor, signature_len)?.to_vec();
        signatures.push(prikk_object::Signature {
            algorithm,
            key_id,
            signature_bytes,
            created_at,
            signer_role: role,
        });
    }
    Ok(ObjectEnvelope {
        object_type,
        schema_version,
        canonical_payload,
        signatures,
    })
}

fn write_maintainer_envelope(
    layout: &RepositoryLayout,
    object_type: ObjectType,
    payload: Vec<u8>,
    signer: &impl MaintainerSigner,
) -> TestResult<ObjectId> {
    let mut envelope = ObjectEnvelope::unsigned(object_type, 1, payload);
    envelope.add_signature(maintainer_signature(
        signer,
        object_type,
        envelope.object_id(),
    )?)?;
    codec::write_object(layout, &envelope)
}

fn operation(op_seq: u32, kind: OperationKind) -> Operation {
    Operation {
        op_seq,
        op_id: None,
        preconditions: Vec::new(),
        kind,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn take<'a>(bytes: &'a [u8], cursor: &mut usize, len: usize) -> TestResult<&'a [u8]> {
    let end = cursor.checked_add(len).ok_or("fixture decode overflow")?;
    let value = bytes
        .get(*cursor..end)
        .ok_or("fixture decode out of range")?;
    *cursor = end;
    Ok(value)
}

fn read_u16(bytes: &[u8], cursor: &mut usize) -> TestResult<u16> {
    Ok(u16::from_be_bytes(take(bytes, cursor, 2)?.try_into()?))
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> TestResult<u32> {
    Ok(u32::from_be_bytes(take(bytes, cursor, 4)?.try_into()?))
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> TestResult<u64> {
    Ok(u64::from_be_bytes(take(bytes, cursor, 8)?.try_into()?))
}

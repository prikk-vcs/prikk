//! A repository sealed through the real seal path (`simulate_one_seal` → `seal_block`), so that a checkpoint
//! snapshot and a replay-verified record exist, whose history edits the same files repeatedly (RFC 136
//! increment 2c). Node ids and signer seeds are fixed, so two repositories built from the same operations hold
//! the same patches.
//!
//! The history [`AnchoredHistory::standard`] builds, block by block:
//! 1. creates `a.txt` (node A), `b.txt` (B) and `c.txt` (C) -- the first block is a checkpoint;
//! 2. edits A; 3. edits A again; 4. edits B; 5. deletes C (a tombstone that stays); 6. creates `d.txt` (D);
//! 7. edits A -- **the shortest tip that edits text an earlier block edited**;
//! 8. deletes D; 9. recreates D (a restoration the tombstone must equal);
//!
//! then one block per index up to `total`, each creating a filler, editing A every fourth block and B every
//! sixth. With `total` above 64 the second checkpoint falls at block 65.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::collections::BTreeMap;

use prikk_object::{
    BlobKind, BlobPayload, CanonicalEncode, CreateFile, DeleteNode, DeleteNodePreimage, EditText,
    NodeId, NodeKind, ObjectEnvelope, ObjectId, ObjectType, Operation, OperationKind, PatchPayload,
    PatchPurpose,
};

use super::unique_temp_dir;
use crate::rfc111_seal_simulation::simulate_one_seal;
use crate::{
    DEFAULT_ACTIVE_NAME, Ed25519AuthorSigner, Ed25519MaintainerSigner, FileObjectStore,
    MaintainerSigner, ObjectWriter, RepositoryLayout, Wal, add_trusted_maintainer,
    author_signature, write_active_ref_metadata,
};

const MAIN: &str = "heads/main";
const REGULAR: u32 = 0o100_644;

/// Node A, B, C, D of the standard history.
pub(crate) const NODE_A: u8 = 0xA1;
pub(crate) const NODE_B: u8 = 0xB1;
pub(crate) const NODE_C: u8 = 0xC1;
pub(crate) const NODE_D: u8 = 0xD1;

pub(crate) fn node(seed: u8) -> NodeId {
    NodeId::from_bytes([seed; 32])
}

pub(crate) struct AnchoredHistory {
    pub(crate) layout: RepositoryLayout,
    pub(crate) store: FileObjectStore,
    maintainer: Ed25519MaintainerSigner,
    author: Ed25519AuthorSigner,
    text: BTreeMap<u8, Vec<u8>>,
    paths: BTreeMap<u8, String>,
    /// Every block sealed on `heads/main`, oldest first.
    pub(crate) blocks: Vec<ObjectId>,
}

impl AnchoredHistory {
    pub(crate) fn new(name: &str) -> Self {
        let layout = RepositoryLayout::init(unique_temp_dir(name)).unwrap();
        let maintainer =
            Ed25519MaintainerSigner::from_seed("rfc136-2c-maintainer", &[0x2C; 32]).unwrap();
        add_trusted_maintainer(
            &layout,
            maintainer.key_id(),
            &prikk_hash::to_hex(&maintainer.public_key_bytes()),
        )
        .unwrap();
        Self {
            store: FileObjectStore::new(layout.clone()),
            layout,
            maintainer,
            author: Ed25519AuthorSigner::from_seed("rfc136-2c-author", &[0xC2; 32]).unwrap(),
            text: BTreeMap::new(),
            paths: BTreeMap::new(),
            blocks: Vec::new(),
        }
    }

    pub(crate) fn create(&mut self, path: &str, seed: u8, bytes: &[u8]) -> OperationKind {
        let envelope = ObjectEnvelope::unsigned(
            ObjectType::Blob,
            1,
            BlobPayload::new(BlobKind::Text, bytes.to_vec())
                .to_canonical_bytes()
                .unwrap(),
        );
        let blob_id = self.store.write_object(&envelope).unwrap();
        self.text.insert(seed, bytes.to_vec());
        self.paths.insert(seed, path.to_string());
        OperationKind::CreateFile(CreateFile {
            path: path.to_string(),
            node_id: node(seed),
            blob_id,
            mode: REGULAR,
        })
    }

    pub(crate) fn edit(&mut self, seed: u8, new: &[u8]) -> OperationKind {
        let old = self.text.get(&seed).cloned().expect("edit of a known node");
        let span = crate::text_span::plan_authored_text_span(&old, new, node(seed))
            .unwrap()
            .expect("the edit changes something");
        self.text.insert(seed, new.to_vec());
        OperationKind::EditText(EditText {
            node_id: node(seed),
            span_id: span.span_id,
            old_span_hash: span.old_span_hash,
            left_anchor_hash: span.left_anchor_hash,
            right_anchor_hash: span.right_anchor_hash,
            replacement_text: span.replacement_text,
            presentation_hint_line: None,
            presentation_hint_column: None,
            old_span_text: span.old_span_text,
            left_anchor_len: Some(span.left_anchor_len),
            right_anchor_len: Some(span.right_anchor_len),
        })
    }

    pub(crate) fn delete(&mut self, seed: u8) -> OperationKind {
        let bytes = self
            .text
            .get(&seed)
            .cloned()
            .expect("delete of a known node");
        let path = self.paths.get(&seed).cloned().expect("a known path");
        OperationKind::DeleteNode(DeleteNode {
            path,
            node_id: node(seed),
            old_node_kind: NodeKind::TextFile,
            preimage: DeleteNodePreimage::File {
                old_blob_id: crate::text_span::text_blob_id(&bytes).unwrap(),
                old_mode: REGULAR,
            },
        })
    }

    /// The text `seed`'s node holds now.
    pub(crate) fn text(&self, seed: u8) -> &[u8] {
        self.text.get(&seed).expect("a known node")
    }

    pub(crate) fn path(&self, seed: u8) -> &str {
        self.paths.get(&seed).expect("a known node")
    }

    fn append(&self, kinds: Vec<OperationKind>) {
        let operations = kinds
            .into_iter()
            .zip(1_u32..)
            .map(|(kind, op_seq)| Operation {
                op_seq,
                op_id: None,
                preconditions: Vec::new(),
                kind,
            })
            .collect();
        let payload = PatchPayload {
            operations,
            intent: None,
            preconditions: Vec::new(),
            purpose: PatchPurpose::Normal,
            message: None,
        };
        let mut patch = ObjectEnvelope::unsigned(
            ObjectType::Patch,
            prikk_object::PATCH_TEXT_SPAN_V2_SCHEMA,
            payload.to_canonical_bytes().unwrap(),
        );
        let patch_id = patch.object_id();
        patch
            .add_signature(author_signature(&self.author, patch_id).unwrap())
            .unwrap();
        Wal::for_layout(&self.layout, DEFAULT_ACTIVE_NAME)
            .append_patch(&patch)
            .unwrap();
        write_active_ref_metadata(&self.layout, MAIN).unwrap();
    }

    /// Queue one patch of `kinds` on `heads/main` and leave it unsealed.
    pub(crate) fn queue(&self, kinds: Vec<OperationKind>) {
        self.append(kinds);
    }

    /// Seal one patch of `kinds` onto `heads/main` through `seal_block` and return the new tip.
    pub(crate) fn seal(&mut self, kinds: Vec<OperationKind>) -> ObjectId {
        self.append(kinds);
        simulate_one_seal(&self.layout, MAIN, &self.maintainer).unwrap();
        let tip = crate::refs::read_current_ref_tip_block(&self.layout, &self.store, MAIN).unwrap();
        self.blocks.push(tip);
        tip
    }

    /// The lineage's genesis block.
    pub(crate) fn horizon(&self) -> ObjectId {
        self.blocks[0]
    }

    /// The standard history, `total` blocks long (at least 9).
    pub(crate) fn standard(name: &str, total: usize) -> Self {
        assert!(total >= 9);
        let mut h = Self::new(name);
        let a: Vec<u8> = (1..=20)
            .flat_map(|i| format!("A-line-{i}\n").into_bytes())
            .collect();
        let b: Vec<u8> = (1..=10)
            .flat_map(|i| format!("B-line-{i}\n").into_bytes())
            .collect();
        let ops = vec![
            h.create("a.txt", NODE_A, &a),
            h.create("b.txt", NODE_B, &b),
            h.create("c.txt", NODE_C, b"c\n"),
        ];
        h.seal(ops);
        let op = h.edit(
            NODE_A,
            &replace_line(h.text(NODE_A), 2, "A-line-two-edited"),
        );
        h.seal(vec![op]);
        let op = h.edit(
            NODE_A,
            &replace_line(h.text(NODE_A), 3, "A-line-three-edited"),
        );
        h.seal(vec![op]);
        let op = h.edit(
            NODE_B,
            &replace_line(h.text(NODE_B), 1, "B-line-one-edited"),
        );
        h.seal(vec![op]);
        let op = h.delete(NODE_C);
        h.seal(vec![op]);
        let op = h.create("d.txt", NODE_D, b"d\n");
        h.seal(vec![op]);
        let op = h.edit(
            NODE_A,
            &replace_line(h.text(NODE_A), 4, "A-line-four-edited"),
        );
        h.seal(vec![op]);
        let op = h.delete(NODE_D);
        h.seal(vec![op]);
        let op = h.create("d.txt", NODE_D, b"d\n");
        h.seal(vec![op]);
        for index in 10..=total {
            let mut ops = vec![h.create(
                &format!("f{index}.txt"),
                0x20_u8.wrapping_add(u8::try_from(index).unwrap()),
                b"filler\n",
            )];
            if index % 4 == 0 {
                let line = index % 20 + 1;
                let next = replace_line(h.text(NODE_A), line, &format!("A-edit-at-block-{index}"));
                ops.push(h.edit(NODE_A, &next));
            }
            if index % 6 == 0 {
                let next = replace_line(h.text(NODE_B), index % 10 + 1, &format!("B-edit-{index}"));
                ops.push(h.edit(NODE_B, &next));
            }
            h.seal(ops);
        }
        h
    }
}

/// `text` with its 1-based line `line` replaced.
fn replace_line(text: &[u8], line: usize, replacement: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for (index, current) in String::from_utf8(text.to_vec())
        .unwrap()
        .split_inclusive('\n')
        .enumerate()
    {
        if index + 1 == line {
            out.extend_from_slice(format!("{replacement}\n").as_bytes());
        } else {
            out.extend_from_slice(current.as_bytes());
        }
    }
    out
}

impl Drop for AnchoredHistory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.layout.root());
    }
}

//! 0.59.0 P5a3: the subset read against today's full read, over generated claim sets.
//!
//! The oracle is the read every batch made before: every stored claim decoded (`enumerate_stored_claims`), indexed (`ClaimsIndex`), the order check run
//! over the claims that name a patch (`refuse_if_order_ambiguous_indexed`), and the parent check over every claim by block (`claims_by_block`). The
//! subset (`StoredClaims`, scoped to a random batch) must give **the same decision and the same refusal text** for: the order-ambiguity check of every
//! batch claim and of a claim that is **not** in the batch; the parent check of every parent of every stored claim, against a random history (so a
//! parent whose claim is outside the subset is fetched by its offset); and the claim a block resolves to, with its payload. The sets have claims about
//! one block, claims held as several records, overlapping patches in both orders, parents that are held, unheld, and chains several deep.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::collections::BTreeSet;

use prikk_error::Result;
use prikk_object::{ObjectId, ObjectType, RecognitionClaimPayload};

use super::{BatchScope, StoredClaims};
use crate::maintainer_signing::maintainer_signature;
use crate::object_store::ObjectReadSnapshot;
use crate::seal_from_accepted::batch::{ClaimsIndex, check_order};
use crate::seal_from_accepted::tests::{
    base_fixture, cleanup, maintainer_signer, write_claim_with_parents,
};
use crate::seal_from_accepted::{
    ClaimsByBlock, RefHistory, claims_by_block, enumerate_stored_claims, parent_reached,
    refuse_if_order_ambiguous_indexed,
};
use crate::{FileObjectStore, RepositoryLayout};

struct Lcg(u64);

impl Lcg {
    fn next(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from(self.0 >> 33).unwrap() % bound
    }
}

fn id(tag: &str, n: usize) -> ObjectId {
    ObjectId::from_canonical_payload(ObjectType::Blob, 1, format!("{tag}-{n}").as_bytes())
}

/// A repository holding a generated set of claims; the ids of the claims written, in the order written.
fn generated_claims(
    seed: u64,
    layout: &RepositoryLayout,
    signer: &crate::Ed25519MaintainerSigner,
) -> Result<Vec<ObjectId>> {
    let mut rng = Lcg(seed + 1);
    let mut objects = FileObjectStore::new(layout.clone());
    let patches: Vec<ObjectId> = (0..6).map(|n| id("patch", n)).collect();
    let blocks: Vec<ObjectId> = (0..30).map(|n| id("block", n)).collect();
    let mut written = Vec::new();
    for _ in 0..(8 + rng.next(30)) {
        // One to four patches, drawn in a random order, so two claims overlap and agree or disagree on the order.
        let mut named: Vec<ObjectId> = Vec::new();
        for _ in 0..(1 + rng.next(4)) {
            let patch = patches[rng.next(patches.len())];
            if !named.contains(&patch) {
                named.push(patch);
            }
        }
        // Zero to two parents: blocks other claims are about, or blocks nothing is about.
        let parents: Vec<ObjectId> = (0..rng.next(3))
            .map(|_| blocks[rng.next(blocks.len())])
            .collect();
        // Several claims about one block: the block is drawn from a smaller set than the claims.
        let block = blocks[rng.next(blocks.len() - 6)];
        let claim_id = write_claim_with_parents(&mut objects, signer, block, named, parents)?;
        written.push(claim_id);
        // Some claims are also held as a second record (a superseding record: format 7).
        if rng.next(5) == 0 {
            let mut again = crate::ObjectReader::read_object(&objects, claim_id)?.expect("written");
            again.add_signature(maintainer_signature(
                &maintainer_signer(0x77)?,
                ObjectType::RecognitionClaim,
                claim_id,
            )?)?;
            crate::foundation::index::append_object_to_container(
                layout,
                ObjectType::RecognitionClaim,
                &again,
            )?;
        }
    }
    Ok(written)
}

fn refusal(result: Result<()>) -> String {
    match result {
        Ok(()) => "ok".to_string(),
        Err(error) => error.to_string(),
    }
}

#[test]
fn the_subset_decides_as_the_full_read_does() -> Result<()> {
    // Coverage: the cases the equality is over (refusals, reached and unreached parents, claims fetched by offset, duplicate records).
    let (mut refused, mut reached, mut unreached, mut fetched, mut duplicates) = (0, 0, 0, 0, 0);
    for seed in 0..120_u64 {
        let fixture = base_fixture(&format!("p5a3-diff-{seed}"))?;
        let layout = &fixture.layout;
        let written = generated_claims(seed, layout, &fixture.signer)?;
        let mut rng = Lcg(seed ^ 0x9e37);
        // The batch: a random few of the claims, in a random order.
        let mut batch: Vec<ObjectId> = Vec::new();
        for _ in 0..(1 + rng.next(6)) {
            let claim = written[rng.next(written.len())];
            if !batch.contains(&claim) {
                batch.push(claim);
            }
        }
        let snapshot = ObjectReadSnapshot::open(layout)?;
        let scope = BatchScope::of(&batch);
        let subset = StoredClaims::read(layout, Some(&scope))?;

        // The oracle: every stored claim, indexed as the batch always indexed them.
        let all = enumerate_stored_claims(layout)?;
        let records = std::fs::read(layout.container_slot_path(
            ObjectType::RecognitionClaim,
            crate::foundation::layout::ContainerSlot::A,
        ))?;
        duplicates += crate::foundation::container::decode_container_records(
            ObjectType::RecognitionClaim,
            &records,
        )?
        .records
        .len()
        .saturating_sub(all.len());
        let index = ClaimsIndex::build(&all);
        let by_block_full = claims_by_block(&all);
        let by_block_subset = ClaimsByBlock::subset(&subset, layout);

        // The order-ambiguity check: every batch claim, and every other stored claim (one not in the batch is checked against the whole container).
        for (claim_id, claim) in &all {
            let full = refusal(refuse_if_order_ambiguous_indexed(
                &all,
                &index.by_patch,
                *claim_id,
                claim,
            ));
            let sub = refusal(check_order(layout, &subset, *claim_id, claim));
            refused += usize::from(full != "ok");
            assert_eq!(
                sub, full,
                "seed {seed}: the order check of claim {claim_id}"
            );
        }

        // The parent check, against a random history, for every parent of every stored claim.
        let history = RefHistory {
            blocks: (0..30)
                .filter(|_| rng.next(4) == 0)
                .map(|n| id("block", n))
                .collect(),
            patches: (0..6)
                .filter(|_| rng.next(2) == 0)
                .map(|n| id("patch", n))
                .collect::<BTreeSet<_>>(),
        };
        for (_, claim) in &all {
            for parent in &claim.parent_block_ids {
                let full = parent_reached(&snapshot, &by_block_full, &history, *parent)
                    .map_err(|e| e.to_string());
                let sub = parent_reached(&snapshot, &by_block_subset, &history, *parent)
                    .map_err(|e| e.to_string());
                assert_eq!(sub, full, "seed {seed}: parent {parent} reached");
                reached += usize::from(full == Ok(true));
                unreached += usize::from(full == Ok(false));
            }
        }

        // The claim a block resolves to, and its payload: the first claim for it, whatever the subset holds.
        for block_n in 0..30 {
            let block = id("block", block_n);
            let full = by_block_full_get(&all, &block);
            let sub = subset.claim_for_block(layout, &block)?;
            fetched += usize::from(
                sub.as_ref()
                    .is_some_and(|(claim_id, _)| subset.kept_claim(claim_id).is_none()),
            );
            assert_eq!(
                sub.map(|(claim_id, claim)| (claim_id, (*claim).clone())),
                full,
                "seed {seed}: the claim about block {block_n}"
            );
        }
        cleanup(layout);
    }
    assert!(
        refused >= 30 && reached >= 100 && unreached >= 100 && fetched >= 200 && duplicates >= 20,
        "coverage: {refused} refusals, {reached} reached, {unreached} unreached, {fetched} blocks resolved outside the subset, {duplicates} duplicate-id writes"
    );
    Ok(())
}

/// The first claim (in stored order, one per id) about `block`, as the full read resolves it.
fn by_block_full_get(
    all: &[(ObjectId, RecognitionClaimPayload)],
    block: &ObjectId,
) -> Option<(ObjectId, RecognitionClaimPayload)> {
    all.iter()
        .find(|(_, claim)| claim.block_id == *block)
        .map(|(claim_id, claim)| (*claim_id, claim.clone()))
}

/// What the subset keeps: only the claims that name a patch of the batch (and the batch's own), however many the container holds.
#[test]
fn a_subset_keeps_the_claims_that_name_a_batch_patch_and_nothing_else() -> Result<()> {
    let fixture = base_fixture("p5a3-keeps")?;
    let layout = &fixture.layout;
    let mut objects = FileObjectStore::new(layout.clone());
    let mut written = Vec::new();
    for n in 0..200_usize {
        written.push(write_claim_with_parents(
            &mut objects,
            &fixture.signer,
            id("block", n),
            vec![id("patch", n)],
            Vec::new(),
        )?);
    }
    let batch: Vec<ObjectId> = written[100..105].to_vec();
    let subset = StoredClaims::read(layout, Some(&BatchScope::of(&batch)))?;
    assert_eq!(subset.kept().len(), 5, "the batch's five claims");
    assert_eq!(
        subset.by_block.len(),
        201, // the base fixture holds one claim of its own beside the 200 written here
        "every claim is indexed by its block"
    );
    // An unscoped read keeps every claim.
    assert_eq!(StoredClaims::read(layout, None)?.kept().len(), 201);
    cleanup(layout);
    Ok(())
}

/// A claim held as several records (format 7: a superseding record adds a signature) is **one** claim, and **the first record of its id is the one
/// kept**: the offset the block resolves to is the first record's, as the full read's first position was.
#[test]
fn a_claim_held_as_several_records_is_kept_at_its_first_record() -> Result<()> {
    let fixture = base_fixture("p5a3-first-record")?;
    let layout = &fixture.layout;
    let mut objects = FileObjectStore::new(layout.clone());
    let block = id("block", 7);
    let claim_id = write_claim_with_parents(
        &mut objects,
        &fixture.signer,
        block,
        vec![id("patch", 7)],
        Vec::new(),
    )?;
    for seed in [0x71_u8, 0x72] {
        let mut again = crate::ObjectReader::read_object(&objects, claim_id)?.expect("written");
        again.add_signature(maintainer_signature(
            &maintainer_signer(seed)?,
            ObjectType::RecognitionClaim,
            claim_id,
        )?)?;
        crate::foundation::index::append_object_to_container(
            layout,
            ObjectType::RecognitionClaim,
            &again,
        )?;
    }
    let bytes = std::fs::read(layout.container_slot_path(
        ObjectType::RecognitionClaim,
        crate::foundation::layout::ContainerSlot::A,
    ))?;
    let replay = crate::foundation::container::decode_container_records(
        ObjectType::RecognitionClaim,
        &bytes,
    )?;
    let own: Vec<usize> = replay
        .records
        .iter()
        .zip(&replay.record_outcomes)
        .filter(|(record, _)| record.envelope.object_id() == claim_id)
        .map(|(_, outcome)| outcome.offset)
        .collect();
    assert_eq!(own.len(), 3, "fixture sanity: three records of one id");
    let subset = StoredClaims::read(layout, None)?;
    assert_eq!(
        subset.block_claim(&block).expect("indexed").offset,
        own[0] as u64,
        "the first record is the one kept"
    );
    assert_eq!(
        subset
            .kept()
            .iter()
            .filter(|(id, _)| *id == claim_id)
            .count(),
        1,
        "one claim"
    );
    cleanup(layout);
    Ok(())
}

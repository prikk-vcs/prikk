//! 0.58.0 F2: the one decision (`decide`): the lead, then the holder, then the lead again.
//!
//! Each row writes the ref's lock body the way a publisher would and asks. A row for each answer a holder can give: none, running, gone, another
//! pid namespace (unknown), an incomplete body; and the race, where the publication finishes between the two reads.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use prikk_error::Result;
use prikk_object::ObjectId;

use super::{Lead, decide};
use crate::RepositoryLayout;
use crate::lock_holder::{own_facts, render_body};
use crate::test_gates::test_support::unique_temp_dir;

const REF: &str = "tags/t1";

/// Two scripted leads: the RefState ids a pointer names (the lead's identity).
fn lead_a() -> ObjectId {
    ObjectId::from_bytes([0xA1; 32])
}

fn lead_b() -> ObjectId {
    ObjectId::from_bytes([0xB2; 32])
}

fn layout(name: &str) -> (RepositoryLayout, std::path::PathBuf) {
    let root = unique_temp_dir(name);
    (RepositoryLayout::init(root.clone()).expect("init"), root)
}

fn lock(layout: &RepositoryLayout, body: &str) {
    std::fs::write(layout.ref_lock_path(REF), body).expect("write the lock");
}

/// The pid of a process that has exited (and was reaped): a holder that does not appear to be running.
fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("true").spawn().expect("spawn");
    let pid = child.id();
    child.wait().expect("wait");
    pid
}

#[test]
fn no_lock_is_an_interrupted_publication() -> Result<()> {
    let (layout, root) = layout("f2-no-lock");
    assert_eq!(
        decide(&layout, REF, lead_a(), || Ok(Some(lead_a())))?,
        Lead::Interrupted { holder_note: None }
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn a_running_holder_is_a_publication_in_flight() -> Result<()> {
    let (layout, root) = layout("f2-running");
    lock(
        &layout,
        &render_body("ref", std::process::id(), own_facts()),
    );
    assert_eq!(
        decide(&layout, REF, lead_a(), || Ok(Some(lead_a())))?,
        Lead::InFlight {
            pid: Some(std::process::id())
        }
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The race: the holder is read while the lock is there, and the publication finishes before the second read of the lead. No lead is no
/// lead: neither in flight nor interrupted. Also the order the other way: the lock is already gone at the holder read and the lead is gone at the second.
#[test]
fn a_publication_that_finishes_between_the_two_reads_is_no_lead() -> Result<()> {
    let (layout, root) = layout("f2-race");
    lock(
        &layout,
        &render_body("ref", std::process::id(), own_facts()),
    );
    assert_eq!(decide(&layout, REF, lead_a(), || Ok(None))?, Lead::Gone);
    std::fs::remove_file(layout.ref_lock_path(REF))?;
    assert_eq!(decide(&layout, REF, lead_a(), || Ok(None))?, Lead::Gone);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn a_holder_that_does_not_appear_to_be_running_is_interrupted_and_says_so() -> Result<()> {
    let (layout, root) = layout("f2-dead");
    lock(&layout, &render_body("ref", dead_pid(), own_facts()));
    let Lead::Interrupted {
        holder_note: Some(note),
    } = decide(&layout, REF, lead_a(), || Ok(Some(lead_a())))?
    else {
        panic!("a dead holder is an interrupted publication");
    };
    assert!(note.contains("does not appear to be running"), "{note}");
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Unknown is not in flight (step 0a's rule: only a definite answer changes behaviour): an incomplete body, an older prikk's body, and -- on Linux --
/// another pid namespace.
#[test]
fn an_unknown_holder_is_interrupted_and_says_why() -> Result<()> {
    let (layout, root) = layout("f2-unknown");
    let mut bodies = vec![
        (String::new(), "incomplete"),
        (
            format!("pid={}\nkind=ref\n", std::process::id()),
            "older prikk",
        ),
    ];
    if cfg!(target_os = "linux") {
        let body = render_body("ref", std::process::id(), own_facts())
            .lines()
            .map(|line| {
                if line.starts_with("pidns=") {
                    "pidns=pid:[1]".to_string()
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        bodies.push((body, "another pid or time namespace"));
    }
    for (body, words) in bodies {
        lock(&layout, &body);
        let Lead::Interrupted {
            holder_note: Some(note),
        } = decide(&layout, REF, lead_a(), || Ok(Some(lead_a())))?
        else {
            panic!("an unknown holder ({words}) is not in flight");
        };
        assert!(note.contains("unknown"), "{words}: {note}");
    }
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RS2c: **a lead is identified by its RefState id.** The same lead twice with no holder at either read is interrupted; a lead that is a different one on
/// the second read is in flight (a `sync seal` publishes one claim per publication, back to back, and a holder read can land in the gap between two);
/// the same lead whose holder appears only at the later holder read is in flight. Each arm was made red by reverting it (`perturb.sh`).
#[test]
fn a_lead_is_judged_by_its_identity() -> Result<()> {
    let (layout, root, _) = {
        let (layout, root) = layout("rs2c-identity");
        (layout, root, ())
    };
    // A, A, no holder at either read: interrupted (a lead that existed at two moments with no lock between them is not a running publication).
    assert_eq!(
        decide(&layout, REF, lead_a(), || Ok(Some(lead_a())))?,
        Lead::Interrupted { holder_note: None }
    );
    // A, then B, no holder at the first read: in flight, no pid (the holder read made before B existed cannot speak for it).
    assert_eq!(
        decide(&layout, REF, lead_a(), || Ok(Some(lead_b())))?,
        Lead::InFlight { pid: None }
    );
    // A, then B, a running holder at the first read: in flight with its pid.
    lock(
        &layout,
        &render_body("ref", std::process::id(), own_facts()),
    );
    assert_eq!(
        decide(&layout, REF, lead_a(), || Ok(Some(lead_b())))?,
        Lead::InFlight {
            pid: Some(std::process::id())
        }
    );
    std::fs::remove_file(layout.ref_lock_path(REF))?;
    // A, A, and the holder appears **between the second lead read and the second holder read**: in flight.
    let appearing = layout.clone();
    assert_eq!(
        decide(&layout, REF, lead_a(), || {
            lock(
                &appearing,
                &render_body("ref", std::process::id(), own_facts()),
            );
            Ok(Some(lead_a()))
        })?,
        Lead::InFlight {
            pid: Some(std::process::id())
        }
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

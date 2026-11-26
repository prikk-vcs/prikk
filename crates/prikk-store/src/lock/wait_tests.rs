//! 0.59.0 P5b3b: the bounded wait on a holder that is running, its rank rule and its held record.
//!
//! Every test here sets its own bound (`with_lock_wait`); under `cfg(test)` the bound is otherwise off, so the unit tests of every other module,
//! which hold locks and expect to be refused at once, keep doing so. The holder is a real `sleep` process whose lock body names it exactly as
//! `render_body` writes it, so the real judge says "appears running".

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use super::test_hooks::with_incomplete_grace;
use super::{
    DEFAULT_LOCK_WAIT_SECONDS, MAX_LOCK_WAIT_SECONDS, Rank, held_ranks_of_this_thread,
    parse_lock_wait, test_hooks, with_lock_wait,
};
use crate::foundation::layout::LockableContainer;
use crate::lock_holder::{body_naming_another_running_process, own_facts, render_body};
use crate::test_gates::test_support::unique_temp_dir;
use crate::{ActiveLock, DEFAULT_ACTIVE_NAME, RefLock, RepositoryLayout, acquire_container_locks};

fn repository(name: &str) -> (RepositoryLayout, PathBuf) {
    let root = unique_temp_dir(name);
    let layout = RepositoryLayout::init(root.clone()).expect("init");
    (layout, root)
}

fn active_path(layout: &RepositoryLayout) -> PathBuf {
    layout.active_lock_path(DEFAULT_ACTIVE_NAME)
}

fn put(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    fs::write(path, body).expect("write the lock");
}

fn stop(mut child: std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn conflict_text<T>(result: prikk_error::Result<T>) -> String {
    match result {
        Err(prikk_error::PrikkError::LockConflict(text)) => text,
        Err(other) => panic!("a lock conflict was expected, got {other:?}"),
        Ok(_) => panic!("a lock conflict was expected, the lock was taken"),
    }
}

// ---------------------------------------------------------------------------------------------------------------------------------------------
// The bound, and who sets it.

/// **The bound is 5 seconds a stop; `PRIKK_LOCK_WAIT_SECONDS` sets it; `0` is off; the ceiling holds; a value that is not a whole number of seconds
/// is the default.** Perturb: the default 5 changed, the ceiling dropped, or `0` read as the default: each is red.
#[test]
fn the_bound_is_five_seconds_zero_is_off_and_the_ceiling_holds() {
    assert_eq!(DEFAULT_LOCK_WAIT_SECONDS, 5);
    assert_eq!(parse_lock_wait(None), Duration::from_secs(5));
    assert_eq!(parse_lock_wait(Some("0")), Duration::ZERO);
    assert_eq!(parse_lock_wait(Some("1")), Duration::from_secs(1));
    assert_eq!(parse_lock_wait(Some(" 12 ")), Duration::from_secs(12));
    assert_eq!(
        parse_lock_wait(Some("300")),
        Duration::from_secs(MAX_LOCK_WAIT_SECONDS)
    );
    assert_eq!(
        parse_lock_wait(Some("301")),
        Duration::from_secs(MAX_LOCK_WAIT_SECONDS)
    );
    assert_eq!(
        parse_lock_wait(Some("99999999999999999999")),
        Duration::from_secs(5),
        "a number too large for the type is not a number of seconds"
    );
    for unreadable in ["", "five", "-1", "1.5", "1s"] {
        assert_eq!(
            parse_lock_wait(Some(unreadable)),
            Duration::from_secs(5),
            "{unreadable:?} is the default"
        );
    }
    assert_eq!(MAX_LOCK_WAIT_SECONDS, 300);
}

#[test]
fn seconds_are_written_whole_or_in_tenths() {
    use super::seconds;
    assert_eq!(seconds(Duration::from_secs(5)), "5 s");
    assert_eq!(seconds(Duration::from_millis(500)), "0.5 s");
    assert_eq!(seconds(Duration::from_millis(1_340)), "1.3 s");
    assert_eq!(seconds(Duration::from_millis(4_990)), "5 s");
}

// ---------------------------------------------------------------------------------------------------------------------------------------------
// §4's case table, on the build: the holder finishes, dies, never ends.

/// **The holder finishes:** the waiter goes through when the holder's lock is removed, in about the time the holder took, not the bound.
/// Perturb: no retry after the name is found taken (the waiter is refused at once): red.
#[test]
fn a_holder_that_finishes_is_waited_out() {
    let (layout, root) = repository("p5b3b-finishes");
    let path = active_path(&layout);
    let (child, body) = body_naming_another_running_process("active");
    put(&path, &body);
    let releaser = {
        let path = path.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            fs::remove_file(path).expect("the holder releases");
        })
    };
    let started = Instant::now();
    let lock = with_lock_wait(Duration::from_secs(5), || {
        ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)
    });
    let waited = started.elapsed();
    releaser.join().unwrap();
    stop(child);
    assert!(
        lock.is_ok(),
        "the lock was taken once the holder finished: {:?}",
        lock.err()
    );
    assert!(
        waited >= Duration::from_millis(250),
        "it waited: {waited:?}"
    );
    assert!(
        waited < Duration::from_secs(2),
        "not to the bound: {waited:?}"
    );
    let _ = fs::remove_dir_all(root);
}

/// **The holder dies (its lock stays):** refused at once, within a poll of the kill, with today's sentence and the words for the wait.
/// Perturb: the judge's answer ignored (every taken name waited for): the waiter runs to the bound and this is red.
#[test]
fn a_holder_that_dies_is_refused_when_it_does() {
    let (layout, root) = repository("p5b3b-dies");
    let path = active_path(&layout);
    let (child, body) = body_naming_another_running_process("active");
    let pid = child.id();
    put(&path, &body);
    let killer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        stop(child);
    });
    let started = Instant::now();
    let refused = with_lock_wait(Duration::from_secs(5), || {
        ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)
    });
    let waited = started.elapsed();
    killer.join().unwrap();
    let text = conflict_text(refused);
    assert!(
        waited >= Duration::from_millis(250),
        "it waited for the holder: {waited:?}"
    );
    assert!(
        waited < Duration::from_secs(2),
        "and gave up when the holder died: {waited:?}"
    );
    assert!(
        text.starts_with("active lock already exists: "),
        "today's sentence first: {text}"
    );
    assert!(
        text.contains("run `prikk unlock` to list held locks"),
        "{text}"
    );
    assert!(
        text.contains("(waited 0.")
            && text.ends_with(&format!("pid {pid}: no process has the recorded pid)")),
        "{text}"
    );
    assert!(
        path.is_file(),
        "the dead holder's lock is left for `unlock`, as before"
    );
    let _ = fs::remove_dir_all(root);
}

/// **The holder never ends:** refused at the bound, with the words for the wait. Perturb: the bound not read (waits for ever): the test never
/// ends, so it asserts on the time it took; or the bound doubled: red.
#[test]
fn a_holder_that_never_ends_is_refused_at_the_bound() {
    let (layout, root) = repository("p5b3b-never");
    let path = active_path(&layout);
    let (child, body) = body_naming_another_running_process("active");
    let pid = child.id();
    put(&path, &body);
    let started = Instant::now();
    let refused = with_lock_wait(Duration::from_millis(500), || {
        ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)
    });
    let waited = started.elapsed();
    stop(child);
    let text = conflict_text(refused);
    assert!(
        waited >= Duration::from_millis(500),
        "to the bound: {waited:?}"
    );
    assert!(
        waited < Duration::from_millis(1_500),
        "and not past it: {waited:?}"
    );
    assert!(
        text.ends_with(&format!(
            "(waited 0.5 s: pid {pid} still appears to be running)"
        )),
        "{text}"
    );
    let _ = fs::remove_dir_all(root);
}

/// **A bound of zero is today's behaviour:** refused at once, today's sentence, no words added. Perturb: the zero ignored: red.
#[test]
fn a_bound_of_zero_refuses_at_once_with_no_added_words() {
    let (layout, root) = repository("p5b3b-zero");
    let (child, body) = body_naming_another_running_process("active");
    put(&active_path(&layout), &body);
    let started = Instant::now();
    let refused = with_lock_wait(Duration::ZERO, || {
        ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)
    });
    let waited = started.elapsed();
    stop(child);
    let text = conflict_text(refused);
    assert!(waited < Duration::from_millis(200), "{waited:?}");
    assert!(text.ends_with("is confirmed gone"), "no suffix: {text}");
    let _ = fs::remove_dir_all(root);
}

// ---------------------------------------------------------------------------------------------------------------------------------------------
// What is never waited on: a holder that does not appear to be running, one that cannot be judged, a body that stays incomplete, this process.

fn refused_at_once_without_added_words(layout: &RepositoryLayout, what: &str) {
    let started = Instant::now();
    let refused = with_lock_wait(Duration::from_secs(5), || {
        ActiveLock::acquire(layout, DEFAULT_ACTIVE_NAME)
    });
    let waited = started.elapsed();
    let text = conflict_text(refused);
    assert!(
        waited < Duration::from_millis(500),
        "{what}: refused at once, took {waited:?}"
    );
    assert!(
        text.ends_with("is confirmed gone"),
        "{what}: today's sentence, nothing added: {text}"
    );
}

/// **A holder that does not appear to be running is never waited on** (its pid is gone: a command that was killed and left its lock).
/// Perturb: the judge's negative answers waited for as well: red.
#[test]
fn a_stale_holder_is_never_waited_on() {
    let (layout, root) = repository("p5b3b-stale");
    // A holder that was running, with its own start time, and is gone: the lock a killed command leaves.
    let (child, body) = body_naming_another_running_process("active");
    stop(child);
    put(&active_path(&layout), &body);
    refused_at_once_without_added_words(&layout, "a pid that is gone");
    let _ = fs::remove_dir_all(root);
}

/// **A holder that cannot be judged is never waited on:** another pid namespace, an older prikk's body, another machine's. For the older body the
/// pid is a **running** one and it is still not waited on: the judge cannot say it is the process that took the lock. Perturb: "unknown" waited
/// for: red.
#[test]
fn an_unknown_holder_is_never_waited_on() {
    let (layout, root) = repository("p5b3b-unknown");
    let path = active_path(&layout);
    let (other, _) = body_naming_another_running_process("active");
    // Another pid namespace.
    let mut facts = own_facts().clone();
    facts.pidns = Some("pid:[4026539999]".to_string());
    put(&path, &render_body("active", other.id(), &facts));
    refused_at_once_without_added_words(&layout, "another namespace");
    // An older prikk's body: a running pid, and no start time to check it against.
    put(
        &path,
        &format!(
            "pid={}\nkind=active\nnote=PR-007 lock has no stale-lock stealing yet\n",
            other.id()
        ),
    );
    refused_at_once_without_added_words(&layout, "an older prikk's body");
    // Another machine's.
    let mut facts = own_facts().clone();
    facts.machine = Some("some-other-machine".to_string());
    put(&path, &render_body("active", other.id(), &facts));
    refused_at_once_without_added_words(&layout, "another machine");
    stop(other);
    let _ = fs::remove_dir_all(root);
}

/// Wait until `looks` has counted more than `seen` looks at an incomplete body (the waiter has looked at the empty file again), or fail after a
/// minute. **Nothing here is a deadline the thread under test must meet:** a writer waits for the waiter's look, not for the clock.
fn wait_for_a_look_after(looks: &AtomicUsize, seen: usize) {
    let patience = Instant::now();
    while looks.load(Ordering::SeqCst) <= seen {
        assert!(
            patience.elapsed() < Duration::from_secs(60),
            "the waiter never looked at the body"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// **The production grace is 250 ms** (P5b3c; 30 ms in P5b3b flaked on a loaded macOS runner). Perturb: another number: red.
#[test]
fn the_grace_a_body_still_being_written_is_given_is_250_ms() {
    assert_eq!(super::INCOMPLETE_GRACE, Duration::from_millis(250));
}

/// **A body that stays incomplete is never waited on past the grace:** an empty file (a command killed between its create and its body) is given
/// the grace, in case a live creator is about to write it, and is then refused with today's sentence. The grace is set to 50 ms here and the test
/// asserts only lower bounds on what the grace gave and a generous upper one. Perturb: an incomplete body waited for far longer: red; no grace:
/// red (the lower bound).
#[test]
fn an_incomplete_holder_is_refused_after_the_grace() {
    let (layout, root) = repository("p5b3b-incomplete");
    put(&active_path(&layout), "");
    let looks = Arc::new(AtomicUsize::new(0));
    let started = Instant::now();
    let refused = with_incomplete_grace(Duration::from_millis(50), looks.clone(), || {
        with_lock_wait(Duration::from_secs(5), || {
            ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)
        })
    });
    let waited = started.elapsed();
    let trace = test_hooks::trace();
    let text = conflict_text(refused);
    assert!(
        waited >= Duration::from_millis(50),
        "given the grace first: {waited:?}; {trace}"
    );
    assert!(
        waited < Duration::from_secs(5),
        "and not the bound: {waited:?}; {trace}"
    );
    assert!(
        looks.load(Ordering::SeqCst) >= 2,
        "it looked more than once: {trace}"
    );
    assert!(
        text.ends_with("is confirmed gone"),
        "today's sentence, nothing added: {text}"
    );
    let _ = fs::remove_dir_all(root);
}

/// **A live creator's body is read after the grace's first moment:** the lock file is there, empty, and its creator writes the body **after the
/// waiter has looked at the empty file** (the writer waits for that look, not for the clock; the grace is set to ten seconds so no thread has a
/// deadline); the waiter does not refuse at its first look, and takes the lock when the creator lets go. Perturb: the grace removed: red.
#[test]
fn a_body_still_being_written_is_read_again() {
    let (layout, root) = repository("p5b3b-being-written");
    let path = active_path(&layout);
    let (child, body) = body_naming_another_running_process("active");
    put(&path, "");
    let looks = Arc::new(AtomicUsize::new(0));
    let writer = {
        let (path, looks) = (path.clone(), looks.clone());
        std::thread::spawn(move || {
            wait_for_a_look_after(&looks, 0);
            fs::write(&path, body).expect("the body");
            std::thread::sleep(Duration::from_millis(200));
            fs::remove_file(&path).expect("released");
        })
    };
    let lock = with_incomplete_grace(Duration::from_secs(10), looks, || {
        let lock = with_lock_wait(Duration::from_secs(20), || {
            ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)
        });
        (lock, test_hooks::trace())
    });
    writer.join().unwrap();
    stop(child);
    assert!(
        lock.0.is_ok(),
        "waited for the creator, then took the lock: {:?}; what it saw: {}",
        lock.0.err(),
        lock.1
    );
    let _ = fs::remove_dir_all(root);
}

/// **Each holder's moment of an incomplete body gets its own grace:** the waiter sees one holder's body being written, then (after more than the
/// grace has passed) the next holder's: the lock released and created again. The second is not judged against the first's clock. The grace is two
/// seconds; the writer lets **2.3 s** pass (a lower bound: sleeping longer is harmless) between the episodes and writes each body after the
/// waiter's look at the empty file. Perturb: the grace's clock not reset when a complete body is read: the waiter refuses at the second
/// holder's empty body, red.
#[test]
fn a_second_holders_moment_of_an_incomplete_body_gets_its_own_grace() {
    let (layout, root) = repository("p5b3b-two-incomplete-moments");
    let path = active_path(&layout);
    let (child, body) = body_naming_another_running_process("active");
    put(&path, "");
    let looks = Arc::new(AtomicUsize::new(0));
    let holders = {
        let (path, looks) = (path.clone(), looks.clone());
        std::thread::spawn(move || {
            wait_for_a_look_after(&looks, 0);
            fs::write(&path, &body).expect("the first holder's body");
            let after_the_first = looks.load(Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(2_300));
            fs::write(&path, "").expect("the second holder creates");
            wait_for_a_look_after(&looks, after_the_first);
            fs::write(&path, &body).expect("the second holder's body");
            std::thread::sleep(Duration::from_millis(200));
            fs::remove_file(&path).expect("released");
        })
    };
    let lock = with_incomplete_grace(Duration::from_secs(2), looks, || {
        let lock = with_lock_wait(Duration::from_secs(30), || {
            ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)
        });
        (lock, test_hooks::trace())
    });
    holders.join().unwrap();
    stop(child);
    assert!(
        lock.0.is_ok(),
        "{:?}; what it saw: {}",
        lock.0.err(),
        lock.1
    );
    let _ = fs::remove_dir_all(root);
}

/// **A lock this process holds is never waited on:** a second acquisition of the active lock by the thread that holds it is refused at once, as the
/// Part I-A bug was found (the rank rule refuses it too: a lock of the rank already held is not waited for, so this test is guarded twice; the
/// next test is the record's alone).
#[test]
fn a_lock_this_process_holds_is_never_waited_on() {
    let (layout, root) = repository("p5b3b-own");
    let _held = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).expect("hold it");
    refused_at_once_without_added_words(&layout, "this process's own active lock");
    let reference = RefLock::acquire(&layout, "heads/main").expect("a ref lock");
    let started = Instant::now();
    let refused = with_lock_wait(Duration::from_secs(5), || {
        RefLock::acquire(&layout, "heads/main")
    });
    assert!(started.elapsed() < Duration::from_millis(500));
    conflict_text(refused);
    drop(reference);
    let _ = fs::remove_dir_all(root);
}

/// **A lock another thread of this process holds is never waited on** (the held record, and the pid in the body): the waiter holds nothing, so the
/// rank rule lets it wait, and it would wait for a thread of its own process. Perturb: the record not consulted **and** the pid not compared: the
/// waiter stalls to the bound, red. (With only one of the two removed this stays green: they are two guards of one rule; the next test is the pid's
/// alone.)
#[test]
fn a_lock_another_thread_of_this_process_holds_is_never_waited_on() {
    let (layout, root) = repository("p5b3b-own-thread");
    let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let holder = {
        let layout = layout.clone();
        std::thread::spawn(move || {
            let _lock = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).expect("hold it");
            held_tx.send(()).unwrap();
            let _ = release_rx.recv();
        })
    };
    held_rx.recv().unwrap();
    refused_at_once_without_added_words(
        &layout,
        "an active lock another thread of this process holds",
    );
    release_tx.send(()).unwrap();
    holder.join().unwrap();
    let _ = fs::remove_dir_all(root);
}

/// **A body that names this very process is never waited on, even with no entry in the record** (a file written between a create and the
/// registration, or left by this pid): the pid is compared too. Perturb: the pid comparison removed: red.
#[test]
fn a_body_naming_this_process_is_never_waited_on() {
    let (layout, root) = repository("p5b3b-own-pid");
    put(
        &active_path(&layout),
        &render_body("active", std::process::id(), own_facts()),
    );
    refused_at_once_without_added_words(&layout, "a body with this process's pid");
    let _ = fs::remove_dir_all(root);
}

// ---------------------------------------------------------------------------------------------------------------------------------------------
// The held record, and the rank rule that reads only it.

/// **Every lock kind registers in the record and unregisters when its guard drops.** The kinds are enumerated: the active lock, a ref lock, and
/// every `LockableContainer` (`ALL`, so a container added without registering fails here). Perturb: any one kind not registered: red.
#[test]
fn every_lock_kind_registers_and_unregisters() {
    let (layout, root) = repository("p5b3b-record");
    assert!(held_ranks_of_this_thread().is_empty());
    {
        let _active = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).expect("active");
        assert_eq!(held_ranks_of_this_thread(), vec![Rank::Active]);
    }
    assert!(held_ranks_of_this_thread().is_empty(), "released");
    {
        let _reference = RefLock::acquire(&layout, "heads/main").expect("ref");
        assert_eq!(held_ranks_of_this_thread(), vec![Rank::Ref]);
    }
    assert!(held_ranks_of_this_thread().is_empty(), "released");
    for container in LockableContainer::ALL {
        {
            let _guard = acquire_container_locks(&layout, &[container]).expect("container");
            assert_eq!(
                held_ranks_of_this_thread(),
                vec![Rank::Container(container)],
                "{container:?} registers"
            );
        }
        assert!(
            held_ranks_of_this_thread().is_empty(),
            "{container:?} unregisters"
        );
    }
    {
        let _all =
            acquire_container_locks(&layout, &LockableContainer::ALL).expect("every container");
        let want: Vec<Rank> = LockableContainer::ALL
            .into_iter()
            .map(Rank::Container)
            .collect();
        assert_eq!(held_ranks_of_this_thread(), want);
    }
    // A refused acquisition registers nothing, and a partial failure leaves nothing behind.
    let held = acquire_container_locks(&layout, &[LockableContainer::ObjectStore]).expect("hold");
    assert!(
        acquire_container_locks(
            &layout,
            &[LockableContainer::RefLog, LockableContainer::ObjectStore]
        )
        .is_err()
    );
    assert_eq!(
        held_ranks_of_this_thread(),
        vec![Rank::Container(LockableContainer::ObjectStore)]
    );
    drop(held);
    assert!(held_ranks_of_this_thread().is_empty());
    let _ = fs::remove_dir_all(root);
}

/// The rank is the declared order and nothing else: active, ref, then the containers as `LockableContainer` orders them. Perturb: the variants
/// reordered: red.
#[test]
fn the_rank_is_the_one_fixed_order() {
    let mut order = vec![Rank::Active, Rank::Ref];
    order.extend(LockableContainer::ALL.into_iter().map(Rank::Container));
    let mut sorted = order.clone();
    sorted.sort_unstable();
    assert_eq!(order, sorted);
    assert!(Rank::Active < Rank::Ref);
    assert!(Rank::Ref < Rank::Container(LockableContainer::RefPointerIndex));
    assert!(
        Rank::Container(LockableContainer::TrustPolicy)
            < Rank::Container(LockableContainer::ObjectStore)
    );
}

/// **A lock that does not rank after everything held is refused at once, though its holder is running** (the rule's first half): this thread holds the
/// object store (the leaf), and the active lock is held by a running process; waiting for the active lock would be waiting up the order's wrong
/// way. Perturb: the rank check removed: the waiter waits to the bound, red.
#[test]
fn a_lock_ranking_before_one_held_is_not_waited_for() {
    let (layout, root) = repository("p5b3b-rank-down");
    let (child, body) = body_naming_another_running_process("active");
    put(&active_path(&layout), &body);
    let _leaf =
        acquire_container_locks(&layout, &[LockableContainer::ObjectStore]).expect("the leaf");
    refused_at_once_without_added_words(&layout, "the active lock while holding the leaf");
    stop(child);
    let _ = fs::remove_dir_all(root);
}

/// **A lock of the same rank is not waited for either:** two ref locks (a command holding one ref's lock and wanting another's) are never in a
/// wait-for chain by name. Perturb: `>=` read as `>`: red.
#[test]
fn a_lock_of_the_same_rank_is_not_waited_for() {
    let (layout, root) = repository("p5b3b-rank-equal");
    let (child, body) = body_naming_another_running_process("ref");
    put(&layout.ref_lock_path("tags/other"), &body);
    let _mine = RefLock::acquire(&layout, "heads/main").expect("one ref lock");
    let started = Instant::now();
    let refused = with_lock_wait(Duration::from_secs(5), || {
        RefLock::acquire(&layout, "tags/other")
    });
    assert!(started.elapsed() < Duration::from_millis(500));
    assert!(conflict_text(refused).ends_with("is confirmed gone"));
    stop(child);
    let _ = fs::remove_dir_all(root);
}

/// **A lock ranking after everything held is waited for** (the rule's other half): this thread holds the active lock, the object store is held by a
/// running process that releases it, and the waiter takes it. Perturb: the wait removed for locks that rank after: red.
#[test]
fn a_lock_ranking_after_everything_held_is_waited_for() {
    let (layout, root) = repository("p5b3b-rank-up");
    let _active = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).expect("active");
    let leaf_path = layout.lockable_container_lock_path(LockableContainer::ObjectStore);
    let (child, body) = body_naming_another_running_process("container:object-store");
    put(&leaf_path, &body);
    let releaser = {
        let path = leaf_path.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            fs::remove_file(path).expect("released");
        })
    };
    let started = Instant::now();
    let guard = with_lock_wait(Duration::from_secs(5), || {
        acquire_container_locks(&layout, &[LockableContainer::ObjectStore])
    });
    let waited = started.elapsed();
    releaser.join().unwrap();
    stop(child);
    assert!(guard.is_ok(), "{:?}", guard.err());
    assert!(
        waited >= Duration::from_millis(250) && waited < Duration::from_secs(2),
        "{waited:?}"
    );
    let _ = fs::remove_dir_all(root);
}

// ---------------------------------------------------------------------------------------------------------------------------------------------
// The two-process test: the rank rule breaks the active <-> leaf cycle, with archive import's own two acquisitions as one side.

const HELPER_DIR: &str = "PRIKK_P5B3B_HELPER_DIR";

/// **The import side, run as its own process** (a no-op unless the test below starts it): it takes the container locks the way `archive import`
/// does (`acquire_container_locks_allowing_import_journal`, the object store among them) and **then** the active lock
/// (`acquire_for_write_allowing_import_journal`), with the wait on. It writes how long the second took and what it said.
#[test]
fn helper_the_import_side_of_the_cycle() {
    let Some(dir) = std::env::var_os(HELPER_DIR) else {
        return;
    };
    let dir = PathBuf::from(dir);
    let layout = RepositoryLayout::open(dir.join("repo")).expect("open");
    let leaf = super::acquire_container_locks_allowing_import_journal(
        &layout,
        &[LockableContainer::ObjectStore],
    )
    .expect("the import takes the leaf first");
    fs::write(dir.join("import-holds-the-leaf"), b"1").expect("marker");
    while !dir.join("sealer-holds-the-active-lock").exists() {
        std::thread::sleep(Duration::from_millis(2));
    }
    let started = Instant::now();
    let result = with_lock_wait(Duration::from_secs(4), || {
        ActiveLock::acquire_for_write_allowing_import_journal(&layout, DEFAULT_ACTIVE_NAME)
    });
    let line = format!(
        "{} {}",
        started.elapsed().as_millis(),
        match result {
            Ok(_) => "took-the-active-lock".to_string(),
            Err(error) => format!("refused: {error}"),
        }
    );
    // Release the leaf only after the sealer is waiting for it, so the sealer's wait is what ends the cycle.
    std::thread::sleep(Duration::from_millis(200));
    drop(leaf);
    fs::write(dir.join("import-result"), line).expect("result");
}

/// **A `sync seal` (the active lock, then the leaf) against an `archive import` (the leaf, then the active lock): the rank rule ends the cycle.** The
/// import holds the leaf and wants the active lock, which ranks before what it holds, so it is refused at once; the sealer holds the active lock and
/// wants the leaf, which ranks after, so it waits, and takes the leaf when the import lets go. Without the rule both wait to the bound (4 s) and are
/// refused. Perturb: the rank check removed: the import waits to the bound, the sealer is refused, red.
#[test]
fn the_rank_rule_breaks_the_active_and_leaf_cycle_across_two_processes() {
    let dir = unique_temp_dir("p5b3b-cycle");
    let repo = dir.join("repo");
    fs::create_dir_all(&repo).expect("repo dir");
    let layout = RepositoryLayout::init(repo).expect("init");
    let exe = std::env::current_exe().expect("this test binary");
    let mut child = std::process::Command::new(exe)
        .args([
            "--exact",
            "lock::wait_tests::helper_the_import_side_of_the_cycle",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(HELPER_DIR, &dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start the import side");
    let patience = Instant::now();
    while !dir.join("import-holds-the-leaf").exists() {
        assert!(
            patience.elapsed() < Duration::from_secs(30),
            "the import side never held the leaf"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    let active = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)
        .expect("the sealer holds the active lock");
    fs::write(dir.join("sealer-holds-the-active-lock"), b"1").expect("marker");
    let started = Instant::now();
    let leaf = with_lock_wait(Duration::from_secs(4), || {
        acquire_container_locks(&layout, &[LockableContainer::ObjectStore])
    });
    let waited = started.elapsed();
    let status = child.wait().expect("the import side ends");
    drop(active);
    let result = fs::read_to_string(dir.join("import-result")).unwrap_or_default();
    assert!(status.success(), "the helper test passed: {status:?}");
    assert!(
        result.contains("refused: ") && result.contains("active lock already exists"),
        "the import's wait for the active lock was refused: {result}"
    );
    let import_ms: u128 = result
        .split(' ')
        .next()
        .unwrap_or("")
        .parse()
        .unwrap_or(u128::MAX);
    assert!(
        import_ms < 1_000,
        "refused at once, not at the bound: {result}"
    );
    assert!(
        !result.contains("(waited"),
        "no words for a wait that did not happen: {result}"
    );
    assert!(
        leaf.is_ok(),
        "the sealer took the leaf when the import let go: {:?}",
        leaf.err()
    );
    assert!(
        waited >= Duration::from_millis(150) && waited < Duration::from_secs(3),
        "the sealer waited for the leaf, not for the bound: {waited:?}"
    );
    let _ = fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------------------------------------------------------------------------
// The record is the whole truth only if nothing else makes a lock file.

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read the source directory") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// **`lock.rs` is the only caller of the lock-file create** (`create_lock_file_required`) and of the lock-file removal
/// (`remove_lock_file_best_effort`): the held record is complete only if every lock file is made and removed there. The two functions are defined
/// and re-exported in `foundation/fsutil`, and called nowhere else. Perturb: a call added in any other file: red.
#[test]
fn only_lock_rs_makes_or_removes_a_lock_file() {
    let mut files = Vec::new();
    rust_sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    let mut callers = Vec::new();
    for file in files {
        let text = fs::read_to_string(&file).expect("read");
        let relative = file
            .strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")).join("src"))
            .expect("under src")
            .to_string_lossy()
            .replace('\\', "/");
        // Definitions, re-exports and this file's own text are not calls.
        if relative.starts_with("foundation/fsutil") || relative == "lock/wait_tests.rs" {
            continue;
        }
        for name in [
            "create_lock_file_required(",
            "remove_lock_file_best_effort(",
        ] {
            if text.contains(name) {
                callers.push(format!("{relative}: {name}"));
            }
        }
    }
    callers.sort();
    assert_eq!(
        callers,
        vec![
            "lock.rs: create_lock_file_required(".to_string(),
            "lock.rs: remove_lock_file_best_effort(".to_string()
        ],
        "a lock file is made and removed in lock.rs only"
    );
}

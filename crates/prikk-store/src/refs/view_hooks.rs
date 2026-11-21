//! 0.59.0 RS2a's test seam: a closure a test registers under a name runs, once, on the registering thread, at the point in a read sequence that
//! names it -- between two reads of the ref view -- so a test can make a publication land exactly there. Compiled into no shipped build.

use std::cell::RefCell;
use std::collections::BTreeMap;

type Hook = Box<dyn FnOnce()>;

thread_local! {
    static HOOKS: RefCell<BTreeMap<&'static str, Hook>> = const { RefCell::new(BTreeMap::new()) };
}

/// Register `hook` to run once at the next [`run`] of `name` on this thread.
pub(crate) fn set(name: &'static str, hook: impl FnOnce() + 'static) {
    HOOKS.with(|hooks| {
        hooks.borrow_mut().insert(name, Box::new(hook));
    });
}

/// Run (and clear) the hook registered under `name`, if any.
pub(crate) fn run(name: &'static str) {
    let hook = HOOKS.with(|hooks| hooks.borrow_mut().remove(name));
    if let Some(hook) = hook {
        hook();
    }
}

/// Whether a hook registered under `name` has not run yet (a test asserts its hook was reached).
pub(crate) fn pending(name: &'static str) -> bool {
    HOOKS.with(|hooks| hooks.borrow().contains_key(name))
}

//! Synchronous, thread-local race injection at the actual publication boundary.
use std::cell::RefCell;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Boundary {
    TargetCas,
    Push,
}

type Hook = (Boundary, Box<dyn FnOnce()>);
thread_local! {
    static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
}

pub(super) fn run(boundary: Boundary) {
    let action = HOOK.with(|hook| {
        let mut hook = hook.borrow_mut();
        if hook
            .as_ref()
            .is_some_and(|(expected, _)| *expected == boundary)
        {
            hook.take().map(|(_, action)| action)
        } else {
            None
        }
    });
    if let Some(action) = action {
        action();
    }
}

pub(super) fn during<T>(
    boundary: Boundary,
    action: impl FnOnce() + 'static,
    operation: impl FnOnce() -> T,
) -> T {
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            HOOK.with(|hook| hook.borrow_mut().take());
        }
    }
    let _clear = Clear;
    HOOK.with(|hook| {
        assert!(
            hook.borrow_mut()
                .replace((boundary, Box::new(action)))
                .is_none()
        );
    });
    operation()
}

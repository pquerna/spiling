use std::cell::RefCell;

#[derive(Debug, Default)]
pub(in crate::planner) struct Receipt {
    pub(in crate::planner) raw_dispatch: Vec<(usize, (usize, usize))>,
}

thread_local! {
    static ARM: RefCell<Option<(bool, Receipt)>> = const { RefCell::new(None) };
}

pub(in crate::planner) fn with_arm<T>(enabled: bool, run: impl FnOnce() -> T) -> (T, Receipt) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ARM.with_borrow_mut(|arm| *arm = None);
        }
    }
    let _reset = Reset;
    ARM.with_borrow_mut(|arm| {
        assert!(arm.is_none());
        *arm = Some((enabled, Receipt::default()));
    });
    let output = run();
    (output, ARM.with_borrow_mut(|arm| arm.take().unwrap().1))
}

pub(super) fn enabled() -> bool {
    ARM.with_borrow(|arm| arm.as_ref().is_none_or(|(enabled, _)| *enabled))
}

pub(super) fn record_raw_dispatch(section_index: usize, identity: (usize, usize)) {
    ARM.with_borrow_mut(|arm| {
        if let Some((_, receipt)) = arm.as_mut() {
            receipt.raw_dispatch.push((section_index, identity));
        }
    });
}

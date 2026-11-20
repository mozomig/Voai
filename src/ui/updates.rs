//! Sparkle and its preferences are owned by the macOS main thread.
use std::cell::Cell;

thread_local! { static AVAILABLE: Cell<bool> = const { Cell::new(false) }; }

unsafe extern "C" {
    fn voai_updates_start() -> bool;
    fn voai_updates_check();
    fn voai_updates_auto() -> bool;
    fn voai_updates_set_auto(enabled: bool);
    fn voai_updates_set_busy(busy: bool);
}

pub(super) fn start() {
    AVAILABLE.set(unsafe { voai_updates_start() });
}
pub(super) fn available() -> bool {
    AVAILABLE.get()
}
pub(super) fn check() {
    unsafe { voai_updates_check() }
}
pub(super) fn automatic() -> bool {
    unsafe { voai_updates_auto() }
}
pub(super) fn set_automatic(enabled: bool) {
    unsafe { voai_updates_set_auto(enabled) }
}
pub(super) fn set_busy(busy: bool) {
    unsafe { voai_updates_set_busy(busy) }
}

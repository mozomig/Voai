//! AppKit can finish restoring windows after GPUI's launch callback. Opening a
//! key window during that ordering pass can reenter GPUI's locked focus handler.
use block2::RcBlock;
use objc2_app_kit::NSApplicationDidFinishRestoringWindowsNotification;
use objc2_foundation::{NSNotification, NSNotificationCenter};
use std::ptr::NonNull;

/// Subscribe before Application::run so the no-saved-windows notification is
/// also observed. Only open the window on the foreground executor after delivery.
pub(super) fn windows_restored() -> impl Future<Output = ()> {
    let center = NSNotificationCenter::defaultCenter();
    let (tx, rx) = async_channel::bounded(1);
    let callback = RcBlock::new(move |_: NonNull<NSNotification>| {
        let _ = tx.try_send(());
    });
    // The notification name is an AppKit constant. The block only signals a channel.
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSApplicationDidFinishRestoringWindowsNotification),
            None,
            None,
            &callback,
        )
    };
    async move {
        let _ = rx.recv().await;
        // Remove the token from the same notification center that issued it.
        unsafe { center.removeObserver((*observer).as_ref()) };
    }
}

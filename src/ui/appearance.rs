use objc2::MainThreadMarker;
use objc2_app_kit::{NSAppearance, NSAppearanceNameDarkAqua, NSApplication};

pub(super) fn use_dark_aqua() {
    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };
    // NSAppearanceNameDarkAqua is an immutable AppKit constant on supported macOS.
    let appearance = NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua });
    NSApplication::sharedApplication(main_thread).setAppearance(appearance.as_deref());
}

/// Locate the display in Quartz desktop coordinates (also covers screens left
/// of or above the primary display). GPUI window origins are display-relative.
pub(super) fn pointer_display(cx: &gpui::App) -> Option<std::rc::Rc<dyn gpui::PlatformDisplay>> {
    use objc2_core_graphics::{CGError, CGEvent, CGGetDisplaysWithPoint};
    let event = CGEvent::new(None)?;
    let position = CGEvent::location(Some(&event));
    let mut id = 0;
    let mut count = 0;
    // Both out-pointers address one stack value, matching the one-slot capacity.
    let status = unsafe { CGGetDisplaysWithPoint(position, 1, &mut id, &mut count) };
    if status != CGError::Success || count == 0 {
        return None;
    }
    cx.displays()
        .into_iter()
        .find(|display| u32::from(display.id()) == id)
}

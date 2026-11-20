//! A bare Command key produces flagsChanged events instead of keyDown/keyUp.
use block2::RcBlock;
use objc2::{MainThreadMarker, rc::Retained, runtime::AnyObject};
use objc2_app_kit::{NSApplication, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType};
use std::ptr::NonNull;
use voai_l10n::tr;

pub(crate) const SHORTCUT: &str = "MetaRight";
const KEY_CODE: u16 = 0x36; // kVK_RightCommand
const LEFT: usize = 0x08; // NX_DEVICELCMDKEYMASK
const RIGHT: usize = 0x10; // NX_DEVICERCMDKEYMASK

fn pressed(key: u16, flags: usize) -> Option<bool> {
    (key == KEY_CODE).then(|| {
        if flags & (RIGHT | LEFT) != 0 {
            flags & RIGHT != 0
        } else {
            // Some keyboards supply only device-independent flags.
            flags & NSEventModifierFlags::Command.bits() != 0
        }
    })
}

fn event_state(event: &NSEvent) -> Option<bool> {
    pressed(event.keyCode(), event.modifierFlags().bits())
}

pub(crate) fn capture_edge() -> Option<bool> {
    let event = NSApplication::sharedApplication(MainThreadMarker::new()?).currentEvent()?;
    (event.r#type() == NSEventType::FlagsChanged).then(|| event_state(&event))?
}

struct Monitor(Retained<AnyObject>);

impl Drop for Monitor {
    fn drop(&mut self) {
        // The token came from one of the AppKit event monitor registration APIs.
        unsafe { NSEvent::removeMonitor(&self.0) };
    }
}

pub(super) struct RightCommandMonitor {
    _global: Monitor,
    _local: Monitor,
}

impl RightCommandMonitor {
    pub fn new(notify: impl Fn(bool) + Clone + 'static) -> Result<Self, String> {
        let observe = move |ptr: NonNull<NSEvent>| {
            // AppKit owns this event for the duration of the callback.
            if let Some(down) = event_state(unsafe { ptr.as_ref() }) {
                notify(down);
            }
        };
        let global = RcBlock::new(observe.clone());
        let global = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
            NSEventMask::FlagsChanged,
            &global,
        )
        .map(Monitor)
        .ok_or(tr("app.shortcuts.monitor_global"))?;
        let local = RcBlock::new(move |event: NonNull<NSEvent>| {
            observe(event);
            event.as_ptr()
        });
        // Return the original event so normal Command shortcuts still reach the app.
        let local = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::FlagsChanged, &local)
        }
        .map(Monitor)
        .ok_or(tr("app.shortcuts.monitor_local"))?;
        Ok(Self {
            _global: global,
            _local: local,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn right_release_is_distinct_even_when_left_command_stays_down() {
        let cmd = NSEventModifierFlags::Command.bits();
        assert_eq!(pressed(0x37, cmd | LEFT), None);
        assert_eq!(pressed(KEY_CODE, cmd | LEFT | RIGHT), Some(true));
        assert_eq!(pressed(KEY_CODE, cmd | LEFT), Some(false));
        assert_eq!(pressed(KEY_CODE, cmd), Some(true));
        assert_eq!(pressed(KEY_CODE, 0), Some(false));
    }
}

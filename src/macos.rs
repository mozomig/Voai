use crate::insertion::{InsertionError, PendingInsertion};
use objc2_application_services::{AXError, AXIsProcessTrusted, AXUIElement, AXValue, AXValueType};
use objc2_core_foundation::{CFRange, CFRetained, CFString, CFType, ConcreteType};
use std::{
    ptr::{self, NonNull},
    thread,
    time::{Duration, Instant},
};
use voai_l10n::{tf, tr};

#[derive(PartialEq)]
pub(crate) struct Target {
    pid: i32,
    direct_write: bool,
    field: CFRetained<AXUIElement>,
    window: CFRetained<AXUIElement>,
    selection: CFRange,
}

fn check(status: AXError, action: &str) -> Result<(), String> {
    match status {
        AXError::Success => Ok(()),
        AXError::APIDisabled => Err(permission_hint().into()),
        _ => Err(format!("{action} (AX {})", status.0)),
    }
}

fn permission_hint() -> &'static str {
    tr("app.insertion.permission")
}

fn attribute<T: ConcreteType>(field: &AXUIElement, key: &str) -> Result<CFRetained<T>, AXError> {
    let mut raw: *const CFType = ptr::null();
    // Copy returns a retained CF object through the out parameter on success.
    let status = unsafe { field.copy_attribute_value(&CFString::from_str(key), (&mut raw).into()) };
    if status != AXError::Success {
        return Err(status);
    }
    let ptr = NonNull::new(raw.cast_mut()).ok_or(AXError::NoValue)?;
    let value = unsafe { CFRetained::<CFType>::from_raw(ptr) };
    value.downcast().map_err(|_| AXError::IllegalArgument)
}

fn required<T: ConcreteType>(field: &AXUIElement, key: &str) -> Result<CFRetained<T>, String> {
    attribute(field, key).map_err(|error| match error {
        AXError::APIDisabled => permission_hint().into(),
        AXError::CannotComplete => tr("app.insertion.app_timeout").into(),
        AXError::NoValue | AXError::AttributeUnsupported => match key {
            "AXSelectedTextRange" => tr("app.insertion.cursor_missing").into(),
            "AXFocusedUIElement" | "AXWindow" | "AXFocusedWindow" => {
                tr("app.insertion.place_cursor").into()
            }
            _ => tr("app.insertion.unsupported").into(),
        },
        _ => tf!("app.insertion.read_attribute", key = key, code = error.0),
    })
}

fn focused_field(pid: i32) -> Result<CFRetained<AXUIElement>, String> {
    // Some AppKit applications expose AXFocusedUIElement only on the application
    // element. Resolve the front process again for every focus check.
    let app = unsafe { AXUIElement::new_application(pid) };
    // Chromium enables its native accessibility tree on an application role query.
    let _ = attribute::<CFString>(&app, "AXRole");
    if let Ok(field) = attribute(&app, "AXFocusedUIElement") {
        return Ok(field);
    }
    // Embedded WebKit creates its accessibility subtree lazily. Materialize
    // structural containers, then ask the app for the authoritative focus again.
    let window: CFRetained<AXUIElement> = required(&app, "AXFocusedWindow")?;
    let mut pending = std::collections::VecDeque::from([window.clone()]);
    let deadline = std::time::Instant::now() + Duration::from_millis(180);
    for _ in 0..64 {
        let Some(container) = pending.pop_front() else {
            break;
        };
        if std::time::Instant::now() > deadline {
            break;
        }
        if let Ok(children) = attribute::<objc2_core_foundation::CFArray>(&container, "AXChildren")
        {
            // AXChildren contains retained CF objects; validate each element's type.
            let children = unsafe {
                CFRetained::cast_unchecked::<objc2_core_foundation::CFArray<CFType>>(children)
            };
            for item in children.iter().take(24) {
                if let Ok(child) = item.downcast::<AXUIElement>() {
                    pending.push_back(child);
                }
            }
        }
        if let Ok(field) = attribute(&app, "AXFocusedUIElement") {
            return Ok(field);
        }
    }
    match attribute(&app, "AXFocusedUIElement") {
        Ok(field) => Ok(field),
        Err(AXError::NoValue | AXError::AttributeUnsupported) => Ok(window),
        Err(status) => Err(tf!("app.insertion.check_field", code = status.0)),
    }
}

#[derive(PartialEq)]
enum Focus {
    Field(Target),
    Window(WindowTarget),
}

#[derive(PartialEq)]
struct WindowTarget {
    pid: i32,
    window: CFRetained<AXUIElement>,
    bundle: String,
    name: String,
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn IsSecureEventInputEnabled() -> u8;
}

fn capture(expected: Option<i32>) -> Result<Focus, String> {
    if !trusted() {
        return Err(permission_hint().into());
    }
    let pid = front_pid().ok_or(tr("app.insertion.no_app"))?;
    if expected.is_some_and(|expected| expected != pid) {
        return Err(tr("app.insertion.other_app").into());
    }
    let field = focused_field(pid)?;
    match attribute::<CFString>(&field, "AXSubrole") {
        Ok(role) if role.to_string() == "AXSecureTextField" => {
            return Err(tr("app.insertion.secure_field").into());
        }
        Ok(_) | Err(AXError::AttributeUnsupported | AXError::NoValue) => {}
        Err(error) => return Err(tf!("app.insertion.check_field", code = error.0)),
    }
    if attribute::<CFString>(&field, "AXRole").is_ok_and(|role| role.to_string() == "AXWindow") {
        let app = objc2_app_kit::NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .filter(|app| app.processIdentifier() == pid && pid != std::process::id() as i32)
            .ok_or(tr("app.insertion.other_app"))?;
        let bundle = app
            .bundleIdentifier()
            .ok_or(tr("app.insertion.window_only"))?
            .to_string();
        let owner = unsafe { AXUIElement::new_application(pid) };
        let window = required::<AXUIElement>(&owner, "AXFocusedWindow")?;
        if window != field || bundle.is_empty() {
            return Err(tr("app.insertion.place_cursor").into());
        }
        if unsafe { IsSecureEventInputEnabled() } != 0 {
            return Err(tr("app.insertion.secure_field").into());
        }
        return Ok(Focus::Window(WindowTarget {
            name: app
                .localizedName()
                .map_or_else(|| bundle.clone(), |s| s.to_string()),
            pid,
            window,
            bundle,
        }));
    }
    let window = required(&field, "AXWindow")?;
    let mut writable = 0;
    let status = unsafe {
        field.is_attribute_settable(
            &CFString::from_str("AXSelectedText"),
            (&mut writable).into(),
        )
    };
    let settable = match status {
        AXError::Success => writable != 0,
        AXError::AttributeUnsupported => false,
        error => {
            check(error, tr("app.insertion.check_support"))?;
            false
        }
    };
    // WebKit can acknowledge AXSelectedText writes without updating the DOM.
    // Inspect the ancestry only when a native AX write is otherwise possible.
    let direct_write = settable && has_native_ancestry(&field);
    if !direct_write {
        let role: CFRetained<CFString> = required(&field, "AXRole")?;
        if !matches!(
            role.to_string().as_str(),
            "AXTextField" | "AXTextArea" | "AXComboBox"
        ) {
            return Err(tr("app.insertion.choose_editable").into());
        }
    }
    // Remote accessibility elements can belong to a renderer. Keyboard events
    // must go to the foreground application hosting the captured window.
    if front_pid() != Some(pid) {
        return Err(tr("app.insertion.app_changed").into());
    }
    let selection = selected_range(&field)?;
    Ok(Focus::Field(Target {
        pid,
        direct_write,
        field,
        window,
        selection,
    }))
}

fn has_native_ancestry(field: &CFRetained<AXUIElement>) -> bool {
    let mut element = field.clone();
    // Monaco can put more than twenty containers between its editable surface
    // and AXWebArea. Only enable direct writes after finding the native host.
    for _ in 0..64 {
        let Ok(role) = attribute::<CFString>(&element, "AXRole") else {
            break;
        };
        match role.to_string().as_str() {
            "AXWebArea" => return false,
            "AXWindow" | "AXApplication" => return true,
            _ => {}
        }
        let Ok(parent) = attribute(&element, "AXParent") else {
            break;
        };
        element = parent;
    }
    false
}

fn selected_range(field: &AXUIElement) -> Result<CFRange, String> {
    let value: CFRetained<AXValue> = required(field, "AXSelectedTextRange")?;
    let mut selection = CFRange {
        location: 0,
        length: 0,
    };
    // AXValueGetValue checks the requested type before writing a CFRange.
    if !unsafe { value.value(AXValueType::CFRange, NonNull::from(&mut selection).cast()) }
        || selection.location < 0
        || selection.length < 0
    {
        return Err(tr("app.insertion.selection_missing").into());
    }
    Ok(selection)
}

pub fn trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

fn front_pid() -> Option<i32> {
    objc2_app_kit::NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|app| app.processIdentifier())
}

pub fn app_frontmost() -> bool {
    front_pid() == Some(std::process::id() as i32)
}

pub fn request_access() -> Result<(), String> {
    use objc2_application_services::{AXIsProcessTrustedWithOptions, kAXTrustedCheckOptionPrompt};
    use objc2_core_foundation::{CFBoolean, CFDictionary};
    let options = CFDictionary::from_slices(
        &[unsafe { kAXTrustedCheckOptionPrompt }],
        &[CFBoolean::new(true)],
    );
    if !unsafe { AXIsProcessTrustedWithOptions(Some(options.as_opaque())) } {
        // macOS may suppress the native prompt after a prior decision. The user
        // explicitly clicked the permission control, so open the relevant pane.
        open_permission_settings("Privacy_Accessibility")?;
    }
    Ok(())
}

fn open_permission_settings(pane: &str) -> Result<(), String> {
    std::process::Command::new("/usr/bin/open")
        .arg(format!(
            "x-apple.systempreferences:com.apple.preference.security?{pane}"
        ))
        .spawn()
        .map(|_| ())
        .map_err(|err| tf!("app.insertion.settings_failed", error = err))
}

pub fn microphone_allowed() -> bool {
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
    unsafe {
        AVMediaTypeAudio.is_some_and(|audio| {
            AVCaptureDevice::authorizationStatusForMediaType(audio)
                == AVAuthorizationStatus::Authorized
        })
    }
}

pub fn request_microphone() -> Result<(), String> {
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
    let audio = unsafe { AVMediaTypeAudio }.ok_or(tr("app.audio.mic_unavailable"))?;
    match unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) } {
        AVAuthorizationStatus::NotDetermined => unsafe {
            AVCaptureDevice::requestAccessForMediaType_completionHandler(
                audio,
                &block2::RcBlock::new(|_| {}),
            );
        },
        AVAuthorizationStatus::Authorized => (),
        _ => open_permission_settings("Privacy_Microphone")?,
    }
    Ok(())
}

impl Target {
    /// Read geometry once from the captured field. Editors vary in which AX
    /// geometry they publish; neither geometry nor its absence affects delivery.
    fn widget_bounds(&self) -> Option<objc2_core_foundation::CGRect> {
        use objc2_core_foundation::{CGPoint, CGRect, CGSize};
        let position = attribute::<AXValue>(&self.field, "AXPosition").ok();
        let size = attribute::<AXValue>(&self.field, "AXSize").ok();
        let mut point = CGPoint::default();
        let mut dimensions = CGSize::default();
        // AXValue writes only if the requested native type matches its payload.
        let field = position.zip(size).and_then(|(p, s)| {
            let valid = unsafe {
                p.value(AXValueType::CGPoint, NonNull::from(&mut point).cast())
                    && s.value(AXValueType::CGSize, NonNull::from(&mut dimensions).cast())
            };
            (valid && dimensions.width > 0. && dimensions.height > 0.)
                .then(|| CGRect::new(point, dimensions))
        });
        if field.is_some_and(|r| r.size.height <= 160.) {
            return field;
        }
        let caret = || {
            let range = CFRange {
                location: self.selection.location,
                length: 0,
            };
            let parameter =
                unsafe { AXValue::new(AXValueType::CFRange, NonNull::from(&range).cast()) }?;
            let mut out: *const CFType = ptr::null();
            let status = unsafe {
                self.field.copy_parameterized_attribute_value(
                    &CFString::from_str("AXBoundsForRange"),
                    &parameter,
                    (&mut out).into(),
                )
            };
            if status != AXError::Success {
                return None;
            }
            let raw = NonNull::new(out.cast_mut())?;
            let value = unsafe { CFRetained::<CFType>::from_raw(raw) }
                .downcast::<AXValue>()
                .ok()?;
            let mut rect = CGRect::default();
            let valid =
                unsafe { value.value(AXValueType::CGRect, NonNull::from(&mut rect).cast()) };
            (valid && rect.size.height > 0.).then_some(rect)
        };
        caret().or(field)
    }
}

pub struct Destination {
    focus: Focus,
    pub widget_anchor: Option<objc2_core_foundation::CGRect>,
}

impl Destination {
    pub fn capture(with_anchor: bool) -> Result<Self, String> {
        Self::capture_for(None, with_anchor)
    }

    fn capture_for(pid: Option<i32>, with_anchor: bool) -> Result<Self, String> {
        if !trusted() {
            return Err(permission_hint().into());
        }
        let system = unsafe { AXUIElement::new_system_wide() };
        check(
            unsafe { system.set_messaging_timeout(0.4) },
            tr("app.insertion.accessibility_timeout"),
        )?;
        // Electron publishes its editable DOM only after assistive clients opt in.
        let front = front_pid().ok_or(tr("app.insertion.no_app"))?;
        if pid.is_some_and(|expected| front != expected) {
            return Err(tr("app.insertion.other_app").into());
        }
        let app = unsafe { AXUIElement::new_application(front) };
        let attr = CFString::from_str("AXManualAccessibility");
        let mut supported = 0;
        if unsafe { app.is_attribute_settable(&attr, (&mut supported).into()) } == AXError::Success
            && supported != 0
        {
            unsafe { app.set_attribute_value(&attr, objc2_core_foundation::CFBoolean::new(true)) };
        }
        let target = capture(Some(front))?;
        let widget_anchor = match &target {
            Focus::Field(field) if with_anchor => field.widget_bounds(),
            _ => None,
        };
        Ok(Self {
            focus: target,
            widget_anchor,
        })
    }

    #[cfg(test)]
    pub(crate) fn test_window(bundle: &str) -> Self {
        Self {
            focus: Focus::Window(WindowTarget {
                // No actual delivery in session consent tests.
                pid: std::process::id() as i32,
                window: unsafe { AXUIElement::new_application(std::process::id() as i32) },
                bundle: bundle.into(),
                name: "Test editor".into(),
            }),
            widget_anchor: None,
        }
    }

    pub fn window_app(&self) -> Option<(&str, &str)> {
        match &self.focus {
            Focus::Window(target) => Some((&target.bundle, &target.name)),
            Focus::Field(_) => None,
        }
    }

    pub fn deliver(self, text: &str) -> Result<Option<Receipt>, InsertionError<String>> {
        let mut receipt = None;
        let field = match self.focus {
            Focus::Window(target) => return deliver_to_window(target, text).map(|()| None),
            Focus::Field(field) => field,
        };
        PendingInsertion::new(field).deliver(
            || {
                let Focus::Field(current) = capture(None)? else {
                    return Err(tr("app.dictation.target_changed").into());
                };
                if !current.direct_write && text.chars().any(char::is_control) {
                    return Err(tr("app.insertion.multiline").into());
                }
                Ok(current)
            },
            |target| {
                if target.direct_write {
                    let status = unsafe {
                        target.field.set_attribute_value(
                            &CFString::from_str("AXSelectedText"),
                            &CFString::from_str(text),
                        )
                    };
                    match status {
                        AXError::Failure | AXError::CannotComplete => {
                            // A setter can change the field and still report a failure.
                            receipt = Some(Receipt::new(target, text)?);
                            return Ok(());
                        }
                        _ => return check(status, tr("app.insertion.ax_write")),
                    }
                }
                type_unicode(target, text)?;
                receipt = Some(Receipt::new(target, text)?);
                Ok(())
            },
        )?;
        Ok(receipt)
    }
}

/// Quartz posts have no acknowledgement. Never retry a partially posted string.
fn type_unicode(target: &Target, text: &str) -> Result<(), String> {
    post_unicode(target.pid, text, || {
        if front_pid() != Some(target.pid) {
            return Err(tr("app.insertion.app_switched").into());
        }
        if focused_field(target.pid)? != target.field {
            return Err(tr("app.insertion.field_switched").into());
        }
        Ok(())
    })
    .map_err(|e| match e {
        InsertionError::Unavailable(reason) | InsertionError::DeliveryUnknown(reason) => reason,
        InsertionError::TargetChanged => tr("app.dictation.target_changed").into(),
    })
}

fn deliver_to_window(target: WindowTarget, text: &str) -> Result<(), InsertionError<String>> {
    if text.chars().any(char::is_control) {
        return Err(InsertionError::Unavailable(
            tr("app.insertion.multiline").into(),
        ));
    }
    let owner = unsafe { AXUIElement::new_application(target.pid) };
    post_unicode(target.pid, text, || {
        if !trusted() {
            return Err(permission_hint().into());
        }
        if unsafe { IsSecureEventInputEnabled() } != 0 {
            return Err(tr("app.insertion.secure_field").into());
        }
        if required::<AXUIElement>(&owner, "AXFocusedWindow")? != target.window {
            return Err(tr("app.dictation.target_changed").into());
        }
        match attribute::<AXUIElement>(&owner, "AXFocusedUIElement") {
            Ok(field) if field == target.window => (),
            Err(AXError::AttributeUnsupported | AXError::NoValue) => (),
            _ => return Err(tr("app.dictation.target_changed").into()),
        }
        if front_pid() != Some(target.pid) {
            return Err(tr("app.insertion.app_switched").into());
        }
        Ok(())
    })
}

fn post_unicode(
    pid: i32,
    text: &str,
    mut validate: impl FnMut() -> Result<(), String>,
) -> Result<(), InsertionError<String>> {
    use objc2_core_graphics::{CGEvent, CGEventFlags};
    let mut chars = text.chars().peekable();
    let mut posted = false;
    while chars.peek().is_some() {
        let failure = |message: String| {
            if posted {
                InsertionError::DeliveryUnknown(tr("app.insertion.uncertain").into())
            } else {
                InsertionError::Unavailable(message)
            }
        };
        let mut chunk = Vec::with_capacity(20);
        while let Some(&ch) = chars.peek() {
            if chunk.len() + ch.len_utf16() > 20 {
                break;
            }
            chunk.extend_from_slice(ch.encode_utf16(&mut [0; 2]));
            chars.next();
        }
        let down = CGEvent::new_keyboard_event(None, 0, true)
            .ok_or_else(|| failure(tr("app.insertion.keyboard_event").into()))?;
        let up = CGEvent::new_keyboard_event(None, 0, false)
            .ok_or_else(|| failure(tr("app.insertion.keyboard_event").into()))?;
        validate().map_err(failure)?;
        for event in [&down, &up] {
            CGEvent::set_flags(Some(event), CGEventFlags(0));
            // The slice remains live for the call and contains complete UTF-16 scalars.
            unsafe {
                CGEvent::keyboard_set_unicode_string(Some(event), chunk.len() as _, chunk.as_ptr())
            };
            CGEvent::post_to_pid(pid, Some(event));
        }
        posted = true;
    }
    Ok(())
}

/// The UI polls after returning to the event loop so the target can publish
/// asynchronous accessibility updates. This check never initiates a write.
pub struct Receipt {
    field: CFRetained<AXUIElement>,
    range: CFRange,
    parameter: CFRetained<AXValue>,
    expected: CFRetained<CFString>,
    require_caret: bool,
    deadline: Instant,
}

impl Receipt {
    fn new(target: &Target, text: &str) -> Result<Self, String> {
        let expected = CFString::from_str(text);
        let range = CFRange {
            location: target.selection.location,
            length: expected.length(),
        };
        let parameter = unsafe { AXValue::new(AXValueType::CFRange, NonNull::from(&range).cast()) }
            .ok_or(tr("app.insertion.verify_failed"))?;
        Ok(Self {
            field: target.field.clone(),
            range,
            parameter,
            expected,
            require_caret: target.direct_write,
            // Electron can update the document well before it publishes the new
            // accessible range. Return as soon as the edit is visible.
            deadline: Instant::now() + Duration::from_secs(4),
        })
    }

    pub fn poll(&self) -> Option<Result<(), String>> {
        let mut raw: *const CFType = ptr::null();
        let status = unsafe {
            self.field.copy_parameterized_attribute_value(
                &CFString::from_str("AXStringForRange"),
                &self.parameter,
                (&mut raw).into(),
            )
        };
        let range_matches = if status == AXError::Success
            && let Some(ptr) = NonNull::new(raw.cast_mut())
        {
            let value = unsafe { CFRetained::<CFType>::from_raw(ptr) };
            value
                .downcast::<CFString>()
                .is_ok_and(|actual| actual == self.expected)
        } else {
            false
        };
        if range_matches && !self.require_caret {
            return Some(Ok(()));
        }
        // A rejected AX write can leave identical pre-existing text at the
        // insertion point. Also require the caret to have advanced past it.
        if let Some(end) = self.range.location.checked_add(self.range.length)
            && selected_range(&self.field)
                .is_ok_and(|caret| caret.location == end && caret.length == 0)
        {
            if range_matches {
                return Some(Ok(()));
            }
            // Some editors expose only AXValue.
            if let Ok(value) = attribute::<CFString>(&self.field, "AXValue")
                && value.length() >= end
            {
                use objc2_core_foundation::{CFComparisonResult, CFStringCompareFlags};
                // The checked bounds describe a valid range in the captured field.
                if unsafe {
                    value.compare_with_options(
                        Some(&self.expected),
                        self.range,
                        CFStringCompareFlags(0),
                    )
                } == CFComparisonResult::CompareEqualTo
                {
                    return Some(Ok(()));
                }
            }
        }
        (status == AXError::InvalidUIElement || Instant::now() >= self.deadline)
            .then(|| Err(tr("app.insertion.uncertain").into()))
    }
}

pub fn probe(expected_pid: Option<i32>) -> Result<(), String> {
    if !trusted() {
        return Err(permission_hint().into());
    }
    // Automated probes specify the already-focused owner. Only the interactive
    // command needs time for a person to switch from their terminal.
    if expected_pid.is_none() {
        eprintln!("{}", tr("app.probe.choose_field"));
        thread::sleep(Duration::from_secs(4));
    }
    let destination = Destination::capture_for(expected_pid, false)?;
    if expected_pid.is_none() && destination.window_app().is_some() {
        return Err(tr("app.insertion.window_only").into());
    }
    eprintln!("{}", tr("app.probe.captured"));
    thread::sleep(if expected_pid.is_some() {
        Duration::from_millis(350)
    } else {
        Duration::from_secs(2)
    });
    let receipt = destination
        .deliver("Тест Voai 🎙")
        .map_err(|e| tf!("app.probe.result", error = format_args!("{e:?}")))?;
    if let Some(receipt) = receipt {
        loop {
            if let Some(result) = receipt.poll() {
                result?;
                break;
            }
            // Match GPUI's main run loop during a command-line probe. Chromium
            // delivers accessibility cache changes asynchronously on this loop.
            use objc2_core_foundation::{CFRunLoop, kCFRunLoopDefaultMode};
            CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, 0.030, false);
        }
    }
    println!("{}", tr("app.probe.complete"));
    Ok(())
}

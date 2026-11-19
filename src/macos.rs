use crate::insertion::{InsertionError, PendingInsertion};
use objc2_application_services::{AXError, AXIsProcessTrusted, AXUIElement, AXValue, AXValueType};
use objc2_core_foundation::{CFRange, CFRetained, CFString, CFType, ConcreteType};
use std::{
    ptr::{self, NonNull},
    thread,
    time::{Duration, Instant},
};

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
    "Разрешите Voai в «Конфиденциальность и безопасность → Универсальный доступ». Если переключатель уже включён, удалите прежнюю запись Voai и добавьте текущую Voai.app."
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
    attribute(field, key).map_err(|error| format!("Не удалось прочитать {key} (AX {})", error.0))
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
    let mut pending = std::collections::VecDeque::from([window]);
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
    required(&app, "AXFocusedUIElement")
}

fn capture(expected: Option<i32>) -> Result<Target, String> {
    let pid = front_pid().ok_or("Нет активного приложения")?;
    if expected.is_some_and(|expected| expected != pid) {
        return Err("Фокус находится в другом процессе.".into());
    }
    let field = focused_field(pid)?;
    match attribute::<CFString>(&field, "AXSubrole") {
        Ok(role) if role.to_string() == "AXSecureTextField" => {
            return Err("Защищённое поле.".into());
        }
        Ok(_) | Err(AXError::AttributeUnsupported | AXError::NoValue) => {}
        Err(error) => return Err(format!("Не удалось проверить тип поля (AX {})", error.0)),
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
            check(error, "Не удалось проверить поддержку прямого ввода")?;
            false
        }
    };
    let mut parent = field.clone();
    let mut web = false;
    for _ in 0..20 {
        if attribute::<CFString>(&parent, "AXRole")
            .is_ok_and(|role| role.to_string() == "AXWebArea")
        {
            web = true;
            break;
        }
        let Ok(next) = attribute(&parent, "AXParent") else {
            break;
        };
        parent = next;
    }
    // WebKit can acknowledge AXSelectedText writes without updating the DOM.
    // Send ordinary Unicode input to web editors so their input handlers run.
    let direct_write = settable && !web;
    if !direct_write {
        let role: CFRetained<CFString> = required(&field, "AXRole")?;
        if !matches!(
            role.to_string().as_str(),
            "AXTextField" | "AXTextArea" | "AXComboBox"
        ) {
            return Err("Выберите редактируемое текстовое поле.".into());
        }
    }
    // Remote accessibility elements can belong to a renderer. Keyboard events
    // must go to the foreground application hosting the captured window.
    if front_pid() != Some(pid) {
        return Err("Активное приложение сменилось.".into());
    }
    let selection = selected_range(&field)?;
    Ok(Target {
        pid,
        direct_write,
        field,
        window,
        selection,
    })
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
        return Err("Недоступно положение курсора или выделения.".into());
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
        .map_err(|err| format!("Не удалось открыть системные настройки: {err}"))
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
    let audio = unsafe { AVMediaTypeAudio }.ok_or("Микрофон недоступен")?;
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

pub struct Destination {
    pending: PendingInsertion<Target>,
}

impl Destination {
    pub fn capture() -> Result<Self, String> {
        Self::capture_for(None)
    }

    fn capture_for(pid: Option<i32>) -> Result<Self, String> {
        if !trusted() {
            return Err(permission_hint().into());
        }
        let system = unsafe { AXUIElement::new_system_wide() };
        check(
            unsafe { system.set_messaging_timeout(0.4) },
            "Таймаут Accessibility",
        )?;
        // Electron publishes its editable DOM only after assistive clients opt in.
        if let Some(front) = front_pid().filter(|front| pid.is_none_or(|pid| *front == pid)) {
            let app = unsafe { AXUIElement::new_application(front) };
            let attr = CFString::from_str("AXManualAccessibility");
            let mut supported = 0;
            if unsafe { app.is_attribute_settable(&attr, (&mut supported).into()) }
                == AXError::Success
                && supported != 0
            {
                unsafe {
                    app.set_attribute_value(&attr, objc2_core_foundation::CFBoolean::new(true))
                };
            }
        }
        let target = capture(pid)?;
        let pending = PendingInsertion::new(target);
        Ok(Self { pending })
    }

    pub fn deliver(self, text: &str) -> Result<Option<Receipt>, InsertionError<String>> {
        let mut receipt = None;
        self.pending.deliver(
            || {
                let current = capture(None)?;
                if !current.direct_write && text.chars().any(char::is_control) {
                    return Err(
                        "Для многострочного текста выберите поле с поддержкой AXSelectedText."
                            .into(),
                    );
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
                    return check(status, "Ошибка записи AXSelectedText");
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
    use objc2_core_graphics::{CGEvent, CGEventFlags};
    let mut chars = text.chars().peekable();
    while chars.peek().is_some() {
        if front_pid() != Some(target.pid) {
            return Err("Активное приложение сменилось во время ввода.".into());
        }
        let field = focused_field(target.pid)?;
        if field != target.field {
            return Err("Фокус изменился во время ввода.".into());
        }
        let mut chunk = Vec::with_capacity(20);
        while let Some(&ch) = chars.peek() {
            if chunk.len() + ch.len_utf16() > 20 {
                break;
            }
            chunk.extend_from_slice(ch.encode_utf16(&mut [0; 2]));
            chars.next();
        }
        let down = CGEvent::new_keyboard_event(None, 0, true).ok_or("Событие недоступно")?;
        let up = CGEvent::new_keyboard_event(None, 0, false).ok_or("Событие недоступно")?;
        for event in [&down, &up] {
            CGEvent::set_flags(Some(event), CGEventFlags(0));
            // The slice remains live for the call and contains complete UTF-16 scalars.
            unsafe {
                CGEvent::keyboard_set_unicode_string(Some(event), chunk.len() as _, chunk.as_ptr())
            };
            CGEvent::post_to_pid(target.pid, Some(event));
        }
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
            .ok_or("Не удалось проверить результат ввода.")?;
        Ok(Self {
            field: target.field.clone(),
            range,
            parameter,
            expected,
            deadline: Instant::now() + Duration::from_secs(1),
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
        if status == AXError::Success
            && let Some(ptr) = NonNull::new(raw.cast_mut())
        {
            let value = unsafe { CFRetained::<CFType>::from_raw(ptr) };
            if value
                .downcast::<CFString>()
                .is_ok_and(|actual| actual == self.expected)
            {
                return Some(Ok(()));
            }
        }
        // Some editors expose only AXValue. Require both the expected caret
        // and an exact match within the inserted range.
        if let Some(end) = self.range.location.checked_add(self.range.length)
            && selected_range(&self.field)
                .is_ok_and(|caret| caret.location == end && caret.length == 0)
            && let Ok(value) = attribute::<CFString>(&self.field, "AXValue")
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
        (status == AXError::InvalidUIElement || Instant::now() >= self.deadline)
            .then(|| Err("Приложение не подтвердило введённый текст. Проверьте поле.".into()))
    }
}

pub fn probe(expected_pid: Option<i32>) -> Result<(), String> {
    if !trusted() {
        return Err(permission_hint().into());
    }
    // Automated probes specify the already-focused owner. Only the interactive
    // command needs time for a person to switch from their terminal.
    if expected_pid.is_none() {
        eprintln!("Выберите тестовое поле. Захват цели через 4 секунды.");
        thread::sleep(Duration::from_secs(4));
    }
    let destination = Destination::capture_for(expected_pid)?;
    eprintln!("Цель зафиксирована.");
    thread::sleep(if expected_pid.is_some() {
        Duration::from_millis(350)
    } else {
        Duration::from_secs(2)
    });
    let receipt = destination
        .deliver("Тест Voai 🎙")
        .map_err(|e| format!("Результат ввода: {e:?}"))?;
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
    println!("Запрос отправлен. Проверьте фразу у курсора.");
    Ok(())
}

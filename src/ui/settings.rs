mod dictation;
mod files;
mod history;
mod model_switcher;
mod models;
mod processing;

use gpui::{prelude::*, *};
use gpui_component::{
    Disableable, Sizable, TitleBar,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu, PopupMenuItem},
};

use super::{Engine, style::*};
use crate::app::{Command, Mode, ModelRow, ModelState, Snapshot};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Page {
    Dictation,
    Models,
    Processing,
    History,
    Files,
}

impl Page {
    fn title(self) -> &'static str {
        match self {
            Self::Dictation => "Диктовка",
            Self::Models => "Модели",
            Self::Processing => "Обработка",
            Self::History => "История",
            Self::Files => "Файлы",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Self::Dictation => "mic",
            Self::Models => "models",
            Self::Processing => "sliders",
            Self::History => "history",
            Self::Files => "file",
        }
    }
}

pub(super) fn mode_title(mode: Mode) -> &'static str {
    match mode {
        Mode::Off => "Выкл.",
        Mode::Local => "Из файла",
        Mode::Server => "Локальный сервер",
        Mode::Cloud => "Облако",
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ModelTab {
    Speech,
    Text,
    Speakers,
}

pub(super) struct Settings {
    engine: Engine,
    snapshot: Snapshot,
    page: Page,
    files_ui: files::FileControls,
    expanded_entry: Option<i64>,
    confirm_delete: Option<i64>,
    model_tab: ModelTab,
    capture: bool,
    right_command_pending: bool,
    focus: FocusHandle,
    test: Entity<InputState>,
    key: Entity<InputState>,
    prompt: Entity<InputState>,
    prompt_needs_layout: bool,
    server_url: Entity<InputState>,
    show_server_key: bool,
    file_dialog_error: Option<String>,
    last_test: Option<u64>,
    selection_anchor: Option<(u64, std::ops::Range<usize>)>,
    edit_revision: u64,
    test_ready: bool,
    shortcut_error: Option<&'static str>,
}

impl Settings {
    pub(super) fn new(engine: Engine, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let snapshot = engine.borrow().snapshot();
        let test = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder("Нажмите здесь и попробуйте диктовать")
        });
        let key = cx.new(|cx| InputState::new(window, cx).masked(true));
        cx.subscribe_in(&test, window, |this, _, event, _, _| match event {
            InputEvent::Change => this.edit_revision = this.edit_revision.wrapping_add(1),
            InputEvent::Blur => this.set_test_ready(false),
            _ => {}
        })
        .detach();
        let prompt = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .soft_wrap(true)
                .default_value(snapshot.processing_prompt.clone())
        });
        let server_url =
            cx.new(|cx| InputState::new(window, cx).default_value(snapshot.server_url.clone()));
        cx.subscribe(&prompt, |_, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        cx.subscribe_in(&key, window, |this, _, event, window, cx| match event {
            InputEvent::PressEnter { .. } => this.save_key(window, cx),
            InputEvent::Change => cx.notify(),
            _ => {}
        })
        .detach();
        cx.subscribe_in(
            &server_url,
            window,
            |this, _, event, window, cx| match event {
                InputEvent::Change => {
                    this.show_server_key = false;
                    this.key.update(cx, |key, cx| key.set_value("", window, cx));
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.discover_server(cx),
                _ => {}
            },
        )
        .detach();
        let files_ui = files::FileControls::new(window, cx);
        let focus = cx.focus_handle();
        cx.on_blur(&focus, window, |this, _, cx| this.end_capture(cx))
            .detach();
        cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                this.end_capture(cx);
            }
        })
        .detach();
        window.focus(&focus);
        Self {
            engine,
            last_test: snapshot.test_result.as_ref().map(|(id, _)| *id),
            snapshot,
            page: Page::Dictation,
            files_ui,
            expanded_entry: None,
            confirm_delete: None,
            model_tab: ModelTab::Speech,
            capture: false,
            right_command_pending: false,
            focus,
            test,
            key,
            prompt,
            prompt_needs_layout: true,
            server_url,
            show_server_key: false,
            file_dialog_error: None,
            shortcut_error: None,
            selection_anchor: None,
            edit_revision: 0,
            test_ready: false,
        }
    }

    fn send(&self, command: Command) {
        self.engine.borrow_mut().dispatch(command);
    }

    fn set_test_ready(&mut self, ready: bool) {
        if self.test_ready != ready {
            self.test_ready = ready;
            self.send(Command::TestFocus(ready));
        }
    }

    /// Verify the same edit revision and UTF-16 selection just before the
    /// backend processes an input event or delivers a completed phrase.
    pub(super) fn check_test_target(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.page == Page::Dictation
            && window.is_window_active()
            && self.test.read(cx).focus_handle(cx).is_focused(window);
        let selection = if focused {
            self.test
                .update(cx, |state, cx| state.selected_text_range(false, window, cx))
                .map(|selected| (self.edit_revision, selected.range))
        } else {
            None
        };
        if matches!(
            self.snapshot.phase,
            crate::app::Phase::Idle | crate::app::Phase::RecoverFocus | crate::app::Phase::Error
        ) {
            self.selection_anchor.clone_from(&selection);
        }
        self.set_test_ready(selection.is_some() && selection == self.selection_anchor);
    }

    pub(super) fn sync(&mut self, snapshot: Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        if self.snapshot.processing_prompt != snapshot.processing_prompt
            && self.prompt.read(cx).value().as_str() == self.snapshot.processing_prompt
        {
            self.prompt.update(cx, |input, cx| {
                input.set_value(snapshot.processing_prompt.clone(), window, cx)
            });
        }
        let draft = self.server_url.read(cx).value();
        if self.snapshot.server_url != snapshot.server_url
            && (draft.as_str() == self.snapshot.server_url
                || voai_models::normalize_server_url(&draft)
                    .is_ok_and(|url| url == snapshot.server_url))
        {
            self.server_url.update(cx, |input, cx| {
                input.set_value(snapshot.server_url.clone(), window, cx)
            });
        }
        if self.snapshot.provider != snapshot.provider
            || self.snapshot.mode != snapshot.mode
            || self.snapshot.server_url != snapshot.server_url
        {
            self.show_server_key = false;
            self.key.update(cx, |key, cx| key.set_value("", window, cx));
        }
        if let Some((id, phrase)) = &snapshot.test_result
            && self.last_test != Some(*id)
        {
            self.test
                .update(cx, |test, cx| test.replace(phrase.clone(), window, cx));
            self.last_test = Some(*id);
            self.send(Command::TestResultHandled(*id));
        }
        if self.page == Page::History
            && (self.snapshot.history.entries.is_empty()
                || self
                    .expanded_entry
                    .is_some_and(|id| !snapshot.history.entries.iter().any(|entry| entry.id == id)))
        {
            self.expanded_entry = snapshot.history.entries.first().map(|e| e.id);
        }
        self.files_ui.sync(&self.snapshot.files, &snapshot.files);
        self.snapshot = snapshot;
        cx.notify();
    }

    pub(in crate::ui) fn show_dictation_setup(
        &mut self,
        models: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if models {
            self.model_tab = ModelTab::Speech;
            self.file_dialog_error = None;
            self.navigate(Page::Models, window, cx);
        } else {
            self.navigate(Page::Dictation, window, cx);
        }
    }

    pub(super) fn navigate(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        if self.page != page {
            if self.page == Page::Files {
                self.send(Command::File(crate::app::FileCommand::Stop));
                self.files_ui.leave();
            }
            if self.page == Page::History {
                self.send(Command::StopPlayback);
            }
            if page == Page::History {
                self.expanded_entry = self.snapshot.history.entries.first().map(|e| e.id);
            }
            self.confirm_delete = None;
            self.set_test_ready(false);
            self.show_server_key = false;
            self.key.update(cx, |key, cx| key.set_value("", window, cx));
        }
        if page == Page::Dictation {
            self.send(Command::RefreshDevices);
        }
        window.focus(&self.focus);
        self.end_capture(cx);
        self.page = page;
        self.prompt_needs_layout = page == Page::Processing;
        cx.notify();
    }

    fn end_capture(&mut self, cx: &mut Context<Self>) {
        if self.capture {
            self.capture = false;
            self.right_command_pending = false;
            self.shortcut_error = None;
            self.send(Command::CaptureShortcut(false));
            cx.notify();
        }
    }

    fn modifier_input(
        &mut self,
        event: &ModifiersChangedEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.capture {
            return;
        }
        let mods = event.modifiers;
        match crate::app::right_command::capture_edge() {
            Some(true) => self.right_command_pending = mods == Modifiers::command(),
            Some(false) if self.right_command_pending && mods == Modifiers::default() => {
                self.send(Command::SetShortcut(
                    crate::app::right_command::SHORTCUT.into(),
                ));
                self.end_capture(cx);
            }
            _ => self.right_command_pending = false,
        }
    }

    fn hotkey_input(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.capture {
            let key = &event.keystroke;
            if self.page == Page::Files && key.key == "escape" && self.files_ui.cancel_edit() {
                window.prevent_default();
                cx.stop_propagation();
                window.focus(&self.focus);
                cx.notify();
                return;
            }
            if self.page == Page::Processing
                && key.modifiers == Modifiers::command()
                && key.key == "s"
                && self.prompt.read(cx).focus_handle(cx).is_focused(window)
            {
                window.prevent_default();
                cx.stop_propagation();
                self.save_prompt(cx);
            }
            return;
        }
        window.prevent_default();
        cx.stop_propagation();
        self.right_command_pending = false;
        let key = &event.keystroke.key;
        if key == "escape" {
            self.end_capture(cx);
        } else {
            let mods = event.keystroke.modifiers;
            if !(mods.control || mods.alt || mods.platform) {
                self.shortcut_error = Some("Добавьте ⌥, ⌘ или ⌃.");
            } else {
                let mut parts = Vec::new();
                for (pressed, label) in [
                    (mods.control, "Control"),
                    (mods.alt, "Alt"),
                    (mods.shift, "Shift"),
                    (mods.platform, "Super"),
                ] {
                    if pressed {
                        parts.push(label.to_string());
                    }
                }
                parts.push(key.to_ascii_uppercase());
                self.send(Command::SetShortcut(parts.join("+")));
                self.end_capture(cx);
            }
        }
        cx.notify();
    }

    fn sidebar(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        col()
            .w(px(176.))
            .h_full()
            .flex_shrink_0()
            .px(px(10.))
            .py(px(18.))
            .gap(px(3.))
            .bg(p.side)
            .border_r_1()
            .border_color(p.rule)
            .children(
                [
                    Page::Dictation,
                    Page::Models,
                    Page::Processing,
                    Page::History,
                    Page::Files,
                ]
                .map(|page| {
                    let selected = page == self.page;
                    Button::new(page.title())
                        .ghost()
                        .w_full()
                        .justify_start()
                        .h(px(35.))
                        .px(px(9.))
                        .rounded(px(6.))
                        .when(selected, |b| b.bg(p.soft))
                        .child(
                            row()
                                .gap(px(9.))
                                .child(icon(
                                    page.icon(),
                                    if selected { p.accent } else { p.muted },
                                    17.,
                                ))
                                .child(text(page.title(), 13., p.ink).font_weight(if selected {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::NORMAL
                                })),
                        )
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.navigate(page, window, cx)),
                        )
                }),
            )
            .child(div().flex_1())
            .child(self.model_switcher(p, cx))
    }

    fn setting(p: Palette, title: &str, detail: &str, control: impl IntoElement) -> Div {
        Self::setting_row(
            p,
            title,
            (!detail.is_empty()).then(|| text(detail.to_owned(), 12., p.muted)),
            control,
        )
    }

    fn setting_row(p: Palette, title: &str, detail: Option<Div>, control: impl IntoElement) -> Div {
        row()
            .w_full()
            .gap_3()
            .justify_between()
            .py(px(7.))
            .border_b_1()
            .border_color(p.rule)
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(label(title.to_owned(), p.ink))
                    .children(detail),
            )
            .child(control)
    }

    fn picker(
        &self,
        id: &'static str,
        title: String,
        choices: Vec<(String, String)>,
        current: String,
        command: fn(String) -> Command,
        p: Palette,
    ) -> impl IntoElement + use<> {
        let engine = self.engine.clone();
        Button::new(id)
            .small()
            .rounded(px(6.))
            .bg(p.surface)
            .border_1()
            .border_color(p.rule)
            .child(
                row()
                    .gap(px(8.))
                    .child(text(title, 12., p.ink).max_w(px(230.)).truncate())
                    .child(icon("chevron", p.muted, 13.)),
            )
            .dropdown_menu(move |mut menu, _, _| {
                for (name, value) in &choices {
                    let engine = engine.clone();
                    let value = value.clone();
                    menu = menu.item(
                        PopupMenuItem::new(name.clone())
                            .checked(value == current)
                            .on_click(move |_, _, _| {
                                engine.borrow_mut().dispatch(command(value.clone()))
                            }),
                    );
                }
                menu
            })
    }

    fn tabs(
        &self,
        p: Palette,
        options: &[(&'static str, bool)],
        select: impl Fn(&mut Self, usize, &mut Context<Self>) + Copy + 'static,
        cx: &mut Context<Self>,
    ) -> Div {
        row()
            .p(px(2.))
            .gap(px(2.))
            .bg(p.side)
            .rounded(px(8.))
            .border_1()
            .border_color(p.rule)
            .children(options.iter().enumerate().map(|(index, (title, active))| {
                Button::new(*title)
                    .small()
                    .ghost()
                    .label(*title)
                    .rounded(px(5.))
                    .text_size(px(12.))
                    .text_color(if *active { p.ink } else { p.muted })
                    .when(*active, |b| b.bg(p.surface))
                    .on_click(cx.listener(move |this, _, _, cx| select(this, index, cx)))
            }))
    }
}

fn group_title(title: &'static str, p: Palette) -> Div {
    text(title, 12., p.ink)
        .font_weight(FontWeight::SEMIBOLD)
        .mt(px(14.))
        .mb_1()
}

impl Render for Settings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.page == Page::Processing && std::mem::take(&mut self.prompt_needs_layout) {
            // The editor can wrap the initial text once its first frame has a measured width.
            let prompt = self.prompt.clone();
            window.on_next_frame(move |window, cx| {
                prompt.update(cx, |input, cx| input.set_soft_wrap(true, window, cx))
            });
        }
        let p = Palette::dark();
        let page = match self.page {
            Page::Dictation => self.dictation(p, cx),
            Page::Models => self.models(p, cx),
            Page::Processing => self.processing(p, cx),
            Page::History => self.history(p, cx),
            Page::Files => self.files(p, cx),
        };
        div()
            .size_full()
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::hotkey_input))
            .on_modifiers_changed(cx.listener(Self::modifier_input))
            .flex()
            .flex_col()
            .bg(p.paper)
            .text_color(p.ink)
            .font_family(".SystemUIFont")
            .text_size(px(14.))
            .child(
                TitleBar::new().bg(p.side).child(
                    row()
                        .gap(px(8.))
                        .child(mark(p.accent))
                        .child(text("Voai", 14., p.ink).font_weight(FontWeight::SEMIBOLD)),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.sidebar(p, cx))
                    .child(
                        col()
                            .id(SharedString::from(format!(
                                "settings-{}",
                                self.page.title()
                            )))
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .when(self.page != Page::Files, |view| view.overflow_y_scroll())
                            .px(px(28.))
                            .py(px(20.))
                            .child(
                                text(
                                    if self.page == Page::Files {
                                        "Расшифровка файла"
                                    } else {
                                        self.page.title()
                                    },
                                    24.,
                                    p.ink,
                                )
                                .font_weight(FontWeight::SEMIBOLD)
                                .mb(px(16.)),
                            )
                            .when(
                                !self.snapshot.message.is_empty()
                                    && self.snapshot.phase == crate::app::Phase::Idle,
                                |d| {
                                    d.child(
                                        row()
                                            .mb_2()
                                            .p_2()
                                            .rounded_md()
                                            .gap_2()
                                            .bg(p.soft)
                                            .child(
                                                text(self.snapshot.message.clone(), 12., p.ink)
                                                    .flex_1(),
                                            )
                                            .child(button("dismiss", "Закрыть", p).on_click(
                                                cx.listener(|this, _, _, _| {
                                                    this.send(Command::DismissMessage)
                                                }),
                                            )),
                                    )
                                },
                            )
                            .child(page),
                    ),
            )
    }
}

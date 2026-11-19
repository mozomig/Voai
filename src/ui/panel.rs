use std::collections::VecDeque;

use super::{Engine, style::*};
use crate::app::{Command, ModelState, Phase, Snapshot};
use gpui::{prelude::*, *};

const BARS: usize = 23;

pub(super) fn height(snapshot: &Snapshot) -> Pixels {
    match snapshot.phase {
        Phase::Error if snapshot.message.chars().count() <= 90 => px(146.),
        Phase::RecoverFocus | Phase::RecoverText | Phase::Uncertain => px(218.),
        _ => px(178.),
    }
}

pub(super) struct Panel {
    engine: Engine,
    snapshot: Snapshot,
    levels: VecDeque<f32>,
}

impl Panel {
    pub(super) fn new(engine: Engine, snapshot: Snapshot, window: &Window) -> Self {
        configure_panel(window);
        Self {
            engine,
            snapshot,
            levels: VecDeque::from(vec![0.; BARS]),
        }
    }

    pub(super) fn sync(&mut self, snapshot: Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        if height(&snapshot) != height(&self.snapshot) {
            window.resize(size(window.bounds().size.width, height(&snapshot)));
        }
        if snapshot.phase == Phase::Listening {
            self.levels.pop_front();
            self.levels.push_back(snapshot.level.clamp(0., 1.));
        } else if snapshot == self.snapshot {
            return;
        }
        self.snapshot = snapshot;
        cx.notify();
    }
}

/// Keep keyboard focus in the editor even when clicking a panel action.
fn configure_panel(window: &Window) {
    use objc2_app_kit::{NSPanel, NSView};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // GPUI supplies the host NSView. The Window borrow keeps it alive and
    // window creation always runs on the AppKit main thread.
    let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    if let Some(panel) = view.window().and_then(|w| w.downcast::<NSPanel>().ok()) {
        panel.setBecomesKeyOnlyIfNeeded(true);
        panel.setHidesOnDeactivate(false);
    }
}

pub(super) fn phase_title(phase: Phase) -> &'static str {
    match phase {
        Phase::Idle => "Готов к диктовке",
        Phase::Listening => "Слушаю",
        Phase::Finishing => "Завершаю…",
        Phase::Editing => "Редактирую…",
        Phase::Inserting => "Ввожу текст…",
        Phase::RecoverFocus => "Выберите поле ввода",
        Phase::RecoverText => "Обработка недоступна",
        Phase::Uncertain => "Проверьте поле ввода",
        Phase::Error => "Диктовка прервана",
    }
}

impl Render for Panel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::dark();
        let s = &self.snapshot;
        let listening = s.phase == Phase::Listening;
        let recovering = matches!(
            s.phase,
            Phase::RecoverFocus | Phase::RecoverText | Phase::Uncertain | Phase::Error
        );
        let transcript = if recovering {
            s.transcript.clone()
        } else {
            live_tail(&s.transcript, 108)
        };
        let empty = transcript.is_empty();
        let last_word = listening
            .then(|| transcript.trim_end())
            .and_then(|trimmed| {
                trimmed
                    .split_whitespace()
                    .next_back()
                    .map(|word| trimmed.len() - word.len()..trimmed.len())
            });
        let mut live_text = StyledText::new(transcript);
        if let Some(range) = last_word {
            live_text = live_text.with_highlights([(
                range,
                HighlightStyle {
                    underline: Some(UnderlineStyle {
                        thickness: px(1.),
                        color: Some(p.accent),
                        wavy: false,
                    }),
                    ..Default::default()
                },
            )]);
        }
        let hint = match s.phase {
            Phase::Listening if s.elapsed_secs >= 300 => {
                "Запись остановлена. Отпустите клавиши.".into()
            }
            Phase::Listening if s.elapsed_secs >= 290 => {
                format!("Осталось {} с до остановки записи", 300 - s.elapsed_secs)
            }
            Phase::Listening => "Отпустите клавиши для ввода".into(),
            Phase::RecoverFocus => {
                format!("Выберите поле и нажмите {}", shortcut_label(&s.shortcut))
            }
            _ => String::new(),
        };
        let setup: Option<(&str, Box<dyn Action>)> = if s.phase != Phase::Error {
            None
        } else if !s.access_allowed || !s.mic_allowed {
            Some(("Открыть разрешения", Box::new(super::OpenPermissions)))
        } else if !s
            .models
            .iter()
            .any(|m| !m.text_model && matches!(m.state, ModelState::Selected))
        {
            Some(("Выбрать речевую модель", Box::new(super::OpenSpeechModels)))
        } else {
            None
        };
        let footer = match setup {
            Some((label, action)) => primary_button("open-setup", label, p)
                .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
                .into_any_element(),
            None => text(hint, 11., p.muted).into_any_element(),
        };
        col()
            .size_full()
            .bg(p.side)
            .text_color(p.ink)
            .font_family(".SystemUIFont")
            .rounded(px(12.))
            .border_1()
            .border_color(p.rule)
            .px(px(16.))
            .pt(px(14.))
            .pb(px(10.))
            .child(
                row()
                    .w_full()
                    .justify_between()
                    .gap_2()
                    .child(
                        row()
                            .gap_2()
                            .child(mark(if recovering { p.muted } else { p.accent }))
                            .child(
                                text(phase_title(s.phase), 13., p.ink)
                                    .font_weight(FontWeight::SEMIBOLD),
                            ),
                    )
                    .when(!recovering, |r| {
                        r.child(
                            row()
                                .gap(px(9.))
                                .when(listening, |wave| {
                                    wave.child(row().h(px(22.)).gap(px(2.)).children(
                                        self.levels.iter().map(|&v| {
                                            div()
                                                .rounded_full()
                                                .w(px(2.))
                                                .h(px(2. + v.sqrt() * 18.))
                                                .bg(p.accent)
                                        }),
                                    ))
                                })
                                .child(
                                    text(
                                        format!(
                                            "{:02}:{:02}",
                                            s.elapsed_secs / 60,
                                            s.elapsed_secs % 60
                                        ),
                                        11.,
                                        p.muted,
                                    )
                                    .font_family("Menlo"),
                                ),
                        )
                    }),
            )
            .child(
                col()
                    .id("transcript")
                    .mt(px(12.))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when((recovering || listening) && !s.message.is_empty(), |v| {
                        v.child(text(s.message.clone(), 13., p.ink).mb_1())
                    })
                    .child(
                        div()
                            .text_size(px(17.))
                            .line_height(px(22.))
                            .child(live_text)
                            .when(listening && empty, |d| {
                                d.child(text("Говорите…", 17., p.muted))
                            }),
                    ),
            )
            .when(s.phase == Phase::RecoverText, |v| {
                v.child(row().mt_1().child(
                    primary_button("insert-original", "Ввести исходный текст", p).on_click(
                        cx.listener(|this, _, _, _| {
                            this.engine.borrow_mut().dispatch(Command::UseOriginal)
                        }),
                    ),
                ))
            })
            .child(
                row()
                    .mt_2()
                    .pt_2()
                    .border_t_1()
                    .border_color(p.rule)
                    .justify_between()
                    .gap_2()
                    .child(
                        button(
                            "cancel",
                            if recovering || s.phase == Phase::Inserting {
                                "Закрыть  Esc"
                            } else {
                                "Отменить  Esc"
                            },
                            p,
                        )
                        .on_click(cx.listener(|this, _, _, _| {
                            this.engine.borrow_mut().dispatch(Command::Cancel)
                        })),
                    )
                    .child(footer),
            )
    }
}

/// Mark omitted text and avoid starting the preview in the middle of a word.
fn live_tail(text: &str, count: usize) -> String {
    let mut reversed = text.char_indices().rev();
    let Some((last_omitted, omitted_char)) = reversed.nth(count) else {
        return text.to_owned();
    };
    let suffix = &text[last_omitted + omitted_char.len_utf8()..];
    let visible = if omitted_char.is_whitespace() {
        suffix
    } else {
        suffix
            .split_once(char::is_whitespace)
            .map(|(_, rest)| rest.trim_start())
            .filter(|rest| !rest.is_empty())
            .unwrap_or(suffix)
    };
    format!("…{}", visible.trim_start())
}

#[cfg(test)]
mod tests {
    use super::live_tail;

    #[test]
    fn live_preview_marks_omissions_at_word_and_utf8_boundaries() {
        assert_eq!(live_tail("ёж 🦀", 2), "…🦀");
        assert_eq!(live_tail("ёж 🦀", 4), "ёж 🦀");
        assert_eq!(live_tail("доброе утро", 4), "…утро");
        assert_eq!(live_tail("длинноеслово", 5), "…слово");
        assert_eq!(live_tail("", 108), "");
    }
}

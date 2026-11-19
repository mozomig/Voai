use std::collections::VecDeque;

use super::{Engine, style::*};
use crate::app::{Command, Phase, Snapshot};
use gpui::{prelude::*, *};

const BARS: usize = 23;

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

    pub(super) fn sync(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
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
            s.transcript.as_str()
        } else {
            tail(&s.transcript, 108)
        };
        let mut live_text = StyledText::new(transcript.to_owned());
        if listening
            && let Some(last) = transcript.split_whitespace().next_back()
            && let Some(start) = transcript.rfind(last)
        {
            live_text = live_text.with_highlights([(
                start..start + last.len(),
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
            Phase::Listening if !s.message.is_empty() => s.message.clone(),
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
            Phase::RecoverText | Phase::Uncertain => "Фраза сохранена в панели".into(),
            Phase::Error => String::new(),
            _ => String::new(),
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
                    .child(
                        div()
                            .text_size(px(17.))
                            .line_height(px(22.))
                            .child(live_text)
                            .when(listening && transcript.is_empty(), |d| {
                                d.child(text("Говорите…", 17., p.muted))
                            }),
                    )
                    .when(recovering && !s.message.is_empty(), |v| {
                        v.child(text(s.message.clone(), 12., p.muted))
                    }),
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
                    .child(text(hint, 11., p.muted)),
            )
    }
}

fn tail(text: &str, count: usize) -> &str {
    text.char_indices()
        .rev()
        .nth(count.saturating_sub(1))
        .map_or(text, |(offset, _)| &text[offset..])
}

#[cfg(test)]
mod tests {
    use super::tail;

    #[test]
    fn live_tail_keeps_utf8_character_boundaries() {
        assert_eq!(tail("ёж 🦀", 2), " 🦀");
        assert_eq!(tail("ёж 🦀", 20), "ёж 🦀");
        assert_eq!(tail("", 108), "");
    }
}

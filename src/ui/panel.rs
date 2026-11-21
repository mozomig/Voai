use std::collections::VecDeque;
use voai_l10n::{tf, tr};

use super::{Engine, copy_text::CopyText, style::*};
use crate::app::{Command, ModelState, Phase, Snapshot};
use gpui::{prelude::*, *};

const BARS: usize = 23;

pub(super) fn compact(snapshot: &Snapshot) -> bool {
    snapshot.compact_widget
        && matches!(
            snapshot.phase,
            Phase::Listening | Phase::Finishing | Phase::Editing | Phase::Inserting
        )
}

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
    opened: std::time::Instant,
}

impl Panel {
    pub(super) fn new(engine: Engine, snapshot: Snapshot, window: &Window) -> Self {
        configure_panel(window, compact(&snapshot));
        Self {
            engine,
            snapshot,
            levels: VecDeque::from(vec![0.; BARS]),
            opened: std::time::Instant::now(),
        }
    }

    pub(super) fn sync(&mut self, snapshot: Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        if height(&snapshot) != height(&self.snapshot) {
            window.resize(size(window.bounds().size.width, height(&snapshot)));
        }
        if snapshot.phase == Phase::Listening {
            self.levels.pop_front();
            self.levels.push_back(snapshot.level.clamp(0., 1.));
        } else if snapshot == self.snapshot && !compact(&snapshot) {
            return;
        }
        self.snapshot = snapshot;
        cx.notify();
    }
}

/// Keep keyboard focus in the editor even when clicking a panel action.
fn configure_panel(window: &Window, compact: bool) {
    if let Some(panel) = super::appearance::native_window(window)
        .and_then(|w| w.downcast::<objc2_app_kit::NSPanel>().ok())
    {
        panel.setCanHide(false);
        panel.setBecomesKeyOnlyIfNeeded(true);
        panel.setHidesOnDeactivate(false);
        panel.setIgnoresMouseEvents(compact);
    }
}

pub(super) fn phase_title(phase: Phase) -> &'static str {
    match phase {
        Phase::Idle => tr("ui.phase.ready"),
        Phase::Listening => tr("ui.phase.listening"),
        Phase::Finishing => tr("ui.phase.transcribing"),
        Phase::Editing => tr("ui.phase.editing"),
        Phase::Inserting => tr("ui.phase.inserting"),
        Phase::RecoverFocus => tr("ui.phase.focus"),
        Phase::RecoverText => tr("ui.phase.edit_failed"),
        Phase::Uncertain => tr("ui.phase.uncertain"),
        Phase::Error => tr("ui.phase.error"),
    }
}

impl Render for Panel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::dark();
        let s = &self.snapshot;
        let listening = s.phase == Phase::Listening;
        if compact(s) {
            return row()
                .size_full()
                .justify_center()
                .gap(px(2.))
                .rounded_full()
                .border_1()
                .border_color(p.rule)
                .bg(p.side)
                .children(
                    self.levels
                        .iter()
                        .skip(BARS - 13)
                        .enumerate()
                        .map(|(i, level)| {
                            let amplitude = if listening {
                                level.sqrt() * 17.
                            } else {
                                (self.opened.elapsed().as_secs_f32() * 5. - i as f32 * 0.6)
                                    .sin()
                                    .mul_add(3., 4.)
                            };
                            div()
                                .w(px(3.))
                                .h(px(3. + amplitude))
                                .rounded_full()
                                .bg(if listening { p.accent } else { p.muted })
                        }),
                );
        }
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
            Phase::Listening if s.elapsed_secs >= 300 => tr("ui.panel.limit").into(),
            Phase::Listening if s.elapsed_secs >= 290 => {
                tf!("ui.panel.remaining", seconds = 300 - s.elapsed_secs)
            }
            Phase::Listening => tr("ui.panel.release").into(),
            Phase::RecoverFocus => {
                tf!(
                    "ui.panel.focus_hint",
                    shortcut = shortcut_label(&s.shortcut)
                )
            }
            _ => String::new(),
        };
        let setup: Option<(&str, Box<dyn Action>)> = if s.phase != Phase::Error {
            None
        } else if !s.access_allowed || !s.mic_allowed {
            Some((tr("ui.panel.permissions"), Box::new(super::OpenPermissions)))
        } else if !s
            .models
            .iter()
            .any(|m| !m.text_model && matches!(m.state, ModelState::Selected))
        {
            Some((tr("ui.model.choose"), Box::new(super::OpenSpeechModels)))
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
                            )
                            .when(!empty, |r| {
                                r.child(CopyText::new(
                                    "panel-copy",
                                    &s.transcript,
                                    tr("ui.action.copy_text"),
                                ))
                            }),
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
                                d.child(text(tr("ui.panel.speak"), 17., p.muted))
                            }),
                    ),
            )
            .when_some(s.window_prompt, |v, session| {
                v.child(
                    row()
                        .mt_1()
                        .gap_2()
                        .child(
                            primary_button("insert-window", tr("ui.panel.insert_here"), p)
                                .on_click(cx.listener(move |this, _, _, _| {
                                    this.engine.borrow_mut().dispatch(Command::ConfirmWindow {
                                        session,
                                        remember: false,
                                    })
                                })),
                        )
                        .child(
                            button("remember-window", tr("ui.panel.always_here"), p).on_click(
                                cx.listener(move |this, _, _, _| {
                                    this.engine.borrow_mut().dispatch(Command::ConfirmWindow {
                                        session,
                                        remember: true,
                                    })
                                }),
                            ),
                        ),
                )
            })
            .when(s.phase == Phase::RecoverText, |v| {
                v.child(row().mt_1().child(
                    primary_button("insert-original", tr("ui.panel.insert_original"), p).on_click(
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
                                tr("ui.panel.close")
                            } else {
                                tr("ui.panel.cancel")
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

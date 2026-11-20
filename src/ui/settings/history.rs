use super::*;
use crate::app::{HistoryOutcome, Phase};
use chrono::{DateTime, Local};

impl Settings {
    pub(super) fn history(&self, p: Palette, cx: &Context<Self>) -> Div {
        let history = &self.snapshot.history;
        let mut list = col()
            .w_full()
            .flex_shrink_0()
            .gap(px(8.))
            .child(text(
                "Последние 20 диктовок хранятся на этом Mac.",
                12.,
                p.muted,
            ))
            .children(history.error.map(|message| text(message, 12., p.error)));
        if history.entries.is_empty() {
            return list.child(
                col()
                    .mt(px(18.))
                    .p(px(20.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(p.rule)
                    .bg(p.side)
                    .gap(px(6.))
                    .child(icon("history", p.muted, 20.))
                    .child(label(
                        if history.loading {
                            "Загружаю записи…"
                        } else {
                            "Записей пока нет"
                        },
                        p.ink,
                    ))
                    .child(text(
                        "После диктовки здесь появятся аудио и текст.",
                        12.,
                        p.muted,
                    )),
            );
        }
        let today = Local::now().date_naive();
        let mut last_day = None;
        for entry in history.entries.iter() {
            let Some(date) =
                DateTime::from_timestamp(entry.started, 0).map(|d| d.with_timezone(&Local))
            else {
                continue;
            };
            let day = date.date_naive();
            if last_day != Some(day) {
                let title = if day == today {
                    "Сегодня".into()
                } else if Some(day) == today.pred_opt() {
                    "Вчера".into()
                } else {
                    day.format("%d.%m.%Y").to_string()
                };
                list = list.child(
                    text(title, 12., p.muted)
                        .mt(px(8.))
                        .font_weight(FontWeight::SEMIBOLD),
                );
                last_day = Some(day);
            }
            let id = entry.id;
            let expanded = self.expanded_entry == Some(id);
            let playing = history.playing == Some(id);
            let secs = entry.duration_ms.div_ceil(1000);
            let playback = button(
                ("history-play", id as u64),
                if playing {
                    "Стоп"
                } else {
                    "Слушать"
                },
                p,
            )
            .disabled(matches!(
                self.snapshot.phase,
                Phase::Listening | Phase::Finishing
            ))
            .on_click(cx.listener(move |this, _, _, _| {
                this.send(if playing {
                    Command::StopPlayback
                } else {
                    Command::PlayEntry(id)
                })
            }));
            let mut card = col()
                .w_full()
                .flex_shrink_0()
                .min_w_0()
                .rounded(px(8.))
                .bg(p.side)
                .border_1()
                .border_color(p.rule)
                .p(px(12.))
                .gap(px(8.))
                .child(
                    row()
                        .gap(px(8.))
                        .child(
                            text(date.format("%H:%M").to_string(), 13., p.ink)
                                .font_weight(FontWeight::SEMIBOLD),
                        )
                        .child(text(
                            format!("{}:{:02}", secs / 60, secs % 60),
                            12.,
                            p.muted,
                        ))
                        .child(div().flex_1())
                        .child(playback)
                        .child(
                            Button::new(("history-open", id as u64))
                                .ghost()
                                .small()
                                .label(if expanded {
                                    "Свернуть"
                                } else {
                                    "Подробнее"
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.expanded_entry = (!expanded).then_some(id);
                                    this.confirm_delete = None;
                                    cx.notify();
                                })),
                        ),
                );
            if expanded {
                let different =
                    matches!(&entry.outcome, HistoryOutcome::Edited(s) if s != &entry.original);
                card = card
                    .child(text(
                        if different {
                            "До обработки"
                        } else {
                            "Расшифровка"
                        },
                        11.,
                        p.muted,
                    ))
                    .child(text(entry.original.clone(), 14., p.ink));
                card = match &entry.outcome {
                    HistoryOutcome::Edited(edited) if different => card
                        .child(div().h_px().bg(p.rule))
                        .child(text("После обработки", 11., p.accent))
                        .child(text(edited.clone(), 14., p.ink)),
                    HistoryOutcome::Edited(_) => {
                        card.child(text("Обработка не изменила текст.", 12., p.muted))
                    }
                    HistoryOutcome::Failed => card.child(text(
                        "Обработка не удалась. Сохранён исходный текст.",
                        12.,
                        p.muted,
                    )),
                    HistoryOutcome::Off => card,
                };
                let confirm = self.confirm_delete == Some(id);
                card = card.child(
                    row()
                        .justify_end()
                        .gap_2()
                        .when(confirm, |r| {
                            r.child(button(("history-keep", id as u64), "Отмена", p).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.confirm_delete = None;
                                    cx.notify();
                                }),
                            ))
                        })
                        .child(
                            Button::new(("history-delete", id as u64))
                                .small()
                                .ghost()
                                .text_color(if confirm { p.error } else { p.muted })
                                .label(if confirm {
                                    "Удалить аудио и текст"
                                } else {
                                    "Удалить"
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if confirm {
                                        this.send(Command::DeleteEntry(id));
                                    }
                                    this.confirm_delete = (!confirm).then_some(id);
                                    cx.notify();
                                })),
                        ),
                );
            } else {
                let preview = match &entry.outcome {
                    HistoryOutcome::Edited(text) => text,
                    _ => &entry.original,
                };
                let mut chars = preview.lines().next().unwrap_or("").chars();
                let mut excerpt: String = chars.by_ref().take(85).collect();
                if chars.next().is_some() {
                    excerpt.push('…');
                }
                card = card.child(text(excerpt, 13., p.ink));
            }
            list = list.child(card);
        }
        list
    }
}

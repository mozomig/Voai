use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Step {
    Welcome,
    Permissions,
}

impl Settings {
    fn finish_introduction(&mut self, models: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.snapshot.onboarding_complete
            || self
                .engine
                .borrow_mut()
                .try_dispatch(Command::CompleteOnboarding)
        {
            self.show_dictation_setup(models, window, cx);
        }
    }

    pub(super) fn introduction(&self, step: Step, p: Palette, cx: &Context<Self>) -> Div {
        let permissions = step == Step::Permissions;
        let needs_model = !self
            .snapshot
            .models
            .iter()
            .any(|model| !model.text_model && matches!(model.state, ModelState::Selected));
        let (title, subtitle, next) = match step {
            Step::Welcome => ("ui.intro.title", "ui.intro.subtitle", "ui.intro.next"),
            Step::Permissions => (
                "ui.intro.permissions",
                "ui.intro.privacy",
                if needs_model {
                    "ui.intro.choose_model"
                } else {
                    "ui.intro.done"
                },
            ),
        };
        let content = col()
            .w_full()
            .max_w(px(540.))
            .gap(px(16.))
            .child(
                col()
                    .gap(px(6.))
                    .child(text(tr(title), 28., p.ink).font_weight(FontWeight::SEMIBOLD))
                    .child(text(tr(subtitle), 14., p.muted)),
            )
            .child(if permissions {
                col()
                    .w_full()
                    .rounded(px(10.))
                    .bg(p.side)
                    .border_1()
                    .border_color(p.rule)
                    .child(self.intro_permission(true, p, cx))
                    .child(div().mx(px(16.)).h(px(1.)).bg(p.rule))
                    .child(self.intro_permission(false, p, cx))
            } else {
                col()
                    .w_full()
                    .rounded(px(10.))
                    .bg(p.side)
                    .border_1()
                    .border_color(p.rule)
                    .child(
                        row()
                            .px(px(16.))
                            .py(px(12.))
                            .gap_2()
                            .child(mark(p.accent))
                            .child(label(tr("ui.intro.hold"), p.muted))
                            .child(
                                text(shortcut_label(&self.snapshot.shortcut), 13., p.ink)
                                    .font_weight(FontWeight::MEDIUM)
                                    .px_2()
                                    .py_1()
                                    .bg(p.surface)
                                    .rounded(px(5.))
                                    .border_1()
                                    .border_color(p.rule),
                            ),
                    )
                    .child(div().h(px(1.)).bg(p.rule))
                    .child(
                        row()
                            .px(px(16.))
                            .py(px(15.))
                            .gap_1()
                            .child(text(tr("ui.intro.sample"), 19., p.ink))
                            .child(div().w(px(2.)).h(px(21.)).bg(p.accent)),
                    )
            })
            .when(permissions, |d| {
                d.child(text(tr("ui.intro.file_hint"), 12., p.muted))
            })
            .when(!self.snapshot.message.is_empty(), |d| {
                d.child(text(self.snapshot.message.clone(), 12., p.error))
            });
        col()
            .size_full()
            .min_h_0()
            .child(
                col()
                    .id("intro-content")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        col()
                            .min_h_full()
                            .items_center()
                            .justify_center()
                            .px(px(32.))
                            .py(px(20.))
                            .child(content),
                    ),
            )
            .child(
                row()
                    .flex_shrink_0()
                    .w_full()
                    .justify_center()
                    .px(px(32.))
                    .py(px(14.))
                    .border_t_1()
                    .border_color(p.rule)
                    .child(
                        row()
                            .w_full()
                            .max_w(px(540.))
                            .justify_between()
                            .child(
                                row()
                                    .gap_2()
                                    .when(permissions, |d| {
                                        d.child(
                                            button("intro-back", tr("ui.intro.back"), p)
                                                .ghost()
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.intro = Some(Step::Welcome);
                                                    cx.notify();
                                                })),
                                        )
                                    })
                                    .when(!permissions, |d| {
                                        d.child(
                                            button("intro-skip", tr("ui.intro.skip"), p)
                                                .ghost()
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.finish_introduction(false, window, cx)
                                                })),
                                        )
                                    }),
                            )
                            .child(primary_button("intro-next", tr(next), p).on_click(
                                cx.listener(move |this, _, window, cx| {
                                    if permissions {
                                        this.finish_introduction(needs_model, window, cx);
                                    } else {
                                        this.intro = Some(Step::Permissions);
                                        cx.notify();
                                    }
                                }),
                            )),
                    ),
            )
    }

    fn intro_permission(&self, mic: bool, p: Palette, cx: &Context<Self>) -> Div {
        let (allowed, glyph, name, purpose) = if mic {
            (
                self.snapshot.mic_allowed,
                "mic",
                "ui.dictation.microphone",
                "ui.intro.mic",
            )
        } else {
            (
                self.snapshot.access_allowed,
                "cursor",
                "ui.permission.accessibility",
                "ui.intro.access",
            )
        };
        row()
            .w_full()
            .px(px(16.))
            .py(px(12.))
            .gap_3()
            .child(
                row()
                    .size(px(34.))
                    .flex_shrink_0()
                    .justify_center()
                    .rounded(px(8.))
                    .bg(p.soft)
                    .child(icon(glyph, p.accent, 18.)),
            )
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(label(tr(name), p.ink))
                    .child(text(tr(purpose), 12., p.muted)),
            )
            .child(self.permission(allowed, mic, p, cx))
    }
}

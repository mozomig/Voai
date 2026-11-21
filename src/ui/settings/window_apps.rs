use super::*;

pub(super) fn rows(ids: &[String]) -> Vec<(String, String)> {
    let mut apps: Vec<_> = ids
        .iter()
        .map(|id| {
            (
                crate::macos::application_name(id).unwrap_or_else(|| id.clone()),
                id.clone(),
            )
        })
        .collect();
    apps.sort_by_cached_key(|(name, _)| name.to_lowercase());
    apps
}

impl Settings {
    fn choose_window_app(&mut self, cx: &mut Context<Self>) {
        self.file_dialog_error = None;
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(tr("ui.window_apps.add").into()),
        });
        cx.spawn(async move |view, cx| {
            let result = picker.await;
            let _ = view.update(cx, |view, cx| {
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            view.send(Command::AddWindowApp(path));
                        }
                    }
                    Ok(Ok(None)) => (),
                    _ => view.file_dialog_error = Some("ui.dialog.open_failed"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn window_apps_page(&self, p: Palette, cx: &Context<Self>) -> Div {
        col()
            .gap_3()
            .child(text(tr("ui.window_apps.detail"), 13., p.muted))
            .child(
                row()
                    .justify_between()
                    .child(
                        button("window-apps-back", tr("ui.intro.back"), p)
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.navigate(Page::Dictation, window, cx)
                            })),
                    )
                    .child(
                        button("add-window-app", tr("ui.window_apps.add"), p)
                            .on_click(cx.listener(|this, _, _, cx| this.choose_window_app(cx))),
                    ),
            )
            .when_some(self.file_dialog_error, |v, err| {
                v.child(text(tr(err), 12., p.muted))
            })
            .child(
                col()
                    .border_1()
                    .border_color(p.rule)
                    .rounded(px(8.))
                    .overflow_hidden()
                    .child(
                        row()
                            .px_3()
                            .py_2()
                            .bg(p.side)
                            .child(text(tr("ui.window_apps.app"), 12., p.muted).flex_1())
                            .child(text(tr("ui.window_apps.insertion"), 12., p.muted).w(px(176.))),
                    )
                    .when(self.window_apps.is_empty(), |v| {
                        v.child(text(tr("ui.window_apps.empty"), 13., p.muted).p_3())
                    })
                    .children(self.window_apps.iter().map(|(name, id)| {
                        let bundle = id.clone();
                        row()
                            .px_3()
                            .py_2()
                            .gap_2()
                            .border_t_1()
                            .border_color(p.rule)
                            .child(
                                col()
                                    .min_w_0()
                                    .flex_1()
                                    .child(label(name.clone(), p.ink).truncate())
                                    .when(name != id, |v| {
                                        v.child(text(id.clone(), 11., p.muted).truncate())
                                    }),
                            )
                            .child(
                                col().w(px(176.)).items_start().child(
                                    button(
                                        SharedString::from(id.clone()),
                                        tr("ui.dictation.ask_again"),
                                        p,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, _| {
                                            this.send(Command::ForgetWindowApp(bundle.clone()))
                                        },
                                    )),
                                ),
                            )
                    })),
            )
    }
}

use super::*;

fn clock(ms: u64) -> String {
    let seconds = ms / 1000;
    if seconds < 3600 {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    } else {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    }
}

impl Settings {
    fn edit_speaker(&mut self, edit: SpeakerEdit, window: &mut Window, cx: &mut Context<Self>) {
        let initial = match edit {
            SpeakerEdit::New(_) => String::new(),
            SpeakerEdit::Rename(id) => self
                .snapshot
                .files
                .document
                .as_ref()
                .and_then(|doc| doc.speakers.get(&id))
                .cloned()
                .unwrap_or_default(),
        };
        self.files_ui.editing = Some(edit);
        self.files_ui.editor.update(cx, |input, cx| {
            input.set_value(initial, window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    pub(super) fn save_speaker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.files_ui.editor.read(cx).value();
        let name = value.trim().to_owned();
        if name.is_empty() {
            return;
        }
        let cmd = match self.files_ui.editing {
            Some(SpeakerEdit::New(line)) => FileCommand::AddSpeaker { line, name },
            Some(SpeakerEdit::Rename(speaker)) => FileCommand::Rename { speaker, name },
            None => return,
        };
        self.send(Command::File(cmd));
        if self.engine.borrow().snapshot().files.error.is_none() {
            self.files_ui.cancel_edit();
            window.focus(&self.focus);
            cx.notify();
        }
    }

    pub(super) fn speaker_editor(&self, edit: SpeakerEdit, p: Palette, cx: &Context<Self>) -> Div {
        col()
            .gap_1()
            .child(text(
                match edit {
                    SpeakerEdit::New(_) => "Новый спикер для этой реплики",
                    SpeakerEdit::Rename(_) => "Имя спикера во всей расшифровке",
                },
                12.,
                p.muted,
            ))
            .child(
                row()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.files_ui.editor).small().w_full()),
                    )
                    .child(
                        button("speaker-save", "Сохранить", p)
                            .disabled(self.files_ui.editor.read(cx).value().trim().is_empty())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.save_speaker(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("speaker-cancel")
                            .ghost()
                            .small()
                            .label("Отмена")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.files_ui.cancel_edit();
                                window.focus(&this.focus);
                                cx.notify();
                            })),
                    ),
            )
    }

    pub(super) fn file_line(
        &self,
        index: usize,
        doc: &Arc<FileDocument>,
        owner: &WeakEntity<Self>,
        p: Palette,
    ) -> Div {
        let playing = self.snapshot.files.playing == Some(index);
        let engine = &self.engine;
        let original_visible = self.files_ui.original == Some(index);
        let original_owner = owner.clone();
        let line = &doc.lines[index];
        let speaker = line.speaker;
        let name = speaker
            .and_then(|id| doc.speakers.get(&id))
            .map(String::as_str)
            .unwrap_or("Без спикера");
        let play_engine = engine.clone();
        let text_engine = engine.clone();
        let doc = doc.clone();
        let engine = engine.clone();
        let owner = owner.clone();
        let menu = Button::new(("file-speaker", index))
            .small()
            .ghost()
            .child(
                row()
                    .gap_1()
                    .child(text(name.to_owned(), 12., p.ink).max_w(px(180.)).truncate())
                    .child(icon("chevron", p.muted, 12.)),
            )
            .dropdown_menu(move |mut menu, _, _| {
                menu = menu
                    .scrollable(true)
                    .max_h(px(280.))
                    .label("Спикер реплики");
                for (&id, name) in &doc.speakers {
                    let engine = engine.clone();
                    menu = menu.item(
                        PopupMenuItem::new(name.clone())
                            .checked(speaker == Some(id))
                            .on_click(move |_, _, _| {
                                engine
                                    .borrow_mut()
                                    .dispatch(Command::File(FileCommand::Assign {
                                        line: index,
                                        speaker: Some(id),
                                    }))
                            }),
                    );
                }
                let clear_engine = engine.clone();
                let create = owner.clone();
                menu = menu
                    .item(
                        PopupMenuItem::new("Без спикера")
                            .checked(speaker.is_none())
                            .on_click(move |_, _, _| {
                                clear_engine.borrow_mut().dispatch(Command::File(
                                    FileCommand::Assign {
                                        line: index,
                                        speaker: None,
                                    },
                                ));
                            }),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("Новый спикер…").on_click(move |_, window, cx| {
                            let _ = create.update(cx, |this, cx| {
                                this.edit_speaker(SpeakerEdit::New(index), window, cx)
                            });
                        }),
                    );
                if let Some(id) = speaker {
                    let rename = owner.clone();
                    menu = menu.item(PopupMenuItem::new("Переименовать во всём файле…").on_click(
                        move |_, window, cx| {
                            let _ = rename.update(cx, |this, cx| {
                                this.edit_speaker(SpeakerEdit::Rename(id), window, cx)
                            });
                        },
                    ));
                }
                menu
            });
        col()
            .w_full()
            .py_2()
            .gap_1()
            .border_t_1()
            .border_color(p.rule)
            .child(
                row()
                    .gap_2()
                    .child(
                        Button::new(("file-time", index))
                            .small()
                            .ghost()
                            .text_color(p.accent)
                            .tooltip(if playing {
                                "Остановить"
                            } else {
                                "Слушать реплику"
                            })
                            .label(format!(
                                "{} {}",
                                if playing { "■" } else { "▶" },
                                clock(line.start_ms)
                            ))
                            .on_click(move |_, _, _| {
                                play_engine
                                    .borrow_mut()
                                    .dispatch(Command::File(FileCommand::Play(index)))
                            }),
                    )
                    .child(menu)
                    .when(line.original.is_some(), |r| {
                        r.child(
                            Button::new(("file-original", index))
                                .ghost()
                                .small()
                                .label(if original_visible {
                                    "Скрыть исходник"
                                } else {
                                    "Исходник"
                                })
                                .on_click(move |_, _, cx| {
                                    let _ = original_owner.update(cx, |view, cx| {
                                        let previous = view.files_ui.original.take();
                                        if previous != Some(index) {
                                            view.files_ui.original = Some(index);
                                        }
                                        for changed in
                                            [previous, view.files_ui.original].into_iter().flatten()
                                        {
                                            view.files_ui.list.splice(changed..changed + 1, 1);
                                        }
                                        cx.notify();
                                    });
                                }),
                        )
                    })
                    .when(line.review, |r| {
                        r.child(text("Проверить спикера", 11., p.muted))
                    }),
            )
            .child(
                div()
                    .id(("file-text", index))
                    .w_full()
                    .cursor_pointer()
                    .px_1()
                    .rounded_sm()
                    .when(playing, |d| d.bg(p.soft))
                    .hover(|d| d.bg(p.side))
                    .child(text(line.text.clone(), 14., p.ink))
                    .on_click(move |_, _, _| {
                        text_engine
                            .borrow_mut()
                            .dispatch(Command::File(FileCommand::Play(index)))
                    }),
            )
            .when(original_visible, |d| {
                d.children(line.original.as_ref().map(|s| {
                    text(s.clone(), 13., p.muted)
                        .pl_2()
                        .border_l_1()
                        .border_color(p.rule)
                }))
            })
    }
}

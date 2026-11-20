use super::*;
use crate::app::{ExportFormat, FileCommand, FileDocument, FileProgress, FileView};
use block2::RcBlock;
use gpui_component::scroll::ScrollableElement;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSModalResponseCancel, NSModalResponseOK, NSSavePanel};
use objc2_foundation::{NSString, NSURL};
use std::{path::Path, sync::Arc};

mod dialogue;
mod setup;

#[derive(Clone, Copy)]
enum SpeakerEdit {
    New(usize),
    Rename(u32),
}

pub(super) struct FileControls {
    configuring: bool,
    original: Option<usize>,
    timed: bool,
    paragraphs: Arc<[SharedString]>,
    list: ListState,
    editor: Entity<InputState>,
    editing: Option<SpeakerEdit>,
    dialog_error: Option<&'static str>,
}

impl FileControls {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Settings>) -> Self {
        let editor = cx.new(|cx| InputState::new(window, cx).placeholder("Имя спикера"));
        cx.subscribe_in(&editor, window, |this, _, event, window, cx| match event {
            InputEvent::PressEnter { .. } => this.save_speaker(window, cx),
            InputEvent::Change => cx.notify(),
            _ => {}
        })
        .detach();
        Self {
            configuring: false,
            original: None,
            timed: false,
            paragraphs: Default::default(),
            list: ListState::new(0, ListAlignment::Top, px(160.)),
            editor,
            editing: None,
            dialog_error: None,
        }
    }

    pub(super) fn sync(&mut self, previous: &FileView, next: &FileView) {
        if let Some(doc) = &next.document
            && previous
                .document
                .as_ref()
                .is_none_or(|old| !Arc::ptr_eq(old, doc))
        {
            self.paragraphs = if doc.speakers.is_empty() {
                doc.paragraphs()
                    .into_iter()
                    .map(SharedString::from)
                    .collect()
            } else {
                Default::default()
            };
        }
        if previous.document.as_ref().map(|doc| &doc.id)
            != next.document.as_ref().map(|doc| &doc.id)
        {
            self.leave();
            self.configuring = false;
            self.timed = false;
            self.list
                .reset(next.document.as_ref().map_or(0, |doc| doc.lines.len()));
        }
    }

    pub(super) fn configure(&mut self) {
        self.leave();
        self.configuring = true;
    }

    pub(super) fn cancel_edit(&mut self) -> bool {
        self.editing.take().is_some()
    }

    pub(super) fn leave(&mut self) {
        self.original = None;
        self.editing = None;
        self.dialog_error = None;
    }
}

impl Settings {
    fn import_media(&mut self, cx: &mut Context<Self>) {
        self.files_ui.leave();
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Расшифровать".into()),
        });
        cx.spawn(async move |view, cx| {
            let result = picker.await;
            let _ = view.update(cx, |view, cx| {
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            view.send(Command::File(FileCommand::Import(path)));
                        }
                    }
                    Ok(Ok(None)) => {}
                    _ => view.files_ui.dialog_error = Some("Не удалось открыть окно выбора файла."),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn export_media(&mut self, format: ExportFormat, cx: &mut Context<Self>) {
        let Some(doc) = self.snapshot.files.document.as_ref() else {
            return;
        };
        let id = doc.id.clone();
        let stem = doc.source.file_stem().unwrap_or_default().to_string_lossy();
        let extension = match format {
            ExportFormat::Txt => "txt",
            ExportFormat::Srt => "srt",
        };
        let suggested = format!("{stem}.{extension}");
        let directory = doc.source.parent().unwrap_or(Path::new("/"));
        self.files_ui.dialog_error = None;
        let Some(main_thread) = MainThreadMarker::new() else {
            return;
        };
        // GPUI 0.2 normalizes some double extensions after the overwrite confirmation.
        // Export must use the exact URL the user approved, even for names with several dots.
        let panel = NSSavePanel::savePanel(main_thread);
        panel.setDirectoryURL(NSURL::from_directory_path(directory).as_deref());
        panel.setNameFieldStringValue(&NSString::from_str(&suggested));
        let confirmed = panel.clone();
        let (sender, receiver) = async_channel::bounded(1);
        panel.beginWithCompletionHandler(&RcBlock::new(move |response| {
            let result = if response == NSModalResponseOK {
                confirmed
                    .URL()
                    .filter(|url| url.isFileURL())
                    .and_then(|url| url.to_file_path())
                    .map(Some)
                    .ok_or(())
            } else if response == NSModalResponseCancel {
                Ok(None)
            } else {
                Err(())
            };
            let _ = sender.try_send(result);
        }));
        cx.spawn(async move |view, cx| {
            let result = receiver.recv().await;
            let _ = view.update(cx, |view, cx| {
                match result {
                    Ok(Ok(Some(path)))
                        if view
                            .engine
                            .borrow()
                            .snapshot()
                            .files
                            .document
                            .as_ref()
                            .is_some_and(|d| d.id == id) =>
                    {
                        view.send(Command::File(FileCommand::Export { path, format }));
                    }
                    Ok(Ok(_)) => {}
                    _ => view.files_ui.dialog_error = Some("Не удалось открыть окно сохранения."),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn file_progress(
        &self,
        progress: &FileProgress,
        p: Palette,
        cx: &Context<Self>,
    ) -> Div {
        col()
            .mt_2()
            .gap_2()
            .child(
                row()
                    .gap_2()
                    .child(text(progress.stage.clone(), 14., p.ink).flex_1())
                    .child(text(format!("{}%", progress.percent), 12., p.muted))
                    .when(progress.cancellable, |r| {
                        r.child(button("file-cancel", "Отменить", p).on_click(cx.listener(
                            |this, _, _, _| this.send(Command::File(FileCommand::Cancel)),
                        )))
                    }),
            )
            .child(track(f32::from(progress.percent) / 100., p))
    }

    pub(super) fn files(&self, p: Palette, cx: &Context<Self>) -> Div {
        let files = &self.snapshot.files;
        let mut body = col()
            .flex_1()
            .min_h_0()
            .w_full()
            .gap(px(10.))
            .children(files.error.as_ref().map(|e| text(e.clone(), 12., p.error)))
            .children(self.files_ui.dialog_error.map(|e| text(e, 12., p.error)))
            .children(
                files
                    .exported
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .map(|name| {
                        text(
                            format!("Сохранено · {}", name.to_string_lossy()),
                            12.,
                            p.muted,
                        )
                    }),
            );
        if let Some(progress) = &files.progress {
            return body.child(self.file_progress(progress, p, cx));
        }
        let configuring = files.document.is_none() || self.files_ui.configuring;
        let mut actions = row().gap_2();
        if !configuring {
            actions = actions.child(button("file-new", "Новая расшифровка", p).on_click(
                cx.listener(|this, _, _, cx| {
                    this.send(Command::File(FileCommand::Stop));
                    this.files_ui.configure();
                    cx.notify();
                }),
            ));
        }
        if !files.library.is_empty() {
            let library = files.library.clone();
            let selected = files.document.as_ref().map(|d| d.id.clone());
            let owner = cx.entity().downgrade();
            actions = actions.child(button("file-library", "Сохранённые ▾", p).dropdown_menu(
                move |mut menu, _, _| {
                    menu = menu.scrollable(true).max_h(px(280.)).max_w(px(320.));
                    for doc in library.iter() {
                        let id = doc.id.clone();
                        let owner = owner.clone();
                        menu = menu.item(
                            PopupMenuItem::new(doc.title.clone())
                                .checked(selected.as_ref() == Some(&id))
                                .on_click(move |_, _, cx| {
                                    let _ = owner.update(cx, |view, cx| {
                                        view.files_ui.configuring = false;
                                        view.send(Command::File(FileCommand::Open(id.clone())));
                                        cx.notify();
                                    });
                                }),
                        );
                    }
                    menu
                },
            ));
        }
        if !files.library.is_empty() || !configuring {
            body = body.child(actions);
        }
        if configuring {
            return body.child(self.file_setup(p, cx));
        }
        let Some(doc) = files.document.as_ref() else {
            return body;
        };
        let plain = doc.speakers.is_empty() && !self.files_ui.timed;
        let owner = cx.entity().downgrade();
        let export = button("file-export", "Экспорт ▾", p).dropdown_menu(move |mut menu, _, _| {
            for (label, format) in [
                ("Текст (.txt)", ExportFormat::Txt),
                ("Субтитры (.srt)", ExportFormat::Srt),
            ] {
                let owner = owner.clone();
                menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                    let _ = owner.update(cx, |this, cx| this.export_media(format, cx));
                }));
            }
            menu
        });
        body = body
            .child(
                row()
                    .w_full()
                    .gap_2()
                    .mt_1()
                    .child(
                        text(doc.title.clone(), 16., p.ink)
                            .flex_1()
                            .min_w_0()
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .when(doc.speakers.is_empty() && !doc.lines.is_empty(), |r| {
                        r.child(
                            button(
                                "file-timed",
                                if plain {
                                    "По фрагментам"
                                } else {
                                    "Текст"
                                },
                                p,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.files_ui.timed = !this.files_ui.timed;
                                this.files_ui.leave();
                                this.send(Command::File(FileCommand::Stop));
                                this.files_ui.list.reset(0);
                                cx.notify();
                            })),
                        )
                    })
                    .when(!doc.lines.is_empty(), |r| r.child(export)),
            )
            .children(doc.warning.as_ref().map(|w| text(w.clone(), 12., p.muted)))
            .children(
                self.files_ui
                    .editing
                    .map(|edit| self.speaker_editor(edit, p, cx)),
            );
        if doc.lines.is_empty() {
            return body.child(text("В записи не удалось распознать речь.", 13., p.muted));
        }
        let owner = cx.entity().downgrade();
        let doc = doc.clone();
        let paragraphs = self.files_ui.paragraphs.clone();
        let count = if plain {
            paragraphs.len()
        } else {
            doc.lines.len()
        };
        if self.files_ui.list.item_count() != count {
            self.files_ui.list.reset(count);
        }
        let rows = gpui::list(self.files_ui.list.clone(), move |index, _, cx| {
            if plain {
                return text(paragraphs[index].clone(), 15., p.ink)
                    .w_full()
                    .line_height(px(23.))
                    .pb_3()
                    .into_any_element();
            }
            let Some(view) = owner.upgrade() else {
                return div().into_any_element();
            };
            view.read(cx)
                .file_line(index, &doc, &owner, p)
                .into_any_element()
        });
        body.child(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(rows.size_full())
                .vertical_scrollbar(&self.files_ui.list),
        )
    }
}

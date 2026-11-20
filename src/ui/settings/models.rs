use super::*;
use crate::app::FileCommand;
use voai_l10n::{tf, tr};

type ModelCommand = fn(String) -> Command;

impl Settings {
    pub(super) fn models(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        let s = &self.snapshot;
        let speakers = self.model_tab == ModelTab::Speakers;
        let tabs = [ModelTab::Speech, ModelTab::Text, ModelTab::Speakers];
        let mut page = col()
            .child(
                row()
                    .flex_wrap()
                    .justify_between()
                    .gap_2()
                    .child(self.tabs(
                        p,
                        &[
                            (tr("ui.models.speech"), self.model_tab == ModelTab::Speech),
                            (tr("ui.models.text"), self.model_tab == ModelTab::Text),
                            (tr("ui.files.speakers"), speakers),
                        ],
                        move |this, i, cx| {
                            this.model_tab = tabs[i];
                            this.file_dialog_error = None;
                            cx.notify();
                        },
                        cx,
                    ))
                    .child(
                        primary_button(
                            "attach-model",
                            if speakers {
                                tr("ui.model.link_folder")
                            } else if s.connecting_model {
                                tr("ui.model.checking")
                            } else {
                                tr("ui.model.link_file")
                            },
                            p,
                        )
                        .disabled(if speakers {
                            s.files.progress.is_some()
                        } else {
                            s.connecting_model
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.choose_file(cx))),
                    ),
            )
            .children(
                self.file_dialog_error
                    .map(|key| text(tr(key), 12., p.error).mt_2()),
            );
        if speakers {
            page = page.child(self.voice_models(p, cx));
            return page;
        }
        let text_models = self.model_tab == ModelTab::Text;
        let local = s
            .models
            .iter()
            .filter(|m| m.text_model == text_models && m.local_path.is_some());
        let occupied = s
            .models
            .iter()
            .filter(|m| m.local_path.is_none())
            .map(|m| m.on_disk)
            .sum();
        page.when(local.clone().next().is_some(), |d| {
            d.child(group_title(tr("ui.model.linked_files"), p))
                .children(local.map(|m| self.model_row(m, p, cx)))
        })
        .child(group_title(tr("ui.model.catalogue"), p))
        .children(
            s.models
                .iter()
                .filter(|m| m.text_model == text_models && m.local_path.is_none())
                .map(|m| self.model_row(m, p, cx)),
        )
        .when(occupied > 0, |d| {
            d.child(
                row()
                    .mt_4()
                    .gap_2()
                    .child(icon("disk", p.muted, 14.))
                    .child(text(
                        tf!("ui.models.disk", size = bytes(occupied)),
                        12.,
                        p.muted,
                    )),
            )
        })
    }

    fn choose_file(&mut self, cx: &mut Context<Self>) {
        let tab = self.model_tab;
        let speakers = tab == ModelTab::Speakers;
        self.file_dialog_error = None;
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: !speakers,
            directories: speakers,
            multiple: false,
            prompt: Some(
                match tab {
                    ModelTab::Speech => tr("ui.model.speech_file"),
                    ModelTab::Text => tr("ui.model.gguf_file"),
                    ModelTab::Speakers => tr("ui.model.choose_folder"),
                }
                .into(),
            ),
        });
        cx.spawn(async move |view, cx| {
            let selected = picker.await;
            let _ = view.update(cx, |view, cx| {
                match selected {
                    Ok(Ok(Some(files))) => {
                        if let Some(path) = files.into_iter().next() {
                            view.send(if speakers {
                                Command::File(FileCommand::ConnectVoices(path))
                            } else {
                                Command::ConnectModel {
                                    path,
                                    text_model: tab == ModelTab::Text,
                                }
                            });
                        }
                    }
                    Ok(Ok(None)) => {}
                    _ => view.file_dialog_error = Some("ui.dialog.open_failed"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn voice_models(&self, p: Palette, cx: &Context<Self>) -> Div {
        let files = &self.snapshot.files;
        let busy = files.progress.is_some();
        let selected = files.voices_downloaded && !files.using_local_voices && files.voices_ready;
        let action = if selected {
            tr("ui.model.chosen")
        } else if files.voices_downloaded {
            tr("ui.action.select")
        } else {
            tr("ui.model.download")
        };
        let download = button("download-speakers", action, p)
            .disabled(selected || busy)
            .on_click(cx.listener(|this, _, _, _| {
                let choice = if this.snapshot.files.voices_downloaded {
                    FileCommand::UseDownloadedVoices
                } else {
                    FileCommand::InstallVoices
                };
                this.send(Command::File(choice));
            }));
        let mut panel = col()
            .gap_1()
            .child(group_title(tr("ui.models.diarization"), p))
            .child(Self::setting(
                p,
                "Pyannote 3.0 + CAM++",
                tr("ui.models.diarization_size"),
                download,
            ))
            .child(text(tr("ui.models.diarization_files"), 12., p.muted).mt_2());
        if let Some(path) = &files.voices_local {
            let active = files.using_local_voices && files.voices_ready;
            panel = panel.child(Self::setting(
                p,
                tr("ui.model.linked_folder"),
                &path.to_string_lossy(),
                button(
                    "use-local-speakers",
                    if active {
                        tr("ui.model.chosen")
                    } else {
                        tr("ui.action.select")
                    },
                    p,
                )
                .disabled(busy || active)
                .on_click(cx.listener(|this, _, _, _| {
                    this.send(Command::File(FileCommand::UseLocalVoices))
                })),
            ));
        }
        if let Some(progress) = &files.progress {
            panel = panel.child(self.file_progress(progress, p, cx));
        }
        panel.children(files.error.as_ref().map(|e| text(e.clone(), 12., p.error)))
    }

    fn model_row(&self, m: &ModelRow, p: Palette, cx: &Context<Self>) -> Div {
        let local = m.local_path.is_some();
        let description = m.description.clone();
        let summary = if local || matches!(m.state, ModelState::Downloading { .. }) {
            m.description.clone()
        } else {
            format!("{} · {}", bytes(m.bytes), m.description)
        };
        let mut details = col()
            .flex_1()
            .min_w_0()
            .gap(px(3.))
            .child(
                row()
                    .gap(px(7.))
                    .when(local, |r| r.child(icon("file", p.muted, 15.)))
                    .child(label(m.name.clone(), p.ink).truncate()),
            )
            .child(
                text(summary, 12., p.muted)
                    .when(local, |d| d.truncate())
                    .id(SharedString::from(format!("model-detail-{}", m.id)))
                    .when(local, |d| {
                        d.tooltip(move |window, cx| {
                            gpui_component::tooltip::Tooltip::new(description.clone())
                                .build(window, cx)
                        })
                    }),
            );
        let mut controls = row().gap_1().flex_shrink_0();
        let mut action: Option<(&str, ModelCommand)> = None;
        match &m.state {
            ModelState::Selected => {
                controls = controls.child(
                    row()
                        .gap_1()
                        .child(icon("check", p.accent, 14.))
                        .child(text(tr("ui.model.selected"), 12., p.accent)),
                )
            }
            ModelState::Available => action = Some((tr("ui.model.download"), Command::Download)),
            ModelState::Installed => action = Some((tr("ui.action.select"), Command::SelectModel)),
            ModelState::Failed(reason) => {
                details = details.child(text(reason.clone(), 12., p.error));
                action = Some((
                    if m.on_disk > 0 {
                        tr("ui.model.resume")
                    } else {
                        tr("ui.action.retry")
                    },
                    Command::Download,
                ));
            }
            ModelState::Unavailable(reason) => {
                details = details.child(text(reason.clone(), 12., p.error));
            }
            ModelState::Downloading { received } => {
                let fraction = *received as f32 / m.bytes.max(1) as f32;
                details = details
                    .child(div().pt_1().child(track(fraction, p)))
                    .child(text(
                        tf!(
                            "ui.model.download_progress",
                            received = bytes(*received),
                            total = bytes(m.bytes),
                            percent = format!("{:.0}", 100. * fraction)
                        ),
                        11.,
                        p.muted,
                    ));
                action = Some((tr("ui.action.cancel_progress"), Command::CancelDownload));
            }
        }
        if local
            && !matches!(
                &m.state,
                ModelState::Downloading { .. } | ModelState::Unavailable(_)
            )
        {
            details = details.child(text(bytes(m.bytes), 11., p.muted));
        }
        if let Some((title, cmd)) = action {
            let id = m.id.clone();
            controls = controls.child(
                button(
                    SharedString::from(format!("model-{id}")),
                    title.to_owned(),
                    p,
                )
                .on_click(cx.listener(move |this, _, _, _| this.send(cmd(id.clone())))),
            );
        }
        if local {
            let id = m.id.clone();
            controls = controls.child(
                Button::new(SharedString::from(format!("unlink-{id}")))
                    .ghost()
                    .small()
                    .label(tr("ui.action.disconnect"))
                    .text_color(p.muted)
                    .on_click(cx.listener(move |this, _, _, _| {
                        this.send(Command::DisconnectModel(id.clone()))
                    })),
            );
        }
        row()
            .py(px(12.))
            .w_full()
            .gap_3()
            .border_b_1()
            .border_color(p.rule)
            .child(details)
            .child(controls)
    }
}

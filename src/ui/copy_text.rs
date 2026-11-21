use gpui::{prelude::*, *};
use gpui_component::{
    Disableable, IconName, Sizable,
    button::{Button, ButtonVariants},
};
use std::time::Duration;

#[derive(IntoElement)]
pub(super) struct CopyText {
    id: ElementId,
    text: SharedString,
    hint: &'static str,
}

impl CopyText {
    pub(super) fn new(id: impl Into<ElementId>, text: &str, hint: &'static str) -> Self {
        Self {
            id: id.into(),
            text: text.to_owned().into(),
            hint,
        }
    }
}

impl RenderOnce for CopyText {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let feedback = window.use_keyed_state(self.id.clone(), cx, |_, _| None::<Task<()>>);
        let copied = feedback.read(cx).is_some();
        Button::new(self.id)
            .small()
            .ghost()
            .icon(if copied {
                IconName::Check
            } else {
                IconName::Copy
            })
            .tooltip(self.hint)
            .disabled(self.text.trim().is_empty())
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                cx.write_to_clipboard(ClipboardItem::new_string(self.text.to_string()));
                feedback.update(cx, |reset, cx| {
                    *reset = Some(cx.spawn(async move |state, cx| {
                        cx.background_executor().timer(Duration::from_secs(1)).await;
                        let _ = state.update(cx, |reset, cx| {
                            *reset = None;
                            cx.notify();
                        });
                    }));
                    cx.notify();
                });
            })
    }
}

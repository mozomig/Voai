use std::borrow::Cow;
use voai_l10n::{tf, tr};

use gpui::{prelude::*, *};
use gpui_component::{Sizable, button::Button};

pub struct Artwork;

impl AssetSource for Artwork {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let data: &'static [u8] = match path {
            "history" => include_bytes!("icons/history.svg"),
            "mic" => include_bytes!("icons/mic.svg"),
            "models" => include_bytes!("icons/models.svg"),
            "sliders" => include_bytes!("icons/sliders.svg"),
            "arrow" => include_bytes!("icons/arrow.svg"),
            "chevron" => include_bytes!("icons/chevron.svg"),
            "file" => include_bytes!("icons/file.svg"),
            "telegram" => include_bytes!("icons/telegram.svg"),
            "disk" => include_bytes!("icons/disk.svg"),
            "check" | "icons/check.svg" => include_bytes!("icons/check.svg"),
            _ => return Ok(None),
        };
        Ok(Some(Cow::Borrowed(data)))
    }

    fn list(&self, _: &str) -> Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub paper: Hsla,
    pub side: Hsla,
    pub ink: Hsla,
    pub muted: Hsla,
    pub accent: Hsla,
    pub error: Hsla,
    pub soft: Hsla,
    pub rule: Hsla,
    pub surface: Hsla,
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            paper: rgb(0x0d1117).into(),
            side: rgb(0x161b22).into(),
            surface: rgb(0x21262d).into(),
            ink: rgb(0xe6edf3).into(),
            muted: rgb(0x9ba7b4).into(),
            accent: rgb(0x58a6ff).into(),
            error: rgb(0xffa198).into(),
            soft: rgb(0x172c44).into(),
            rule: rgb(0x30363d).into(),
        }
    }

    pub fn apply(self, cx: &mut App) {
        use gpui_component::{Theme, ThemeMode};
        Theme::change(ThemeMode::Dark, None, cx);
        let theme = Theme::global_mut(cx);
        theme.background = self.paper;
        theme.foreground = self.ink;
        theme.border = self.rule;
        theme.input = self.rule;
        theme.muted = self.surface;
        theme.muted_foreground = self.muted;
        theme.popover = self.side;
        theme.popover_foreground = self.ink;
        theme.primary = self.accent;
        theme.primary_foreground = self.paper;
        theme.accent = self.soft;
        theme.accent_foreground = self.ink;
        theme.ring = self.accent;
        theme.selection = self.accent.opacity(0.27);
        theme.secondary = self.surface;
        theme.secondary_foreground = self.ink;
        theme.title_bar = self.side;
        theme.title_bar_border = self.rule;
    }
}

pub fn icon(path: &'static str, color: Hsla, width: f32) -> Svg {
    svg().path(path).size(px(width)).text_color(color)
}

pub fn row() -> Div {
    div().flex().items_center()
}

pub fn col() -> Div {
    div().flex().flex_col()
}

pub fn text(value: impl Into<SharedString>, font_size: f32, color: Hsla) -> Div {
    div()
        .text_size(px(font_size))
        .text_color(color)
        .child(value.into())
}

pub fn label(value: impl Into<SharedString>, color: Hsla) -> Div {
    text(value, 13., color).font_weight(FontWeight::MEDIUM)
}

pub fn mark(color: Hsla) -> Div {
    row().gap(px(2.)).children(
        [6., 13., 20., 11., 5.]
            .map(|height| div().w(px(3.)).h(px(height)).rounded_full().bg(color)),
    )
}

pub fn button(id: impl Into<ElementId>, title: impl Into<SharedString>, p: Palette) -> Button {
    Button::new(id)
        .small()
        .label(title)
        .rounded(px(6.))
        .text_size(px(12.))
        .text_color(p.ink)
        .bg(p.surface)
        .border_1()
        .border_color(p.rule)
}

pub fn primary_button(id: &'static str, title: &'static str, p: Palette) -> Button {
    button(id, title, p)
        .bg(p.soft)
        .border_color(p.accent.opacity(0.45))
        .text_color(p.accent)
}

pub fn track(fraction: f32, p: Palette) -> Div {
    div()
        .w_full()
        .h(px(4.))
        .rounded_full()
        .bg(p.rule)
        .overflow_hidden()
        .child(
            div()
                .w(relative(fraction.clamp(0., 1.)))
                .h_full()
                .rounded_full()
                .bg(p.accent),
        )
}

pub fn bytes(size: u64) -> String {
    if size >= 1_000_000_000 {
        let mut number = format!("{:.2}", size as f64 / 1e9);
        if voai_l10n::language() == "ru" {
            number = number.replace('.', ",");
        }
        tf!("ui.size.gb", size = number)
    } else {
        tf!("ui.size.mb", size = size.div_ceil(1_000_000))
    }
}

pub fn shortcut_label(value: &str) -> String {
    if value == crate::app::right_command::SHORTCUT {
        return tr("ui.shortcut.right_command").into();
    }
    value
        .split('+')
        .map(|part| match part.to_ascii_lowercase().as_str() {
            "super" | "cmd" | "command" | "meta" => "⌘".into(),
            "option" | "alt" => "⌥".into(),
            "ctrl" | "control" => "⌃".into(),
            "shift" => "⇧".into(),
            "space" => tr("ui.shortcut.space").into(),
            _ => part.strip_prefix("Key").unwrap_or(part).to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

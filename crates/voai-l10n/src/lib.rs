//! Shared copy for the desktop app and its local workers.
use serde::{Deserialize, Serialize};
use std::sync::{
    LazyLock,
    atomic::{AtomicBool, Ordering},
};

rust_i18n::i18n!("locales", fallback = "en");

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InterfaceLanguage {
    #[default]
    System,
    #[serde(rename = "en")]
    English,
    #[serde(rename = "ru")]
    Russian,
}

impl InterfaceLanguage {
    pub fn resolve(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Russian => "ru",
            Self::System => preferred(sys_locale::get_locales()),
        }
    }

    pub fn apply(self) {
        RUSSIAN.store(self.resolve() == "ru", Ordering::Relaxed);
    }
}

fn preferred(tags: impl IntoIterator<Item = impl AsRef<str>>) -> &'static str {
    for tag in tags {
        let base = tag.as_ref().split(['-', '_']).next().unwrap_or_default();
        if base.eq_ignore_ascii_case("ru") {
            return "ru";
        }
        if base.eq_ignore_ascii_case("en") {
            return "en";
        }
    }
    "en"
}

// GUI launch applies the saved preference. Child processes inherit a snapshot
// explicitly on their Command, so launching a worker never mutates the process environment.
pub const WORKER_LANGUAGE_ENV: &str = "VOAI_UI_LANGUAGE";
static RUSSIAN: LazyLock<AtomicBool> = LazyLock::new(|| {
    let ru = match std::env::var(WORKER_LANGUAGE_ENV).as_deref() {
        Ok("ru") => true,
        Ok("en") => false,
        _ => InterfaceLanguage::System.resolve() == "ru",
    };
    AtomicBool::new(ru)
});

pub fn language() -> &'static str {
    if RUSSIAN.load(Ordering::Relaxed) {
        "ru"
    } else {
        "en"
    }
}

pub fn tr_for(locale: &str, key: &'static str) -> &'static str {
    match rust_i18n::t!(key, locale = locale) {
        std::borrow::Cow::Borrowed(text) => text,
        // All catalog entries are static. A missing key should remain recognizable.
        std::borrow::Cow::Owned(_) => key,
    }
}

pub fn tr(key: &'static str) -> &'static str {
    tr_for(language(), key)
}

#[doc(hidden)]
pub use rust_i18n::replace_patterns;

#[macro_export]
macro_rules! tf {
    ($key:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::replace_patterns($crate::tr($key), &[$(stringify!($name)),+], &[$(($value).to_string()),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respects_the_order_of_supported_system_languages() {
        assert_eq!(preferred(["de-DE", "RU_ru", "en-US"]), "ru");
        assert_eq!(preferred(["en-GB", "ru"]), "en");
        assert_eq!(preferred(["fr-CA"]), "en");
        assert_eq!(preferred(Vec::<String>::new()), "en");
    }
}

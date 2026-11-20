use std::{collections::BTreeSet, fs, path::Path};

fn fields(template: &str) -> BTreeSet<&str> {
    template
        .split("%{")
        .skip(1)
        .map(|s| s.split_once('}').expect("unclosed placeholder").0)
        .collect()
}

#[test]
fn every_message_is_bilingual_and_has_matching_placeholders() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("locales");
    let mut keys = BTreeSet::new();
    for entry in fs::read_dir(dir).unwrap() {
        let file = entry.unwrap().path();
        let messages: serde_yaml::Mapping =
            serde_yaml::from_slice(&fs::read(file).unwrap()).unwrap();
        for (key, translations) in messages {
            let key = key.as_str().unwrap();
            if key == "_version" {
                continue;
            }
            assert!(keys.insert(key.to_owned()), "duplicate message: {key}");
            let en = translations["en"].as_str().unwrap();
            let ru = translations["ru"].as_str().unwrap();
            assert!(!en.trim().is_empty() && !ru.trim().is_empty(), "{key}");
            assert_eq!(fields(en), fields(ru), "{key}");
        }
    }
    assert!(keys.contains("prompt.default"));
}

#[test]
fn resolves_catalog_entries_in_both_languages_and_uses_english_as_fallback() {
    assert_eq!(voai_l10n::tr_for("en", "ui.menu.quit"), "Quit Voai");
    assert_eq!(voai_l10n::tr_for("ru", "ui.menu.quit"), "Выйти из Voai");
    assert_eq!(voai_l10n::tr_for("de", "ui.menu.quit"), "Quit Voai");
}

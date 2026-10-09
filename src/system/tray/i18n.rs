//! Translations for the tray menu.
//!
//! The tray menu is not drawn by the UI process: the daemon publishes its
//! labels over `com.canonical.dbusmenu` and the desktop shell renders them
//! verbatim. The Qt catalogs installed in `waywallen-ui` therefore never
//! apply to it, and the labels have to be resolved here instead.
//!
//! Lookups run against a compile-time table picked from the process locale,
//! so a missing language or a missing entry simply yields the English source
//! string.

use std::sync::OnceLock;

/// One language's messages as `(msgid, msgstr)` pairs, sorted by `msgid`.
type Catalog = &'static [(&'static str, &'static str)];

const DE: Catalog = &[
    ("1 hour", "1 Stunde"),
    ("1 minute", "1 Minute"),
    ("15 minutes", "15 Minuten"),
    ("30 seconds", "30 Sekunden"),
    ("5 minutes", "5 Minuten"),
    ("Linux wallpaper daemon", "Hintergrundbild-Daemon für Linux"),
    ("Mute", "Stummschalten"),
    ("Next", "Nächstes Hintergrundbild"),
    ("Off", "Aus"),
    ("Open UI", "Oberfläche öffnen"),
    ("Pause", "Anhalten"),
    ("Previous", "Vorheriges Hintergrundbild"),
    ("Quit", "Beenden"),
    ("Rescan wallpapers", "Hintergrundbilder neu einlesen"),
    ("Resume", "Fortsetzen"),
    ("Rotate", "Automatisch wechseln"),
    ("Shuffle", "Mischen"),
    ("Unmute", "Stummschaltung aufheben"),
];

const RU: Catalog = &[
    ("1 hour", "1 час"),
    ("1 minute", "1 минута"),
    ("15 minutes", "15 минут"),
    ("30 seconds", "30 секунд"),
    ("5 minutes", "5 минут"),
    ("Linux wallpaper daemon", "Демон обоев для Linux"),
    ("Mute", "Заглушить"),
    ("Next", "Следующие обои"),
    ("Off", "Выкл."),
    ("Open UI", "Открыть интерфейс"),
    ("Pause", "Пауза"),
    ("Previous", "Предыдущие обои"),
    ("Quit", "Выход"),
    ("Rescan wallpapers", "Пересканировать обои"),
    ("Resume", "Возобновить"),
    ("Rotate", "Смена обоев"),
    ("Shuffle", "Вперемешку"),
    ("Unmute", "Включить звук"),
];

fn catalog_for(language: &str) -> Option<Catalog> {
    match language {
        "de" => Some(DE),
        "ru" => Some(RU),
        _ => None,
    }
}

/// Primary language subtag of the active locale, lowercased: `ru_RU.UTF-8`
/// and `ru` both yield `ru`. `C` and `POSIX` mean "no translation".
fn locale_language() -> Option<String> {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        let value = match std::env::var(key) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if value.is_empty() {
            continue;
        }
        if value == "C" || value == "POSIX" {
            return None;
        }
        let tag = value
            .split(['.', '@'])
            .next()
            .unwrap_or_default()
            .split(['_', '-'])
            .next()
            .unwrap_or_default();
        if !tag.is_empty() {
            return Some(tag.to_ascii_lowercase());
        }
    }
    None
}

fn active_catalog() -> Option<Catalog> {
    static ACTIVE: OnceLock<Option<Catalog>> = OnceLock::new();
    *ACTIVE.get_or_init(|| locale_language().as_deref().and_then(catalog_for))
}

/// Translate a tray label, falling back to the English source string.
pub fn tr(msgid: &'static str) -> &'static str {
    let Some(catalog) = active_catalog() else {
        return msgid;
    };
    match catalog.binary_search_by_key(&msgid, |(id, _)| *id) {
        Ok(index) => catalog[index].1,
        Err(_) => msgid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogs_are_sorted_and_complete() {
        for catalog in [DE, RU] {
            assert!(
                catalog.windows(2).all(|w| w[0].0 < w[1].0),
                "catalog entries must be sorted by msgid for binary_search"
            );
            assert!(catalog.iter().all(|(_, text)| !text.is_empty()));
        }
    }

    #[test]
    fn lookup_falls_back_to_the_source_string() {
        assert_eq!(
            RU.binary_search_by_key(&"Rescan wallpapers", |(id, _)| *id)
                .map(|i| RU[i].1),
            Ok("Пересканировать обои")
        );
        assert!(RU
            .binary_search_by_key(&"not a tray label", |(id, _)| *id)
            .is_err());
    }

    #[test]
    fn catalog_is_selected_by_primary_subtag() {
        assert!(catalog_for("de").is_some());
        assert!(catalog_for("ru").is_some());
        assert!(catalog_for("en").is_none());
    }
}

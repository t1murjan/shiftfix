//! Классы клавиш, таблицы раскладок и перекодировка текста.
//!
//! Таблица раскладки — какой символ даёт каждая клавиша без Shift и с Shift.
//! Обычно таблицы строятся из настоящей keymap (модуль `xkb`), встроенные
//! us/ru — запасной вариант.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

use anyhow::{Result, anyhow};
use evdev::KeyCode;

use crate::engine::Stroke;

/// Клавиши основного блока: цифровой ряд, три буквенных ряда и ISO-клавиша
/// между левым Shift и Z.
#[rustfmt::skip]
pub const ROW_KEYS: [KeyCode; 48] = [
    KeyCode::KEY_GRAVE, KeyCode::KEY_1, KeyCode::KEY_2, KeyCode::KEY_3, KeyCode::KEY_4,
    KeyCode::KEY_5, KeyCode::KEY_6, KeyCode::KEY_7, KeyCode::KEY_8, KeyCode::KEY_9,
    KeyCode::KEY_0, KeyCode::KEY_MINUS, KeyCode::KEY_EQUAL,
    KeyCode::KEY_Q, KeyCode::KEY_W, KeyCode::KEY_E, KeyCode::KEY_R, KeyCode::KEY_T,
    KeyCode::KEY_Y, KeyCode::KEY_U, KeyCode::KEY_I, KeyCode::KEY_O, KeyCode::KEY_P,
    KeyCode::KEY_LEFTBRACE, KeyCode::KEY_RIGHTBRACE, KeyCode::KEY_BACKSLASH,
    KeyCode::KEY_A, KeyCode::KEY_S, KeyCode::KEY_D, KeyCode::KEY_F, KeyCode::KEY_G,
    KeyCode::KEY_H, KeyCode::KEY_J, KeyCode::KEY_K, KeyCode::KEY_L,
    KeyCode::KEY_SEMICOLON, KeyCode::KEY_APOSTROPHE,
    KeyCode::KEY_Z, KeyCode::KEY_X, KeyCode::KEY_C, KeyCode::KEY_V, KeyCode::KEY_B,
    KeyCode::KEY_N, KeyCode::KEY_M, KeyCode::KEY_COMMA, KeyCode::KEY_DOT, KeyCode::KEY_SLASH,
    KeyCode::KEY_102ND,
];

/// Встроенные таблицы: символы без Shift и с Shift в порядке `ROW_KEYS`.
const BUILTIN_ROWS: [(&str, &str, &str); 2] = [
    (
        "us",
        "`1234567890-=qwertyuiop[]\\asdfghjkl;'zxcvbnm,./<",
        "~!@#$%^&*()_+QWERTYUIOP{}|ASDFGHJKL:\"ZXCVBNM<>?>",
    ),
    (
        "ru",
        "ё1234567890-=йцукенгшщзхъ\\фывапролджэячсмитьбю./",
        "Ё!\"№;%:?*()_+ЙЦУКЕНГШЩЗХЪ/ФЫВАПРОЛДЖЭЯЧСМИТЬБЮ,|",
    ),
];

#[derive(Debug)]
pub struct Layout {
    pub name: String,
    chars: HashMap<KeyCode, [Option<char>; 2]>,
    keys: HashMap<char, (KeyCode, bool)>,
    letters: HashSet<char>,
}

impl Layout {
    /// Таблица из пар «клавиша → символы без Shift и с Shift». Пробел добавляется сам.
    pub fn from_pairs(
        name: impl Into<String>,
        pairs: impl IntoIterator<Item = (KeyCode, [Option<char>; 2])>,
    ) -> Self {
        let mut chars: HashMap<KeyCode, [Option<char>; 2]> = HashMap::new();
        let mut order = Vec::new();
        for (key, pair) in pairs {
            if chars.insert(key, pair).is_none() {
                order.push(key);
            }
        }
        if let std::collections::hash_map::Entry::Vacant(slot) = chars.entry(KeyCode::KEY_SPACE) {
            slot.insert([Some(' '), Some(' ')]);
            order.push(KeyCode::KEY_SPACE);
        }
        // Первая клавиша выигрывает: KEY_102ND стоит последней и дубли не перебивает.
        let mut keys = HashMap::new();
        let mut letters = HashSet::new();
        for key in order {
            for (level, ch) in chars[&key].into_iter().enumerate() {
                let Some(ch) = ch else { continue };
                keys.entry(ch).or_insert((key, level == 1));
                if ch.is_alphabetic() {
                    letters.insert(ch);
                }
            }
        }
        Layout {
            name: name.into(),
            chars,
            keys,
            letters,
        }
    }

    fn from_rows(name: &str, lower: &str, upper: &str) -> Self {
        let lower: Vec<char> = lower.chars().collect();
        let upper: Vec<char> = upper.chars().collect();
        assert!(
            lower.len() == ROW_KEYS.len() && upper.len() == ROW_KEYS.len(),
            "таблица {name}"
        );
        let pairs = (0..ROW_KEYS.len()).map(|i| (ROW_KEYS[i], [Some(lower[i]), Some(upper[i])]));
        Layout::from_pairs(name, pairs)
    }

    pub fn char_at(&self, key: KeyCode, shift: bool) -> Option<char> {
        self.chars
            .get(&key)
            .and_then(|pair| pair[usize::from(shift)])
    }

    pub fn key_for(&self, ch: char) -> Option<(KeyCode, bool)> {
        self.keys.get(&ch).copied()
    }

    pub fn letter_count(&self) -> usize {
        self.letters.len()
    }

    fn is_letter_key(&self, key: KeyCode) -> bool {
        self.char_at(key, false).is_some_and(char::is_alphabetic)
    }
}

static BUILTIN: LazyLock<Vec<Arc<Layout>>> = LazyLock::new(|| {
    BUILTIN_ROWS
        .iter()
        .map(|&(name, lower, upper)| Arc::new(Layout::from_rows(name, lower, upper)))
        .collect()
});

/// Встроенная таблица по коду раскладки (`us`, `ru`).
pub fn builtin(code: &str) -> Option<Arc<Layout>> {
    BUILTIN.iter().find(|l| l.name == code).cloned()
}

// ─── Классы клавиш ──────────────────────────────────────────────────────────

pub fn is_char(key: KeyCode) -> bool {
    key == KeyCode::KEY_SPACE || ROW_KEYS.contains(&key)
}

pub fn is_shift(key: KeyCode) -> bool {
    matches!(key, KeyCode::KEY_LEFTSHIFT | KeyCode::KEY_RIGHTSHIFT)
}

pub fn is_ctrl(key: KeyCode) -> bool {
    matches!(key, KeyCode::KEY_LEFTCTRL | KeyCode::KEY_RIGHTCTRL)
}

pub fn is_modifier(key: KeyCode) -> bool {
    is_ctrl(key)
        || matches!(
            key,
            KeyCode::KEY_LEFTALT
                | KeyCode::KEY_RIGHTALT
                | KeyCode::KEY_LEFTMETA
                | KeyCode::KEY_RIGHTMETA
        )
}

pub fn is_nav(key: KeyCode) -> bool {
    matches!(
        key,
        KeyCode::KEY_LEFT
            | KeyCode::KEY_RIGHT
            | KeyCode::KEY_UP
            | KeyCode::KEY_DOWN
            | KeyCode::KEY_HOME
            | KeyCode::KEY_END
            | KeyCode::KEY_PAGEUP
            | KeyCode::KEY_PAGEDOWN
    )
}

/// Не двигают курсор и не меняют текст — буфер не сбрасывают.
pub fn is_neutral(key: KeyCode) -> bool {
    matches!(
        key,
        KeyCode::KEY_CAPSLOCK
            | KeyCode::KEY_NUMLOCK
            | KeyCode::KEY_SCROLLLOCK
            | KeyCode::KEY_MUTE
            | KeyCode::KEY_VOLUMEDOWN
            | KeyCode::KEY_VOLUMEUP
            | KeyCode::KEY_PLAYPAUSE
            | KeyCode::KEY_NEXTSONG
            | KeyCode::KEY_PREVIOUSSONG
            | KeyCode::KEY_STOPCD
            | KeyCode::KEY_BRIGHTNESSDOWN
            | KeyCode::KEY_BRIGHTNESSUP
            | KeyCode::KEY_MICMUTE
    )
}

pub fn is_click(key: KeyCode) -> bool {
    matches!(
        key,
        KeyCode::BTN_LEFT
            | KeyCode::BTN_RIGHT
            | KeyCode::BTN_MIDDLE
            | KeyCode::BTN_SIDE
            | KeyCode::BTN_EXTRA
    )
}

/// Коды `BTN_*`; всё остальное в `EV_KEY` — клавиши.
pub fn is_button(key: KeyCode) -> bool {
    (0x100..0x160).contains(&key.code())
}

/// `KEY_LEFTCTRL+KEY_V` или `leftctrl+v` → коды клавиш.
pub fn parse_keys(spec: &str) -> Result<Vec<KeyCode>> {
    spec.split('+')
        .map(|part| {
            let upper = part.trim().to_uppercase();
            let name = if upper.starts_with("KEY_") {
                upper
            } else {
                format!("KEY_{upper}")
            };
            name.parse::<KeyCode>()
                .map_err(|_| anyhow!("неизвестная клавиша: {}", part.trim()))
        })
        .collect()
}

// ─── Имена раскладок ────────────────────────────────────────────────────────

/// `us(intl)` → `us`, `EN` → `us`.
pub fn norm_layout(name: &str) -> String {
    let base = name.trim().to_lowercase();
    let base = base.split('(').next().unwrap_or_default();
    match base {
        "en" | "eng" => "us".into(),
        "rus" => "ru".into(),
        other => other.into(),
    }
}

/// Длинное имя xkb (`Russian`, `English (US)`) → код раскладки.
pub fn code_from_long_name(name: &str) -> Option<&'static str> {
    const HINTS: [(&str, &str); 8] = [
        ("english", "us"),
        ("russian", "ru"),
        ("ukrainian", "ua"),
        ("german", "de"),
        ("belarusian", "by"),
        ("kazakh", "kz"),
        ("french", "fr"),
        ("polish", "pl"),
    ];
    let low = name.to_lowercase();
    HINTS
        .iter()
        .find(|(hint, _)| low.starts_with(hint))
        .map(|&(_, code)| code)
}

// ─── Перекодировка выделения ────────────────────────────────────────────────

/// Таблицы, выстроенные по индексам раскладок; `None` — для раскладки таблицы нет.
pub type Tables = [Option<Arc<Layout>>];

/// Раскладка, в которой набран текст: больше всего её «собственных» букв;
/// при равенстве — текущая.
pub fn detect_source(text: &str, tables: &Tables, current: Option<usize>) -> Option<usize> {
    let mut best: Option<((usize, bool), usize)> = None;
    for (i, table) in tables.iter().enumerate() {
        let Some(layout) = table else { continue };
        let score = (
            text.chars().filter(|c| layout.letters.contains(c)).count(),
            Some(i) == current,
        );
        if best.is_none_or(|(top, _)| score > top) {
            best = Some((score, i));
        }
    }
    best.map(|(_, i)| i)
}

/// Следующая после `src` раскладка с таблицей, которая отличается от исходной.
pub fn next_with_table(src: usize, tables: &Tables) -> Option<usize> {
    let source = tables.get(src)?.as_ref()?;
    (1..tables.len())
        .map(|step| (src + step) % tables.len())
        .find(|&j| {
            tables[j]
                .as_ref()
                .is_some_and(|t| t.letters != source.letters || t.chars != source.chars)
        })
}

/// Тот же текст, как если бы те же клавиши нажали в раскладке `tgt`.
pub fn convert_text(text: &str, src: &Layout, tgt: &Layout) -> String {
    text.chars()
        .map(|ch| {
            src.key_for(ch)
                .and_then(|(key, shift)| tgt.char_at(key, shift))
                .unwrap_or(ch)
        })
        .collect()
}

/// Нажатия, которые в раскладке `tgt` наберут `convert_text()`; `None`, если так не выйдет
/// (перевод строки, табуляция, «», эмодзи…).
pub fn plan_strokes(
    text: &str,
    src: &Layout,
    tgt: &Layout,
    caps: bool,
    layout: usize,
) -> Option<Vec<Stroke>> {
    text.chars()
        .map(|ch| {
            // Если клавиша в целевой раскладке ничего не даёт, символ оставляем как есть.
            let (key, shift) = src
                .key_for(ch)
                .filter(|&(key, shift)| tgt.char_at(key, shift).is_some())
                .or_else(|| tgt.key_for(ch))?;
            // CapsLock инвертирует регистр букв.
            let shift = shift ^ (caps && tgt.is_letter_key(key));
            Some(Stroke {
                key,
                shift,
                layout: Some(layout),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables(list: &[&str]) -> Vec<Option<Arc<Layout>>> {
        list.iter().map(|s| builtin(s)).collect()
    }

    #[test]
    fn builtin_tables_are_complete() {
        for name in ["us", "ru"] {
            let layout = builtin(name).unwrap();
            assert_eq!(layout.chars.len(), ROW_KEYS.len() + 1);
            assert_eq!(layout.key_for(' '), Some((KeyCode::KEY_SPACE, false)));
        }
        assert_eq!(builtin("ru").unwrap().letter_count(), 66); // 33 буквы × 2 регистра
        assert_eq!(builtin("us").unwrap().letter_count(), 52);
        assert!(builtin("de").is_none());
    }

    #[test]
    fn converts_both_ways() {
        let (us, ru) = (builtin("us").unwrap(), builtin("ru").unwrap());
        assert_eq!(
            convert_text("Ghbdtn? rfr ltkf&", &us, &ru),
            "Привет, как дела?"
        );
        assert_eq!(
            convert_text("Привет, как дела?", &ru, &us),
            "Ghbdtn? rfr ltkf&"
        );
        assert_eq!(convert_text("ghbdtn мир", &us, &ru), "привет мир");
        assert_eq!(convert_text("Ёжик № 5", &ru, &us), "~;br # 5");
    }

    #[test]
    fn round_trip_of_main_block() {
        let (us, ru) = (builtin("us").unwrap(), builtin("ru").unwrap());
        let all: String = ROW_KEYS[..47]
            .iter()
            .flat_map(|&k| [us.char_at(k, false), us.char_at(k, true)])
            .flatten()
            .collect();
        assert_eq!(convert_text(&convert_text(&all, &us, &ru), &ru, &us), all);
    }

    #[test]
    fn plans_keys_with_capslock() {
        let (us, ru) = (builtin("us").unwrap(), builtin("ru").unwrap());
        let plan = plan_strokes("Gh;", &us, &ru, false, 1).unwrap();
        let keys: Vec<_> = plan.iter().map(|s| (s.key, s.shift)).collect();
        assert_eq!(
            keys,
            [
                (KeyCode::KEY_G, true),
                (KeyCode::KEY_H, false),
                (KeyCode::KEY_SEMICOLON, false)
            ]
        );
        // С CapsLock буквы целевой раскладки (а «;» → «ж» — буква) набираются с инверсией Shift.
        let caps = plan_strokes("Gh;1", &us, &ru, true, 1).unwrap();
        assert_eq!(
            caps.iter().map(|s| s.shift).collect::<Vec<_>>(),
            [false, true, true, false]
        );
        assert!(plan_strokes("a\nb", &us, &ru, false, 1).is_none());
        assert!(plan_strokes("«a»", &us, &ru, false, 1).is_none());
    }

    #[test]
    fn missing_target_char_is_kept() {
        let us = builtin("us").unwrap();
        // В «урезанной» раскладке у клавиши G нет символа — «g» остаётся «g».
        let partial = Layout::from_pairs("x", [(KeyCode::KEY_H, [Some('р'), Some('Р')])]);
        assert_eq!(convert_text("gh", &us, &partial), "gр");
        assert!(plan_strokes("gh", &us, &partial, false, 1).is_none());
    }

    #[test]
    fn detects_source_layout() {
        let list = tables(&["us", "ru"]);
        assert_eq!(detect_source("ghbdtn мир", &list, Some(1)), Some(0));
        assert_eq!(detect_source("руддщ", &list, Some(0)), Some(1));
        assert_eq!(detect_source("123", &list, Some(1)), Some(1));
        assert_eq!(detect_source("123", &list, None), Some(0));
        assert_eq!(detect_source("abc", &tables(&["de", "fr"]), Some(0)), None);
        assert_eq!(next_with_table(0, &list), Some(1));
        assert_eq!(next_with_table(1, &list), Some(0));
        assert_eq!(next_with_table(0, &tables(&["us", "de", "ru"])), Some(2));
        assert_eq!(next_with_table(0, &tables(&["us"])), None);
        assert_eq!(next_with_table(0, &tables(&["us", "us"])), None);
    }

    #[test]
    fn parses_names() {
        assert_eq!(
            parse_keys("leftctrl+v").unwrap(),
            [KeyCode::KEY_LEFTCTRL, KeyCode::KEY_V]
        );
        assert_eq!(
            parse_keys("KEY_LEFTMETA + KEY_SPACE").unwrap(),
            [KeyCode::KEY_LEFTMETA, KeyCode::KEY_SPACE]
        );
        assert!(parse_keys("ctrl+v").is_err());
        assert_eq!(norm_layout("us(intl)"), "us");
        assert_eq!(norm_layout(" EN "), "us");
        assert_eq!(code_from_long_name("Russian"), Some("ru"));
        assert_eq!(code_from_long_name("English (US)"), Some("us"));
        assert_eq!(code_from_long_name("Klingon"), None);
    }

    #[test]
    fn key_classes() {
        assert!(
            is_char(KeyCode::KEY_SPACE)
                && is_char(KeyCode::KEY_102ND)
                && !is_char(KeyCode::KEY_ENTER)
        );
        assert!(is_modifier(KeyCode::KEY_RIGHTMETA) && !is_modifier(KeyCode::KEY_LEFTSHIFT));
        assert!(is_button(KeyCode::BTN_TOUCH) && !is_button(KeyCode::KEY_MICMUTE));
        assert!(is_click(KeyCode::BTN_LEFT) && !is_click(KeyCode::BTN_TOUCH));
    }
}

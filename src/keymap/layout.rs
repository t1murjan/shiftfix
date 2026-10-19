//! Таблица раскладки: какой символ даёт каждая клавиша без Shift и с Shift.
//! Встроенные таблицы us и ru — запасной вариант, когда keymap сеанса нет.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

use evdev::KeyCode;

use crate::keys::ROW_KEYS;

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
    /// Мёртвые клавиши (клавиша, Shift): сами символ не печатают.
    dead: HashSet<(KeyCode, bool)>,
}

impl Layout {
    /// Таблица из пар «клавиша → символы без Shift и с Shift». Пробел добавляется сам.
    pub(crate) fn from_pairs(
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
            dead: HashSet::new(),
        }
    }

    pub(crate) fn with_dead(mut self, dead: impl IntoIterator<Item = (KeyCode, bool)>) -> Self {
        self.dead.extend(dead);
        self
    }

    pub(crate) fn is_dead(&self, key: KeyCode, shift: bool) -> bool {
        self.dead.contains(&(key, shift))
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

    pub(crate) fn char_at(&self, key: KeyCode, shift: bool) -> Option<char> {
        self.chars
            .get(&key)
            .and_then(|pair| pair[usize::from(shift)])
    }

    pub(crate) fn key_for(&self, ch: char) -> Option<(KeyCode, bool)> {
        self.keys.get(&ch).copied()
    }

    #[cfg(test)]
    pub(crate) fn letter_count(&self) -> usize {
        self.letters.len()
    }

    pub(super) fn is_letter(&self, ch: char) -> bool {
        self.letters.contains(&ch)
    }

    /// Клавиши печатают одно и то же (например, us и us(intl) без мёртвых клавиш).
    pub(super) fn same_chars(&self, other: &Layout) -> bool {
        self.letters == other.letters && self.chars == other.chars
    }

    /// Клавиша печатает букву (без Shift).
    pub(super) fn is_letter_key(&self, key: KeyCode) -> bool {
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
pub(crate) fn builtin(code: &str) -> Option<Arc<Layout>> {
    BUILTIN.iter().find(|l| l.name == code).cloned()
}

/// `us(intl)` → `us`, `EN` → `us`.
pub(crate) fn norm_layout(name: &str) -> String {
    let base = name.trim().to_lowercase();
    let base = base.split('(').next().unwrap_or_default();
    match base {
        "en" | "eng" => "us".into(),
        "rus" => "ru".into(),
        other => other.into(),
    }
}

/// Длинное имя xkb (`Russian`, `English (US)`) → код раскладки.
pub(crate) fn code_from_long_name(name: &str) -> Option<&'static str> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn normalizes_layout_names() {
        assert_eq!(norm_layout("us(intl)"), "us");
        assert_eq!(norm_layout(" EN "), "us");
        assert_eq!(code_from_long_name("Russian"), Some("ru"));
        assert_eq!(code_from_long_name("English (US)"), Some("us"));
        assert_eq!(code_from_long_name("Klingon"), None);
    }
}

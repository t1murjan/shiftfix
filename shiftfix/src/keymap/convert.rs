//! Перекодировка выделенного текста: в какой раскладке он набран и что
//! получится, если нажать те же клавиши в другой.

use std::sync::Arc;

use super::Layout;
use crate::engine::Stroke;

/// Таблицы, выстроенные по индексам раскладок; `None` — для раскладки таблицы нет.
pub type Tables = [Option<Arc<Layout>>];

/// Раскладка, в которой набран текст: больше всего её «собственных» букв;
/// при равенстве — текущая.
pub fn detect_source(text: &str, tables: &Tables, current: Option<usize>) -> Option<usize> {
    let mut best: Option<((usize, bool), usize)> = None;
    for (i, table) in tables.iter().enumerate() {
        let Some(layout) = table else { continue };
        let score = (
            text.chars().filter(|&c| layout.is_letter(c)).count(),
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
        .find(|&j| tables[j].as_ref().is_some_and(|t| !t.same_chars(source)))
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
    use evdev::KeyCode;

    use super::*;
    use crate::keymap::{ROW_KEYS, builtin};

    fn tables(list: &[&str]) -> Vec<Option<Arc<Layout>>> {
        list.iter().map(|s| builtin(s)).collect()
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
}

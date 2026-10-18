//! Буфер нажатий: что ⇧⇧ сотрёт и наберёт заново.

use evdev::KeyCode;

use crate::keys::Stroke;

/// С какого нажатия ⇧⇧ стирает и набирает заново: с начала последнего слова
/// до конца `strokes`, вместе с пробелами после слова. `cut` — начало
/// `strokes` отрезано, и слово с самого начала может быть неполным;
/// `max_len` — самое длинное слово без пробелов после него. `Err` — почему
/// исправлять нечего.
pub(super) fn last_word_start(
    strokes: &[Stroke],
    cut: bool,
    max_len: usize,
) -> Result<usize, &'static str> {
    let is_space = |s: &Stroke| s.key == KeyCode::KEY_SPACE;
    let end = strokes
        .iter()
        .rposition(|s| !is_space(s))
        .map_or(0, |i| i + 1);
    let start = strokes[..end]
        .iter()
        .rposition(is_space)
        .map_or(0, |i| i + 1);
    if start == end {
        return Err("после пробелов нет слова");
    }
    if start == 0 && cut {
        // Начало слова вытеснено из буфера, а исправить только хвост —
        // исправить полслова.
        return Err("слово не поместилось в буфер, пропускаю");
    }
    if end - start > max_len {
        return Err("слово длиннее max_word_len, пропускаю");
    }
    Ok(start)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Пробел — пробел, любой другой символ — клавиша A.
    fn strokes(text: &str) -> Vec<Stroke> {
        text.chars()
            .map(|c| Stroke {
                key: if c == ' ' {
                    KeyCode::KEY_SPACE
                } else {
                    KeyCode::KEY_A
                },
                shift: false,
                layout: None,
            })
            .collect()
    }

    #[test]
    fn last_word_with_spaces_after_it() {
        assert_eq!(last_word_start(&strokes("ab cd"), false, 10), Ok(3));
        assert_eq!(last_word_start(&strokes("ab cd  "), false, 10), Ok(3));
        assert_eq!(last_word_start(&strokes("abcd "), false, 10), Ok(0));
        // Одни пробелы — исправлять нечего.
        assert!(last_word_start(&strokes("   "), false, 10).is_err());
        assert!(last_word_start(&strokes(""), false, 10).is_err());
    }

    #[test]
    fn long_or_cut_word_is_skipped() {
        // Длина — без пробелов после слова.
        assert_eq!(last_word_start(&strokes("ab abcd   "), false, 4), Ok(3));
        assert!(last_word_start(&strokes("ab abcde"), false, 4).is_err());
        // Начало отрезано: первое слово может быть неполным, следующие — целые.
        assert!(last_word_start(&strokes("cd  "), true, 10).is_err());
        assert_eq!(last_word_start(&strokes("cd ef"), true, 10), Ok(3));
    }
}

//! Буфер нажатий: что ⇧⇧ сотрёт и наберёт заново.

use evdev::KeyCode;

use crate::keys::Stroke;

/// Сколько последних нажатий помнить.
pub(super) const BUF_MAX: usize = 256;

/// Последние нажатия с символами.
#[derive(Default)]
pub(super) struct Buffer {
    strokes: Vec<Stroke>,
    /// Начало буфера отрезано (BUF_MAX): первое слово в нём может быть
    /// неполным, и Backspace этого не меняет.
    cut: bool,
    /// Буфер — только что вставленный фрагмент целиком.
    segment: bool,
}

impl Buffer {
    pub(super) fn strokes(&self) -> &[Stroke] {
        &self.strokes
    }

    pub(super) fn is_empty(&self) -> bool {
        self.strokes.is_empty()
    }

    pub(super) fn is_segment(&self) -> bool {
        self.segment
    }

    /// Следующая буква начнёт слово: буфер пуст или кончается пробелом.
    pub(super) fn at_word_start(&self) -> bool {
        self.strokes
            .last()
            .is_none_or(|s| s.key == KeyCode::KEY_SPACE)
    }

    /// Нажатие с символом; в полном буфере самое старое вытесняется.
    pub(super) fn push(&mut self, stroke: Stroke) {
        self.segment = false;
        self.strokes.push(stroke);
        if self.strokes.len() > BUF_MAX {
            let extra = self.strokes.len() - BUF_MAX;
            self.strokes.drain(..extra);
            self.cut = true;
        }
    }

    /// Backspace.
    pub(super) fn pop(&mut self) {
        self.segment = false;
        self.strokes.pop();
    }

    pub(super) fn clear(&mut self) {
        self.strokes.clear();
        self.cut = false;
        self.segment = false;
    }

    /// Заменить только что набранным или вставленным фрагментом.
    pub(super) fn set_segment(&mut self, strokes: Vec<Stroke>) {
        self.strokes = strokes;
        self.cut = false;
        self.segment = true;
    }

    /// С какого нажатия ⇧⇧ стирает и набирает заново: фрагмент — целиком,
    /// иначе с начала последнего слова. `Err` — почему исправлять нечего.
    pub(super) fn fix_start(&self, max_word_len: usize) -> Result<usize, &'static str> {
        if self.segment {
            Ok(0)
        } else {
            last_word_start(&self.strokes, self.cut, max_word_len)
        }
    }

    /// Нажатия с `start` набраны заново в раскладке `layout`.
    pub(super) fn retyped(&mut self, start: usize, layout: Option<usize>) {
        for stroke in &mut self.strokes[start..] {
            stroke.layout = layout;
        }
    }
}

/// С какого нажатия ⇧⇧ стирает и набирает заново: с начала последнего слова
/// до конца `strokes`, вместе с пробелами после слова. `cut` — начало
/// `strokes` отрезано, и слово с самого начала может быть неполным;
/// `max_len` — самое длинное слово без пробелов после него. `Err` — почему
/// исправлять нечего.
fn last_word_start(strokes: &[Stroke], cut: bool, max_len: usize) -> Result<usize, &'static str> {
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

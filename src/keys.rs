//! Классы клавиш: что меняет текст, что двигает курсор, что модификатор;
//! нажатие для набора (`Stroke`); разбор сочетаний клавиш из конфига.

use anyhow::{Result, anyhow};
use evdev::KeyCode;

/// Клавиши основного блока: цифровой ряд, три буквенных ряда и ISO-клавиша
/// между левым Shift и Z.
#[rustfmt::skip]
pub(crate) const ROW_KEYS: [KeyCode; 48] = [
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

/// Нажатие для набора: клавиша, Shift и раскладка, в которой её нажали.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stroke {
    pub key: KeyCode,
    pub shift: bool,
    /// Индекс раскладки, в которой клавиша нажата (если известен).
    pub layout: Option<usize>,
}

/// Печатает символ: пробел или клавиша основного блока (`ROW_KEYS`).
pub(crate) fn is_char(key: KeyCode) -> bool {
    key == KeyCode::KEY_SPACE || ROW_KEYS.contains(&key)
}

pub(crate) fn is_shift(key: KeyCode) -> bool {
    matches!(key, KeyCode::KEY_LEFTSHIFT | KeyCode::KEY_RIGHTSHIFT)
}

pub(crate) fn is_ctrl(key: KeyCode) -> bool {
    matches!(key, KeyCode::KEY_LEFTCTRL | KeyCode::KEY_RIGHTCTRL)
}

/// Ctrl/Alt/Meta, без Shift: с ними нажатие — сочетание, а не символ.
pub(crate) fn is_modifier(key: KeyCode) -> bool {
    is_ctrl(key)
        || matches!(
            key,
            KeyCode::KEY_LEFTALT
                | KeyCode::KEY_RIGHTALT
                | KeyCode::KEY_LEFTMETA
                | KeyCode::KEY_RIGHTMETA
        )
}

pub(crate) fn is_nav(key: KeyCode) -> bool {
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
pub(crate) fn is_neutral(key: KeyCode) -> bool {
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

pub(crate) fn is_click(key: KeyCode) -> bool {
    matches!(
        key,
        KeyCode::BTN_LEFT
            | KeyCode::BTN_RIGHT
            | KeyCode::BTN_MIDDLE
            | KeyCode::BTN_SIDE
            | KeyCode::BTN_EXTRA
    )
}

/// Коды `BTN_*` (мышь, тачпад, перо, джойстик); всё остальное в `EV_KEY` — клавиши.
pub(crate) fn is_button(key: KeyCode) -> bool {
    let code = key.code();
    // BTN_MISC (он же BTN_0) … BTN_GEAR_UP; с KEY_OK снова идут клавиши.
    (KeyCode::BTN_0.code()..KeyCode::KEY_OK.code()).contains(&code)
        || (KeyCode::BTN_DPAD_UP.code()..=KeyCode::BTN_DPAD_RIGHT.code()).contains(&code)
        || (KeyCode::BTN_TRIGGER_HAPPY1.code()..=KeyCode::BTN_TRIGGER_HAPPY40.code())
            .contains(&code)
}

/// `KEY_LEFTCTRL+KEY_V` или `leftctrl+v` → коды клавиш.
pub(crate) fn parse_keys(spec: &str) -> Result<Vec<KeyCode>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_classes() {
        assert!(
            is_char(KeyCode::KEY_SPACE)
                && is_char(KeyCode::KEY_102ND)
                && !is_char(KeyCode::KEY_ENTER)
        );
        assert!(is_modifier(KeyCode::KEY_RIGHTMETA) && !is_modifier(KeyCode::KEY_LEFTSHIFT));
        assert!(is_button(KeyCode::BTN_TOUCH) && !is_button(KeyCode::KEY_MICMUTE));
        // Кнопки геймпада — не только в 0x100…0x15f.
        assert!(
            is_button(KeyCode::BTN_DPAD_LEFT)
                && is_button(KeyCode::BTN_TRIGGER_HAPPY1)
                && is_button(KeyCode::BTN_TRIGGER_HAPPY40)
                && !is_button(KeyCode::KEY_OK)
                && !is_button(KeyCode::KEY_LIGHTS_TOGGLE)
                && !is_button(KeyCode::KEY_SELECTIVE_SCREENSHOT)
        );
        assert!(is_click(KeyCode::BTN_LEFT) && !is_click(KeyCode::BTN_TOUCH));
    }

    #[test]
    fn parses_key_names() {
        assert_eq!(
            parse_keys("leftctrl+v").unwrap(),
            [KeyCode::KEY_LEFTCTRL, KeyCode::KEY_V]
        );
        assert_eq!(
            parse_keys("KEY_LEFTMETA + KEY_SPACE").unwrap(),
            [KeyCode::KEY_LEFTMETA, KeyCode::KEY_SPACE]
        );
        assert!(parse_keys("ctrl+v").is_err());
    }
}

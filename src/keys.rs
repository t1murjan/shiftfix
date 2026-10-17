//! Классы клавиш: что меняет текст, что двигает курсор, что модификатор;
//! разбор сочетаний клавиш из конфига.

use anyhow::{Result, anyhow};
use evdev::KeyCode;

use crate::keymap::ROW_KEYS;

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

//! Двойное нажатие Shift и одиночные Ctrl/Alt/Meta.

use evdev::KeyCode;
use log::debug;

use crate::config::Config;
use crate::keys::{is_modifier, is_shift};

#[derive(Default)]
pub(super) struct Taps {
    /// «Чистое» нажатие Shift: в этот момент больше ничего не зажато.
    shift_press: Option<f64>,
    /// Отпускание первого Shift двойного нажатия.
    first_tap: Option<f64>,
    /// Одиночный Ctrl/Alt/Meta без других клавиш.
    lone_mod: Option<KeyCode>,
}

impl Taps {
    pub(super) fn clear(&mut self) {
        *self = Taps::default();
    }

    /// Нажата клавиша; `alone` — больше ничего не зажато.
    pub(super) fn pressed(&mut self, key: KeyCode, alone: bool, ts: f64) {
        if is_shift(key) {
            self.lone_mod = None;
            if alone {
                self.shift_press = Some(ts);
            } else {
                self.shift_press = None;
                self.first_tap = None;
            }
        } else {
            self.shift_press = None;
            self.first_tap = None;
            self.lone_mod = (alone && is_modifier(key)).then_some(key);
        }
    }

    /// Отпущен Shift: `true`, если это второе касание ⇧⇧.
    ///
    /// Почему касание не засчитано, видно в журнале с `--debug`.
    pub(super) fn shift_released(&mut self, ts: f64, cfg: &Config) -> bool {
        match self.shift_press.take() {
            Some(pressed) if ts - pressed <= cfg.tap_max_hold => match self.first_tap {
                Some(first) if pressed - first <= cfg.double_tap => {
                    self.first_tap = None;
                    true
                }
                Some(first) => {
                    debug!(
                        "⇧⇧ не засчитан: пауза между касаниями {} мс, а double_tap_ms = {}",
                        ms(pressed - first),
                        ms(cfg.double_tap)
                    );
                    self.first_tap = Some(ts);
                    false
                }
                None => {
                    self.first_tap = Some(ts);
                    false
                }
            },
            Some(pressed) => {
                debug!(
                    "⇧⇧ не засчитан: Shift держали {} мс, а tap_max_hold_ms = {}",
                    ms(ts - pressed),
                    ms(cfg.tap_max_hold)
                );
                self.first_tap = None;
                false
            }
            None => {
                self.first_tap = None;
                false
            }
        }
    }

    /// Отпущен Ctrl/Alt/Meta: `true`, если его нажали одним и с ним ничего
    /// не нажимали.
    pub(super) fn lone_mod_released(&mut self, key: KeyCode) -> bool {
        self.lone_mod.take_if(|k| *k == key).is_some()
    }
}

/// Секунды — в целые миллисекунды для журнала.
fn ms(seconds: f64) -> u64 {
    (seconds * 1000.0).round() as u64
}

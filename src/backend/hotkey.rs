//! Запасной переключатель: нажать сочетание клавиш из конфига.

use anyhow::Result;
use evdev::KeyCode;

use crate::engine::{Backend, Keys};

/// Запасной вариант: нажимает хоткей переключения, текущая раскладка неизвестна.
pub struct Hotkey {
    keys: Vec<KeyCode>,
    layouts: Vec<String>,
}

impl Hotkey {
    /// `keys` — сочетание переключения, `layouts` — раскладки по порядку.
    pub fn new(keys: Vec<KeyCode>, layouts: Vec<String>) -> Self {
        Hotkey { keys, layouts }
    }

    pub fn keys(&self) -> &[KeyCode] {
        &self.keys
    }
}

impl Backend for Hotkey {
    fn name(&self) -> &'static str {
        "хоткей"
    }

    fn layouts(&mut self) -> Result<Vec<String>> {
        Ok(self.layouts.clone())
    }

    fn current(&mut self) -> Result<Option<usize>> {
        Ok(None)
    }

    fn set(&mut self, _index: usize, keys: &mut dyn Keys) -> Result<()> {
        self.toggle(keys)
    }

    fn toggle(&mut self, keys: &mut dyn Keys) -> Result<()> {
        keys.combo(&self.keys)
    }
}

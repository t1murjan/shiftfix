//! Что движок получает от службы: эмуляцию нажатий, переключение раскладки,
//! выделение. Здесь только интерфейсы; реализации — `daemon/uinput.rs`,
//! `backend/` и `selection/`.

use anyhow::Result;
use evdev::KeyCode;

use crate::config::Config;
use crate::keys::Stroke;

/// Эмуляция нажатий.
pub trait Keys {
    /// Новые настройки (задержка эмуляции и т.п.).
    fn configure(&mut self, _cfg: &Config) {}
    fn backspace(&mut self, count: usize) -> Result<()>;
    fn type_strokes(&mut self, strokes: &[Stroke]) -> Result<()>;
    fn combo(&mut self, keys: &[KeyCode]) -> Result<()>;
    /// Зажать `hold`, нажать `tap` `times` раз, отпустить (Super+Space+Space в GNOME).
    fn chord(&mut self, hold: &[KeyCode], tap: KeyCode, times: usize) -> Result<()>;
}

/// Переключение раскладки.
pub trait Backend {
    fn name(&self) -> &'static str;
    fn layouts(&mut self) -> Result<Vec<String>>;
    /// `None` — текущая раскладка неизвестна (переключение хоткеем).
    fn current(&mut self) -> Result<Option<usize>>;
    fn set(&mut self, index: usize, keys: &mut dyn Keys) -> Result<()>;
    fn toggle(&mut self, keys: &mut dyn Keys) -> Result<()>;
}

/// Выделенный текст и вставка.
pub trait Selection {
    fn read_primary(&mut self) -> Option<String>;
    fn paste(&mut self, text: &str, keys: &mut dyn Keys) -> Result<()>;
}

/// Почему исправлять сейчас нельзя (`None` — можно): неактивный сеанс и т.п.
pub type Guard = Box<dyn FnMut() -> Option<String>>;

//! Что движок получает от службы: эмуляцию нажатий, переключение раскладки,
//! выделение и хуки (CapsLock, часы, можно ли исправлять). Здесь только
//! интерфейсы; реализации — `daemon/uinput.rs`, `backend/`, `selection/` и
//! `daemon/mod.rs`.

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
    /// Коды раскладок xkb, приведённые `keymap::norm_layout` («us», «ru»).
    /// Порядок совпадает с номерами в `current` и `set`. Движок сверяет список
    /// с keymap: если раскладок столько же, таблицы берутся из неё по номерам,
    /// иначе — встроенные по кодам.
    fn layouts(&mut self) -> Result<Vec<String>>;
    /// `None` — текущая раскладка неизвестна (переключение хоткеем).
    fn current(&mut self) -> Result<Option<usize>>;
    /// Включить раскладку `index`.
    fn set(&mut self, index: usize, keys: &mut dyn Keys) -> Result<()>;
    /// Следующая раскладка по кругу (когда текущая неизвестна).
    fn toggle(&mut self, keys: &mut dyn Keys) -> Result<()>;
}

/// Выделенный текст и вставка.
pub trait Selection {
    fn read_primary(&mut self) -> Option<String>;
    fn paste(&mut self, text: &str, keys: &mut dyn Keys) -> Result<()>;
}

/// О чём ещё движок спрашивает службу.
pub(crate) struct Hooks {
    /// Горит ли CapsLock.
    pub caps_on: Box<dyn FnMut() -> bool>,
    /// Монотонное время, секунды.
    pub clock: Box<dyn Fn() -> f64>,
    /// Почему исправлять сейчас нельзя (`None` — можно): неактивный сеанс и т.п.
    pub guard: Box<dyn FnMut() -> Option<String>>,
}

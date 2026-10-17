//! Переключение раскладки: KWin по DBus, GNOME, Hyprland, Sway, XKB в X11
//! или эмуляция хоткея.
//! В режиме «авто» окружение определяется при первом обращении и заново —
//! если оно сменилось или служба стартовала раньше сессии.

mod auto;
mod gnome;
mod hotkey;
mod hyprland;
mod kde;
mod sway;
mod x11;

pub use auto::Auto;
pub use gnome::Gnome;
pub use hotkey::Hotkey;
pub use hyprland::Hyprland;
pub use kde::Kde;
pub use sway::Sway;
pub use x11::XkbLayout;

use crate::config::{BackendKind, Config};
use crate::engine::Backend;

/// Переключатель, выбранный в конфиге (`backend = …`).
pub fn make(cfg: &Config) -> Box<dyn Backend> {
    let hotkey = || Hotkey::new(cfg.switch_hotkey.clone(), cfg.hotkey_layouts.clone());
    match cfg.backend {
        BackendKind::Auto => Box::new(Auto::new(hotkey())),
        BackendKind::Kde => Box::new(Kde::default()),
        BackendKind::Gnome => Box::new(Gnome::new(cfg.switch_hotkey.clone())),
        BackendKind::Hyprland => Box::new(Hyprland::default()),
        BackendKind::Sway => Box::new(Sway::default()),
        BackendKind::X11 => Box::new(XkbLayout::default()),
        BackendKind::Hotkey => Box::new(hotkey()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_falls_back_to_hotkey() {
        // В песочнице тестов нет ни KDE, ни Hyprland, ни Sway.
        let cfg = Config::default();
        let mut auto = make(&cfg);
        if auto.layouts().is_ok_and(|l| l == ["us", "ru"]) {
            assert_eq!(auto.current().unwrap(), None);
            assert_eq!(auto.name(), "хоткей");
        }
    }
}

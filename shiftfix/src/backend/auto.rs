//! Выбор переключателя по окружению: при первом обращении и заново,
//! если прежний перестал отвечать.

use std::time::{Duration, Instant};

use anyhow::Result;
use evdev::KeyCode;
use log::info;

use super::{Gnome, Hotkey, Hyprland, Kde, Sway, XkbLayout};
use crate::engine::{Backend, Keys};
use crate::sys::dbus::{Bus, Client};
use crate::sys::env;

/// Определяет окружение лениво и переопределяет, если оно перестало отвечать.
pub struct Auto {
    found: Option<Box<dyn Backend>>,
    fallback: Hotkey,
    last_probe: Option<Instant>,
}

impl Auto {
    /// `fallback` работает, пока окружение не определено.
    pub fn new(fallback: Hotkey) -> Self {
        Auto {
            found: None,
            fallback,
            last_probe: None,
        }
    }

    fn probe(hotkey: &[KeyCode]) -> Option<Box<dyn Backend>> {
        let mut candidates: Vec<Box<dyn Backend>> = Vec::new();
        if env::hyprland_signature().is_some() {
            candidates.push(Box::new(Hyprland::default()));
        }
        if env::sway_socket().is_some() {
            candidates.push(Box::new(Sway::default()));
        }
        candidates.push(Box::new(Kde::default()));
        if env::in_path("gsettings") && Client::new(Bus::Session).has_owner("org.gnome.Shell") {
            candidates.push(Box::new(Gnome::new(hotkey.to_vec())));
        }
        // Группа XKB у XWayland — не раскладка сеанса Wayland: только чистый X11.
        if env::wayland_display().is_none() && env::x11_display().is_some() {
            candidates.push(Box::new(XkbLayout::default()));
        }
        candidates
            .into_iter()
            .find_map(|mut backend| match backend.layouts() {
                Ok(list) if !list.is_empty() => {
                    info!(
                        "раскладку переключает {}: {}",
                        backend.name(),
                        list.join(", ")
                    );
                    Some(backend)
                }
                _ => None,
            })
    }

    fn backend(&mut self) -> &mut dyn Backend {
        let stale = self
            .last_probe
            .is_none_or(|t| t.elapsed() > Duration::from_secs(5));
        if self.found.is_none() && stale {
            self.last_probe = Some(Instant::now());
            self.found = Auto::probe(self.fallback.keys());
        }
        match self.found.as_deref_mut() {
            Some(backend) => backend,
            None => &mut self.fallback,
        }
    }

    fn with<R>(&mut self, f: impl FnOnce(&mut dyn Backend) -> Result<R>) -> Result<R> {
        let result = f(self.backend());
        if result.is_err() && self.found.is_some() {
            self.found = None; // окружение сменилось — в следующий раз определим заново
            self.last_probe = None;
        }
        result
    }
}

impl Backend for Auto {
    fn name(&self) -> &'static str {
        self.found.as_ref().map_or("хоткей", |b| b.name())
    }

    fn layouts(&mut self) -> Result<Vec<String>> {
        self.with(|b| b.layouts())
    }

    fn current(&mut self) -> Result<Option<usize>> {
        self.with(|b| b.current())
    }

    fn set(&mut self, index: usize, keys: &mut dyn Keys) -> Result<()> {
        self.with(|b| b.set(index, keys))
    }

    fn toggle(&mut self, keys: &mut dyn Keys) -> Result<()> {
        self.with(|b| b.toggle(keys))
    }
}

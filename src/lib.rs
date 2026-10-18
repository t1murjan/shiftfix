//! shiftfix: двойной Shift исправляет раскладку последнего слова или выделения.
//!
//! Общая часть службы (`shiftfix`) и окна (`shiftfix-gui`):
//!
//! - [`engine`] — что делать с нажатиями: поймать ⇧⇧, стереть и перенабрать слово;
//! - [`daemon`] — служба: главный цикл, сокет IPC, виртуальная клавиатура,
//!   сигналы, трей;
//! - [`devices`] — устройства ввода, их роли и чтение событий (`Event`, о
//!   службе не знает);
//! - [`backend`] — переключение раскладки в KDE, GNOME, Hyprland, Sway и X11;
//! - [`keymap`] — таблицы раскладок и перекодировка текста, [`keys`] — классы
//!   клавиш и нажатие для набора (`Stroke`);
//! - [`selection`] — выделенный текст и вставка через буфер обмена;
//! - [`config`] — `~/.config/shiftfix/config.toml`;
//! - [`ipc`] — связь окна и команд `shiftfix --status` и т.п. со службой:
//!   запросы, ответы и клиент (сервер — в `daemon`);
//! - [`service`] — юнит systemd и файл автозапуска;
//! - [`logbuf`] — журнал, [`icon`] — значок, нарисованный кодом;
//! - [`sys`] — окружение сеанса, logind, DBus, Wayland, X11, внешние команды.

pub mod backend;
pub mod config;
pub mod daemon;
pub mod devices;
pub mod engine;
pub mod icon;
pub mod ipc;
pub mod keymap;
pub mod keys;
pub mod logbuf;
pub mod selection;
pub mod service;
pub mod sys;
#[cfg(test)]
mod testutil;

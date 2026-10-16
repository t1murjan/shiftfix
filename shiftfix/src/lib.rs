//! shiftfix: двойной Shift исправляет раскладку последнего слова или выделения.
//!
//! Общая часть службы (`shiftfix`) и графической оболочки (`shiftfix-gui`).

pub mod backend;
pub mod clipboard;
pub mod config;
pub mod daemon;
pub mod dbus;
pub mod devices;
pub mod engine;
pub mod env;
pub mod icon;
pub mod input;
pub mod ipc;
pub mod keys;
pub mod logbuf;
pub mod portal;
pub mod proc;
pub mod selection;
pub mod service;
pub mod session;
pub mod tray;
pub mod x11;
pub mod xkb;

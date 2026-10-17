//! shiftfix: двойной Shift исправляет раскладку последнего слова или выделения.
//!
//! Общая часть службы (`shiftfix`) и графической оболочки (`shiftfix-gui`).

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

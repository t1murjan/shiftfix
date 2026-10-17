//! Сигналы службе: SIGINT/SIGTERM — выход, SIGUSR1 — пауза, SIGHUP — перечитать конфиг.

use std::sync::mpsc::Sender;
use std::thread;

use anyhow::{Context, Result};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM, SIGUSR1};
use signal_hook::iterator::Signals;

use super::Msg;

/// SIGINT/SIGTERM — выход, SIGUSR1 — пауза, SIGHUP — перечитать конфиг.
pub fn spawn(tx: Sender<Msg>) -> Result<()> {
    let mut signals = Signals::new([SIGINT, SIGTERM, SIGUSR1, SIGHUP]).context("сигналы")?;
    thread::Builder::new()
        .name("signals".into())
        .spawn(move || {
            for signal in signals.forever() {
                let msg = match signal {
                    SIGUSR1 => Msg::TogglePause,
                    SIGHUP => Msg::Reload,
                    _ => Msg::Quit,
                };
                if tx.send(msg).is_err() {
                    break;
                }
            }
        })?;
    Ok(())
}

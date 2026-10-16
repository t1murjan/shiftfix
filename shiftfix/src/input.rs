//! Виртуальная клавиатура uinput и сигналы.

use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, BusType, EventType, InputEvent, InputId, KeyCode};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM, SIGUSR1};
use signal_hook::iterator::Signals;

use crate::daemon::Msg;
use crate::devices::UINPUT_NAME;
use crate::engine::{Keys, Stroke};

/// Производитель 0x0FAC keyd считает своим и не перехватывает: иначе он
/// применил бы свои замены клавиш к уже заменённым нажатиям ещё раз.
pub const UINPUT_VENDOR: u16 = 0x0fac;
pub const UINPUT_PRODUCT: u16 = 0x5f5f;

pub struct Injector {
    dev: VirtualDevice,
    delay: Duration,
}

impl Injector {
    pub fn new(delay: Duration) -> Result<Self> {
        let mut keys = AttributeSet::<KeyCode>::new();
        for code in KeyCode::KEY_ESC.code()..=KeyCode::KEY_MICMUTE.code() {
            keys.insert(KeyCode::new(code));
        }
        let id = InputId::new(BusType::BUS_VIRTUAL, UINPUT_VENDOR, UINPUT_PRODUCT, 1);
        let dev = VirtualDevice::builder()
            .and_then(|b| b.name(UINPUT_NAME).input_id(id).with_keys(&keys))
            .and_then(evdev::uinput::VirtualDeviceBuilder::build)
            .context("нет доступа к /dev/uinput — нужно udev-правило из install.sh")?;
        thread::sleep(Duration::from_millis(300)); // композитору нужно время, чтобы подхватить устройство
        Ok(Injector { dev, delay })
    }

    fn emit(&mut self, key: KeyCode, value: i32) -> Result<()> {
        self.dev
            .emit(&[InputEvent::new(EventType::KEY.0, key.code(), value)])
            .context("uinput: запись не удалась")?;
        thread::sleep(self.delay);
        Ok(())
    }

    fn strokes(&mut self, strokes: &[Stroke], shift: &mut bool) -> Result<()> {
        for s in strokes {
            if s.shift != *shift {
                *shift = s.shift;
                self.emit(KeyCode::KEY_LEFTSHIFT, i32::from(s.shift))?;
            }
            self.emit(s.key, 1)?;
            self.emit(s.key, 0)?;
        }
        Ok(())
    }
}

impl Keys for Injector {
    fn configure(&mut self, cfg: &crate::config::Config) {
        self.delay = cfg.key_delay;
    }

    fn backspace(&mut self, count: usize) -> Result<()> {
        for _ in 0..count {
            self.emit(KeyCode::KEY_BACKSPACE, 1)?;
            self.emit(KeyCode::KEY_BACKSPACE, 0)?;
        }
        Ok(())
    }

    fn type_strokes(&mut self, strokes: &[Stroke]) -> Result<()> {
        let mut shift = false;
        let result = self.strokes(strokes, &mut shift);
        if shift {
            self.emit(KeyCode::KEY_LEFTSHIFT, 0)?; // не оставить Shift зажатым
        }
        result
    }

    fn combo(&mut self, keys: &[KeyCode]) -> Result<()> {
        let pressed = keys.iter().try_for_each(|&k| self.emit(k, 1));
        let released = keys.iter().rev().try_for_each(|&k| self.emit(k, 0));
        pressed.and(released)
    }

    fn chord(&mut self, hold: &[KeyCode], tap: KeyCode, times: usize) -> Result<()> {
        let pressed = hold.iter().try_for_each(|&k| self.emit(k, 1));
        let tapped = pressed.and_then(|()| {
            (0..times).try_for_each(|_| {
                self.emit(tap, 1)?;
                self.emit(tap, 0)
            })
        });
        // Модификаторы отпускаем в любом случае — не оставить их зажатыми.
        let released = hold.iter().rev().try_for_each(|&k| self.emit(k, 0));
        tapped.and(released)
    }
}

// ─── Сигналы ────────────────────────────────────────────────────────────────

/// SIGINT/SIGTERM — выход, SIGUSR1 — пауза, SIGHUP — перечитать конфиг.
pub fn spawn_signals(tx: Sender<Msg>) -> Result<()> {
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

//! Виртуальная клавиатура uinput: через неё служба стирает и перенабирает текст.

use std::io;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, BusType, EventType, InputEvent, InputId, KeyCode};

use crate::devices::OWN_KEYBOARD;
use crate::engine::Keys;
use crate::keys::Stroke;

/// Производитель 0x0FAC keyd считает своим и не перехватывает: иначе он
/// применил бы свои замены клавиш к уже заменённым нажатиям ещё раз.
const VENDOR: u16 = 0x0fac;
const PRODUCT: u16 = 0x5f5f;

/// Куда пишутся события: устройство uinput, в тестах — запись.
pub trait Device {
    fn write(&mut self, event: InputEvent) -> io::Result<()>;
}

impl Device for VirtualDevice {
    fn write(&mut self, event: InputEvent) -> io::Result<()> {
        self.emit(&[event])
    }
}

pub struct Injector<D = VirtualDevice> {
    dev: D,
    delay: Duration,
    /// Нажатые и ещё не отпущенные клавиши, по порядку нажатия.
    held: Vec<KeyCode>,
}

impl Injector {
    pub fn new(delay: Duration) -> Result<Self> {
        let mut keys = AttributeSet::<KeyCode>::new();
        for code in KeyCode::KEY_ESC.code()..=KeyCode::KEY_MICMUTE.code() {
            keys.insert(KeyCode::new(code));
        }
        let id = InputId::new(BusType::BUS_VIRTUAL, VENDOR, PRODUCT, 1);
        let dev = VirtualDevice::builder()
            .and_then(|b| b.name(OWN_KEYBOARD).input_id(id).with_keys(&keys))
            .and_then(evdev::uinput::VirtualDeviceBuilder::build)
            .context("нет доступа к /dev/uinput — нужно udev-правило из install.sh")?;
        thread::sleep(Duration::from_millis(300)); // композитору нужно время, чтобы подхватить устройство
        Ok(Injector {
            dev,
            delay,
            held: Vec::new(),
        })
    }
}

impl<D: Device> Injector<D> {
    fn emit(&mut self, key: KeyCode, value: i32) -> Result<()> {
        self.dev
            .write(InputEvent::new(EventType::KEY.0, key.code(), value))
            .context("uinput: запись не удалась")?;
        if value == 1 {
            self.held.push(key);
        } else {
            self.held.retain(|&k| k != key);
        }
        thread::sleep(self.delay);
        Ok(())
    }

    fn tap(&mut self, key: KeyCode) -> Result<()> {
        self.emit(key, 1)?;
        self.emit(key, 0)
    }

    /// Отпустить всё, что осталось зажатым, — каждую клавишу, даже если
    /// какая-то не отпустилась, а её — ещё раз. Ошибка — исходная, если она была.
    fn finish(&mut self, result: Result<()>) -> Result<()> {
        let mut released = Ok(());
        for _ in 0..2 {
            for key in self.held.clone().into_iter().rev() {
                released = released.and(self.emit(key, 0));
            }
        }
        result.and(released)
    }

    fn strokes(&mut self, strokes: &[Stroke]) -> Result<()> {
        let mut shift = false;
        for s in strokes {
            if s.shift != shift {
                shift = s.shift;
                self.emit(KeyCode::KEY_LEFTSHIFT, i32::from(s.shift))?;
            }
            self.tap(s.key)?;
        }
        Ok(())
    }
}

// Каждый метод в конце отпускает всё, что нажал, в том числе после ошибки:
// иначе клавиша залипнет и композитор начнёт её повторять.
impl<D: Device> Keys for Injector<D> {
    fn configure(&mut self, cfg: &crate::config::Config) {
        self.delay = cfg.key_delay;
    }

    fn backspace(&mut self, count: usize) -> Result<()> {
        let result = (0..count).try_for_each(|_| self.tap(KeyCode::KEY_BACKSPACE));
        self.finish(result)
    }

    fn type_strokes(&mut self, strokes: &[Stroke]) -> Result<()> {
        let result = self.strokes(strokes);
        self.finish(result) // и Shift, если последняя буква была заглавной
    }

    fn combo(&mut self, keys: &[KeyCode]) -> Result<()> {
        let pressed = keys.iter().try_for_each(|&k| self.emit(k, 1));
        self.finish(pressed)
    }

    fn chord(&mut self, hold: &[KeyCode], tap: KeyCode, times: usize) -> Result<()> {
        let tapped = hold
            .iter()
            .try_for_each(|&k| self.emit(k, 1))
            .and_then(|()| (0..times).try_for_each(|_| self.tap(tap)));
        self.finish(tapped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use KeyCode as K;

    /// Пишет события в список; записи из `fail` не удаются по разу.
    #[derive(Default)]
    struct Recorder {
        events: Vec<(KeyCode, i32)>,
        fail: Vec<(KeyCode, i32)>,
    }

    impl Device for Recorder {
        fn write(&mut self, event: InputEvent) -> io::Result<()> {
            let ev = (KeyCode::new(event.code()), event.value());
            if let Some(i) = self.fail.iter().position(|&f| f == ev) {
                self.fail.remove(i);
                return Err(io::Error::other(format!("{:?} {}", ev.0, ev.1)));
            }
            self.events.push(ev);
            Ok(())
        }
    }

    fn injector(fail: &[(KeyCode, i32)]) -> Injector<Recorder> {
        Injector {
            dev: Recorder {
                events: Vec::new(),
                fail: fail.to_vec(),
            },
            delay: Duration::ZERO,
            held: Vec::new(),
        }
    }

    /// Клавиши, которые после всех событий остались нажатыми.
    fn stuck(events: &[(KeyCode, i32)]) -> Vec<KeyCode> {
        let mut down = Vec::new();
        for &(key, value) in events {
            if value == 1 {
                down.push(key);
            } else {
                down.retain(|&k| k != key);
            }
        }
        down
    }

    fn stroke(key: KeyCode, shift: bool) -> Stroke {
        Stroke {
            key,
            shift,
            layout: None,
        }
    }

    #[test]
    fn sequences_without_errors() {
        let mut inj = injector(&[]);
        inj.combo(&[K::KEY_LEFTCTRL, K::KEY_V]).unwrap();
        inj.chord(&[K::KEY_LEFTALT], K::KEY_TAB, 2).unwrap();
        inj.type_strokes(&[
            stroke(K::KEY_A, true),
            stroke(K::KEY_B, false),
            stroke(K::KEY_A, true),
        ])
        .unwrap();
        inj.backspace(1).unwrap();
        #[rustfmt::skip]
        assert_eq!(inj.dev.events, [
            (K::KEY_LEFTCTRL, 1), (K::KEY_V, 1), (K::KEY_V, 0), (K::KEY_LEFTCTRL, 0),
            (K::KEY_LEFTALT, 1), (K::KEY_TAB, 1), (K::KEY_TAB, 0), (K::KEY_TAB, 1), (K::KEY_TAB, 0), (K::KEY_LEFTALT, 0),
            (K::KEY_LEFTSHIFT, 1), (K::KEY_A, 1), (K::KEY_A, 0), (K::KEY_LEFTSHIFT, 0), (K::KEY_B, 1), (K::KEY_B, 0),
            (K::KEY_LEFTSHIFT, 1), (K::KEY_A, 1), (K::KEY_A, 0), (K::KEY_LEFTSHIFT, 0),
            (K::KEY_BACKSPACE, 1), (K::KEY_BACKSPACE, 0),
        ]);
    }

    #[test]
    fn nothing_stays_pressed_after_error() {
        // V не отпустился — Ctrl всё равно отпускается, V — ещё раз.
        let mut inj = injector(&[(K::KEY_V, 0)]);
        assert!(inj.combo(&[K::KEY_LEFTCTRL, K::KEY_V]).is_err());
        assert_eq!(stuck(&inj.dev.events), []);

        let mut inj = injector(&[(K::KEY_TAB, 0)]);
        assert!(inj.chord(&[K::KEY_LEFTALT], K::KEY_TAB, 3).is_err());
        assert_eq!(stuck(&inj.dev.events), []);

        let mut inj = injector(&[(K::KEY_A, 0)]);
        assert!(inj.type_strokes(&[stroke(K::KEY_A, true)]).is_err());
        assert_eq!(stuck(&inj.dev.events), []);
    }

    #[test]
    fn first_error_is_reported() {
        // Не нажалась буква, а потом не отпустился Shift: ошибка — про букву.
        let mut inj = injector(&[(K::KEY_A, 1), (K::KEY_LEFTSHIFT, 0)]);
        let err = inj.type_strokes(&[stroke(K::KEY_A, true)]).unwrap_err();
        assert!(format!("{err:#}").contains("KEY_A 1"), "{err:#}");
    }
}

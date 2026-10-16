//! Виртуальная клавиатура uinput, слежение за PRIMARY и сигналами.

use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::process::Stdio;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventType, InputEvent, KeyCode};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM, SIGUSR1};
use signal_hook::iterator::Signals;

use crate::daemon::Msg;
use crate::devices::UINPUT_NAME;
use crate::engine::{Keys, Stroke};
use crate::env;

// ─── Виртуальная клавиатура ─────────────────────────────────────────────────

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
        let dev = VirtualDevice::builder()
            .and_then(|b| b.name(UINPUT_NAME).with_keys(&keys))
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
}

// ─── PRIMARY ────────────────────────────────────────────────────────────────

/// `wl-paste --primary --watch` печатает размер каждого нового выделения.
/// Сессии ещё нет или wl-clipboard не установлен — ждём и пробуем снова.
pub fn spawn_primary_watcher(tx: Sender<Msg>) {
    let spawned = thread::Builder::new()
        .name("primary".into())
        .spawn(move || watch_primary(&tx));
    if let Err(err) = spawned {
        log::warn!("поток PRIMARY не запустился: {err}");
    }
}

fn watch_primary(tx: &Sender<Msg>) {
    let mut failures = 0u32;
    loop {
        let wait = if env::wayland_display().is_none() {
            Some(("нет Wayland-сессии", 5))
        } else if !env::in_path("wl-paste") {
            Some(("не установлен wl-clipboard", 30))
        } else {
            None
        };
        if let Some((reason, secs)) = wait {
            if tx.send(Msg::Selection(Some(reason.into()))).is_err() {
                return;
            }
            thread::sleep(Duration::from_secs(secs));
            continue;
        }
        let started = Instant::now();
        let mut cmd = env::session_command("wl-paste");
        cmd.args(["--primary", "--watch", "sh", "-c", "wc -c"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // SAFETY: в pre_exec только async-signal-safe prctl: wl-paste завершится вместе с этим потоком.
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong);
                Ok(())
            });
        }
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                let _ = tx.send(Msg::Selection(Some(format!("wl-paste: {err}"))));
                thread::sleep(Duration::from_secs(30));
                continue;
            }
        };
        let mut stderr = child.stderr.take().expect("stderr");
        let errors = thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        });
        let mut announced = false;
        for line in BufReader::new(child.stdout.take().expect("stdout")).lines() {
            let Ok(line) = line else { break };
            if !announced {
                announced = true;
                failures = 0;
                if tx.send(Msg::Selection(None)).is_err() {
                    let _ = child.kill();
                    return;
                }
            }
            if started.elapsed() < Duration::from_millis(700) {
                continue; // первое событие — уже существующее выделение
            }
            if let Ok(size) = line.trim().parse()
                && tx.send(Msg::Primary(size)).is_err()
            {
                let _ = child.kill();
                return;
            }
        }
        let _ = child.wait();
        let err = errors.join().unwrap_or_default();
        failures += 1;
        let reason = if err.trim().is_empty() {
            "wl-paste завершился".to_owned()
        } else {
            err.trim().to_owned()
        };
        if tx.send(Msg::Selection(Some(reason))).is_err() {
            return;
        }
        // Композитор перезапущен или не умеет PRIMARY — пробуем реже и реже.
        thread::sleep(Duration::from_secs(
            2u64.saturating_pow(failures.min(6)).min(60),
        ));
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

//! Лог: stderr (под systemd — с приоритетами журнала) и кольцевой буфер
//! последних строк для вкладки «Журнал» в GUI. Набранного текста в логе нет.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const KEEP: usize = 300;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogLine {
    /// Unix-время, секунды.
    pub time: u64,
    pub level: String,
    pub text: String,
}

static LINES: Mutex<VecDeque<LogLine>> = Mutex::new(VecDeque::new());

struct Logger {
    journal: bool,
}

impl log::Log for Logger {
    fn enabled(&self, meta: &log::Metadata) -> bool {
        meta.level() <= log::max_level() && meta.target().starts_with("shiftfix")
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let text = record.args().to_string();
        if self.journal {
            let priority = match record.level() {
                log::Level::Error => 3,
                log::Level::Warn => 4,
                log::Level::Info => 6,
                _ => 7,
            };
            eprintln!("<{priority}>{text}");
        } else {
            eprintln!("{}: {text}", record.level());
        }
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        if let Ok(mut lines) = LINES.lock() {
            if lines.len() == KEEP {
                lines.pop_front();
            }
            lines.push_back(LogLine {
                time,
                level: record.level().to_string(),
                text,
            });
        }
    }

    fn flush(&self) {}
}

pub fn init(debug: bool) {
    if log::set_boxed_logger(Box::new(Logger {
        journal: stderr_is_journal(),
    }))
    .is_ok()
    {
        log::set_max_level(if debug {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Info
        });
    }
}

pub fn recent() -> Vec<LogLine> {
    LINES
        .lock()
        .map(|lines| lines.iter().cloned().collect())
        .unwrap_or_default()
}

/// systemd передаёт в JOURNAL_STREAM «устройство:inode» своего сокета; сравниваем с stderr.
#[allow(clippy::unnecessary_cast)]
fn stderr_is_journal() -> bool {
    let Some(value) = std::env::var_os("JOURNAL_STREAM") else {
        return false;
    };
    let Some((dev, ino)) = value.to_str().and_then(|v| v.split_once(':')) else {
        return false;
    };
    let (Ok(dev), Ok(ino)) = (dev.parse::<u64>(), ino.parse::<u64>()) else {
        return false;
    };
    // SAFETY: fstat заполняет локальную структуру.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    (unsafe { libc::fstat(2, &mut st) }) == 0 && st.st_dev as u64 == dev && st.st_ino as u64 == ino
}

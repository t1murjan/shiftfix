//! Лог: stderr (под systemd — с приоритетами журнала) и кольцевой буфер
//! последних строк для вкладки «Журнал» в GUI. Набранного текста в логе нет.

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::io::{self, Write};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const KEEP: usize = 300;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
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
        // Не eprintln!: он паникует, если stderr не пишется (закрытый терминал —
        // EIO), а журнал не должен ронять службу.
        let _ = if self.journal {
            let priority = match record.level() {
                log::Level::Error => 3,
                log::Level::Warn => 4,
                log::Level::Info => 6,
                _ => 7,
            };
            writeln!(io::stderr().lock(), "<{priority}>{text}")
        } else {
            writeln!(io::stderr().lock(), "{}: {text}", record.level())
        };
        remember(record.level(), text);
    }

    fn flush(&self) {}
}

/// Строка для вкладки «Журнал»: хранятся KEEP последних.
fn remember(level: log::Level, text: String) {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if let Ok(mut lines) = LINES.lock() {
        if lines.len() == KEEP {
            lines.pop_front();
        }
        lines.push_back(LogLine {
            time,
            level: level.to_string(),
            text,
        });
    }
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
    let Some((dev, ino)) = std::env::var_os("JOURNAL_STREAM")
        .as_deref()
        .and_then(journal_stream)
    else {
        return false;
    };
    // SAFETY: fstat заполняет локальную структуру.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    (unsafe { libc::fstat(2, &mut st) }) == 0 && st.st_dev as u64 == dev && st.st_ino as u64 == ino
}

/// Значение JOURNAL_STREAM: (устройство, inode).
fn journal_stream(value: &OsStr) -> Option<(u64, u64)> {
    let (dev, ino) = value.to_str()?.split_once(':')?;
    Some((dev.parse().ok()?, ino.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_stream_value() {
        assert_eq!(journal_stream(OsStr::new("8:12345")), Some((8, 12345)));
        for bad in ["", "8", "8:", ":1", "a:1", "8:1:2", "-1:5"] {
            assert_eq!(journal_stream(OsStr::new(bad)), None, "{bad}");
        }
    }

    #[test]
    fn keeps_last_lines() {
        for n in 0..KEEP + 5 {
            remember(log::Level::Info, n.to_string());
        }
        let lines = recent();
        assert_eq!(lines.len(), KEEP);
        assert_eq!(lines[0].text, "5");
        assert_eq!(lines[KEEP - 1].text, (KEEP + 4).to_string());
        assert_eq!(lines[0].level, "INFO");
    }
}

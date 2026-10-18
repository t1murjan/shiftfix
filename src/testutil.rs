//! Общее для модульных тестов.

use std::fs;
use std::io::{BufRead, BufReader};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;
use evdev::KeyCode;

use crate::engine::Keys;
use crate::keys::Stroke;

/// Эмуляция нажатий, которая ничего не делает.
pub struct NoKeys;

impl Keys for NoKeys {
    fn backspace(&mut self, _: usize) -> Result<()> {
        Ok(())
    }

    fn type_strokes(&mut self, _: &[Stroke]) -> Result<()> {
        Ok(())
    }

    fn combo(&mut self, _: &[KeyCode]) -> Result<()> {
        Ok(())
    }

    fn chord(&mut self, _: &[KeyCode], _: KeyCode, _: usize) -> Result<()> {
        Ok(())
    }
}

/// Пропуск теста, которому не хватает внешней программы или библиотеки:
/// `return skip("нет dbus-daemon")`. С `SHIFTFIX_TEST_STRICT=1` (так в CI)
/// тест вместо этого падает — там всё нужное установлено.
pub fn skip(reason: &str) {
    if std::env::var_os("SHIFTFIX_TEST_STRICT").is_some_and(|v| v == "1") {
        panic!("{reason}, а пропуск запрещён: SHIFTFIX_TEST_STRICT=1");
    }
    eprintln!("— пропуск: {reason}");
}

/// Пустой временный каталог `shiftfix-<tag>-<pid>-<n>`, свой у каждого вызова:
/// тесты идут параллельно. Удаляется в Drop, в том числе при панике. В нём
/// бывают Unix-сокеты, а их путь — не длиннее 107 байт: при длинном TMPDIR
/// каталог создаётся в /tmp.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let base = Some(std::env::temp_dir())
            .filter(|t| t.as_os_str().len() <= 60)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let dir = base.join(format!("shiftfix-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir); // остаток прогона с тем же pid
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}

impl Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Исполняемый скрипт `dir/name`: `#!/bin/sh` и `body`. Возвращает путь к нему.
///
/// Файл пишет отдельный процесс `sh`, а не этот. Иначе, пока файл открыт на
/// запись, соседний тест может запустить свой процесс: копия дескриптора
/// живёт в нём от fork до exec, и запуск скрипта в это время падает с
/// ETXTBSY («Text file busy»). Дескриптор, открытый в `sh`, к нам не попадает.
pub fn write_script(dir: &Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    let written = Command::new("sh")
        .args(["-c", r#"printf '%s' "$1" > "$0" && chmod 755 "$0""#])
        .arg(&path)
        .arg(format!("#!/bin/sh\n{body}"))
        .status()
        .unwrap();
    assert!(written.success(), "не записать {}", path.display());
    path.to_string_lossy().into_owned()
}

/// Временный исполняемый скрипт вместо внешней команды (hyprctl, swaymsg,
/// gsettings…): каталог со скриптом и путь к нему.
pub fn fake_command(name: &str, body: &str) -> (TempDir, String) {
    let dir = TempDir::new(name);
    let path = write_script(&dir, name, body);
    (dir, path)
}

/// Отдельная шина DBus на время теста; при выходе, в том числе при панике, —
/// kill и wait.
pub struct TestBus {
    child: Child,
    pub address: String,
}

impl TestBus {
    /// `None` — dbus-daemon не запускается.
    pub fn start() -> Option<TestBus> {
        let child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address"])
            .stdout(Stdio::piped())
            .spawn()
            .ok()?;
        let mut bus = TestBus {
            child,
            address: String::new(),
        };
        let stdout = bus.child.stdout.take().unwrap();
        BufReader::new(stdout).read_line(&mut bus.address).unwrap();
        bus.address = bus.address.trim().to_owned();
        assert!(!bus.address.is_empty(), "dbus-daemon не сообщил адрес");
        Some(bus)
    }
}

impl Drop for TestBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

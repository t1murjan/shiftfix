//! Общее для тестов окна: копии помощников из `testutil` библиотеки с теми же
//! именами (её тестовый код, `cfg(test)`, окну недоступен) и своя шина DBus.

use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Пустой временный каталог `shiftfix-<tag>-<pid>-<n>`, свой у каждого вызова:
/// тесты идут параллельно. Удаляется в Drop, в том числе при панике. В нём
/// бывают Unix-сокеты, а их путь — не длиннее 107 байт: при длинном TMPDIR
/// каталог создаётся в /tmp.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let base = Some(env::temp_dir())
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
/// Файл пишет отдельный `sh`, а не этот процесс: иначе соседний тест,
/// запуская процесс, унаследовал бы открытый на запись файл, и запуск скрипта
/// упал бы с ETXTBSY.
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

/// Пропуск теста, которому не хватает dbus-daemon; с `SHIFTFIX_TEST_STRICT=1`
/// (так в CI) — падение. Копия `testutil::skip` библиотеки.
pub fn skip(reason: &str) {
    if std::env::var_os("SHIFTFIX_TEST_STRICT").is_some_and(|v| v == "1") {
        panic!("{reason}, а пропуск запрещён: SHIFTFIX_TEST_STRICT=1");
    }
    eprintln!("— пропуск: {reason}");
}

/// Без servicedirs: иначе шина сама запускала бы настоящие службы, например
/// xdg-desktop-portal. Сокет — в каталоге теста (`{dir}`), он удаляется
/// вместе с каталогом даже после kill.
const BUS_CONFIG: &str = r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir={dir}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#;

/// Отдельная шина DBus на время теста; при выходе (и при панике) — kill и
/// wait, каталог теста удаляется.
pub struct Bus {
    child: Child,
    /// Каталог теста: в нём сокет шины, тест может класть туда свои файлы.
    pub dir: TempDir,
    pub address: String,
}

impl Bus {
    /// `None` — dbus-daemon нет.
    pub fn start() -> Option<Bus> {
        let dir = TempDir::new("bus");
        let config = dir.join("bus.conf");
        let text = BUS_CONFIG.replace("{dir}", &dir.display().to_string());
        fs::write(&config, text).unwrap();
        let child = Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--print-address"])
            .stdout(Stdio::piped())
            .spawn()
            .ok()?;
        let mut bus = Bus {
            child,
            dir,
            address: String::new(),
        };
        let stdout = bus.child.stdout.take().unwrap();
        BufReader::new(stdout).read_line(&mut bus.address).unwrap();
        bus.address = bus.address.trim().to_owned();
        assert!(!bus.address.is_empty(), "dbus-daemon не сообщил адрес");
        Some(bus)
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Каталог теста удалит поле `dir` — уже после этого.
    }
}

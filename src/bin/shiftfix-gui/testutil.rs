//! Общее для тестов окна. Тестовый код библиотеки (`testutil` в `src/`) окну
//! недоступен: он собирается только для её собственных тестов.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

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
    pub dir: PathBuf,
    pub address: String,
}

impl Bus {
    /// `None` — dbus-daemon нет.
    pub fn start() -> Option<Bus> {
        static N: AtomicUsize = AtomicUsize::new(0);
        // Путь сокета — не длиннее 108 байт.
        let base = Some(std::env::temp_dir())
            .filter(|t| t.as_os_str().len() <= 60)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let dir = base.join(format!(
            "shiftfix-bus-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        let config = dir.join("bus.conf");
        let text = BUS_CONFIG.replace("{dir}", &dir.display().to_string());
        fs::write(&config, text).unwrap();
        let spawned = Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--print-address"])
            .stdout(Stdio::piped())
            .spawn();
        let Ok(child) = spawned else {
            let _ = fs::remove_dir_all(&dir);
            return None;
        };
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
        let _ = fs::remove_dir_all(&self.dir);
    }
}

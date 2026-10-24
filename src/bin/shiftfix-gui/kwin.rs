//! KDE Plasma в Wayland: окно не может само встать у значка в трее, поэтому
//! попап просит об этом KWin. Перед показом попапа по DBus загружается
//! маленький KWin-скрипт (`kwin_popup.js`): он ставит окно попапа у курсора
//! и держит поверх окон. Когда попап закрылся, скрипт выгружается. Настройки
//! KWin не меняются.
//!
//! KWin загружает скрипт из файла и читает файл, когда запускает скрипт, —
//! после этого файл удаляется.

use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use shiftfix::sys::env;
use zbus::blocking::Connection;
use zbus::zvariant::{DynamicType, Type};

use crate::placement::GAP;
use crate::popup::APP_ID;

const SCRIPT: &str = include_str!("kwin_popup.js");

const KWIN: &str = "org.kde.KWin";
const SCRIPTING: &str = "/Scripting";
const SCRIPTING_IFACE: &str = "org.kde.kwin.Scripting";
const SCRIPT_IFACE: &str = "org.kde.kwin.Script";
const UNKNOWN_OBJECT: &str = "org.freedesktop.DBus.Error.UnknownObject";

/// Имя скрипта в KWin: по нему скрипт выгружается.
const NAME: &str = "shiftfix-popup";

/// Дольше попап KWin не ждёт и открывается, где тот решит.
const TIMEOUT: Duration = Duration::from_secs(2);

/// Это KDE Plasma.
pub fn running() -> bool {
    env::desktop().split(':').any(|desktop| desktop == "kde")
}

/// Скрипт попапа, загруженный в KWin.
pub struct Script {
    conn: Connection,
}

impl Script {
    /// Загрузить и запустить скрипт. `file` — куда записать его для KWin.
    pub fn load(file: &Path) -> Result<Script> {
        Script::load_on(connect(None)?, file)
    }

    fn load_on(conn: Connection, file: &Path) -> Result<Script> {
        // Попап, который не успел выгрузить свой скрипт (его убили), оставил
        // его в KWin, а второй с тем же именем KWin не загрузит.
        unload(&conn)?;
        fs::write(file, script_text())
            .with_context(|| format!("не записать {}", file.display()))?;
        let started = load_and_run(&conn, file);
        let _ = fs::remove_file(file);
        started?;
        Ok(Script { conn })
    }

    /// Выгрузить скрипт: попап закрылся.
    pub fn unload(self) -> Result<()> {
        unload(&self.conn)
    }
}

fn connect(address: Option<&str>) -> Result<Connection> {
    let builder = match address {
        Some(address) => zbus::blocking::connection::Builder::address(address),
        None => zbus::blocking::connection::Builder::session(),
    };
    builder
        .and_then(|b| b.method_timeout(TIMEOUT).build())
        .context("нет сессионной шины DBus")
}

/// Текст скрипта: константы из кода попапа и `kwin_popup.js`.
fn script_text() -> String {
    format!("const CLASS = \"{APP_ID}\";\nconst GAP = {GAP};\n{SCRIPT}")
}

fn load_and_run(conn: &Connection, file: &Path) -> Result<()> {
    let path = file.to_str().context("путь к скрипту — не UTF-8")?;
    let id: i32 = call(
        conn,
        SCRIPTING,
        SCRIPTING_IFACE,
        "loadScript",
        &(path, NAME),
    )
    .context("loadScript")?;
    if id < 0 {
        bail!("loadScript: скрипт {NAME} не загрузился");
    }
    // KWin отвечает на run, когда скрипт уже выполнен: окно попапа, созданное
    // после этого, скрипт не пропустит. Plasma 6 называет объект скрипта
    // /Scripting/Script<id>, Plasma 5 — /<id>.
    let run = |path: &str| call::<()>(conn, path, SCRIPT_IFACE, "run", &());
    let ran = match run(&format!("/Scripting/Script{id}")) {
        Err(zbus::Error::MethodError(name, ..)) if name.as_str() == UNKNOWN_OBJECT => {
            run(&format!("/{id}"))
        }
        other => other,
    };
    ran.context("run")
}

fn unload(conn: &Connection) -> Result<()> {
    // Ответ — был ли такой скрипт; не было — тоже хорошо.
    let _unloaded: bool =
        call(conn, SCRIPTING, SCRIPTING_IFACE, "unloadScript", &(NAME,)).context("unloadScript")?;
    Ok(())
}

fn call<R>(
    conn: &Connection,
    path: &str,
    iface: &str,
    method: &str,
    body: &(impl serde::Serialize + DynamicType),
) -> zbus::Result<R>
where
    R: DeserializeOwned + Type,
{
    conn.call_method(Some(KWIN), path, Some(iface), method, body)?
        .body()
        .deserialize()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::testutil::{Bus, skip};

    /// Что делали с поддельным KWin: вызовы по порядку и текст файла
    /// скрипта, каким его застал `run`.
    #[derive(Default)]
    struct Log {
        calls: Vec<String>,
        file: String,
        read_at_run: Option<String>,
    }

    type Shared = Arc<Mutex<Log>>;

    /// `/Scripting` поддельного KWin: имена методов и типы как в Plasma.
    struct FakeScripting {
        log: Shared,
        /// Что ответить на loadScript: номер скрипта, -1 — не загружен.
        id: i32,
    }

    #[zbus::interface(name = "org.kde.kwin.Scripting")]
    impl FakeScripting {
        #[zbus(name = "loadScript")]
        fn load_script(&self, file: String, name: String) -> i32 {
            let mut log = self.log.lock().unwrap();
            log.calls.push(format!("loadScript {name}"));
            log.file = file;
            self.id
        }

        #[zbus(name = "unloadScript")]
        fn unload_script(&self, name: String) -> bool {
            self.log
                .lock()
                .unwrap()
                .calls
                .push(format!("unloadScript {name}"));
            false
        }
    }

    /// Объект загруженного скрипта.
    struct FakeScript {
        log: Shared,
        path: &'static str,
    }

    #[zbus::interface(name = "org.kde.kwin.Script")]
    impl FakeScript {
        #[zbus(name = "run")]
        fn run(&self) {
            let mut log = self.log.lock().unwrap();
            log.calls.push(format!("run {}", self.path));
            log.read_at_run = fs::read_to_string(&log.file).ok();
        }
    }

    /// Поддельный KWin на шине `bus`: loadScript отвечает `id`, объект
    /// скрипта — по пути `script_path`.
    fn fake_kwin(bus: &Bus, id: i32, script_path: &'static str) -> (Connection, Shared) {
        let log = Shared::default();
        let scripting = FakeScripting {
            log: log.clone(),
            id,
        };
        let script = FakeScript {
            log: log.clone(),
            path: script_path,
        };
        let server = zbus::blocking::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .name(KWIN)
            .unwrap()
            .serve_at(SCRIPTING, scripting)
            .unwrap()
            .serve_at(script_path, script)
            .unwrap()
            .build()
            .unwrap();
        (server, log)
    }

    fn calls(log: &Shared) -> Vec<String> {
        log.lock().unwrap().calls.clone()
    }

    #[test]
    fn loads_runs_and_unloads_script() {
        let Some(bus) = Bus::start() else {
            return skip("нет dbus-daemon");
        };
        let (_server, log) = fake_kwin(&bus, 0, "/Scripting/Script0");
        let file = bus.dir.join("popup.js");
        let conn = connect(Some(&bus.address)).unwrap();
        let script = Script::load_on(conn, &file).unwrap();
        // Старый скрипт попапа выгружен, новый загружен из файла и запущен.
        assert_eq!(
            calls(&log),
            [
                "unloadScript shiftfix-popup",
                "loadScript shiftfix-popup",
                "run /Scripting/Script0",
            ]
        );
        assert_eq!(log.lock().unwrap().file, file.to_str().unwrap());
        // Запуская скрипт, KWin читает файл: в нём константы и сам скрипт.
        let text = log.lock().unwrap().read_at_run.clone();
        let text = text.expect("файла скрипта не было, когда KWin его запускал");
        assert!(
            text.starts_with("const CLASS = \"shiftfix-popup\";\nconst GAP = 8;\n// KWin-скрипт"),
            "{text}"
        );
        assert!(!file.exists(), "файл больше не нужен");

        script.unload().unwrap();
        assert_eq!(calls(&log).last().unwrap(), "unloadScript shiftfix-popup");
    }

    #[test]
    fn plasma_5_script_object() {
        let Some(bus) = Bus::start() else {
            return skip("нет dbus-daemon");
        };
        // Plasma 5: объекта /Scripting/Script3 нет, скрипт — /3.
        let (_server, log) = fake_kwin(&bus, 3, "/3");
        let file = bus.dir.join("popup.js");
        let conn = connect(Some(&bus.address)).unwrap();
        Script::load_on(conn, &file).unwrap();
        assert_eq!(calls(&log).last().unwrap(), "run /3");
        assert!(log.lock().unwrap().read_at_run.is_some());
    }

    #[test]
    fn script_not_loaded_is_error() {
        let Some(bus) = Bus::start() else {
            return skip("нет dbus-daemon");
        };
        let (_server, log) = fake_kwin(&bus, -1, "/Scripting/Script0");
        let file = bus.dir.join("popup.js");
        let conn = connect(Some(&bus.address)).unwrap();
        let err = Script::load_on(conn, &file).err().unwrap();
        assert_eq!(
            err.to_string(),
            "loadScript: скрипт shiftfix-popup не загрузился"
        );
        assert!(!calls(&log).iter().any(|call| call.starts_with("run")));
        assert!(!file.exists());
    }

    #[test]
    fn no_kwin_is_error() {
        let Some(bus) = Bus::start() else {
            return skip("нет dbus-daemon");
        };
        let file = bus.dir.join("popup.js");
        let conn = connect(Some(&bus.address)).unwrap();
        let err = Script::load_on(conn, &file).err().unwrap();
        assert!(format!("{err:#}").contains(KWIN), "{err:#}");
        assert!(!file.exists(), "до файла дело не дошло");
    }
}

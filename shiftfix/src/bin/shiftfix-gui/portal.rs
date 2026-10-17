//! Тема системы из XDG Desktop Portal: color-scheme и accent-color из
//! `org.freedesktop.portal.Settings`. Нужна окну для режима «как в системе»;
//! служба этот модуль не вызывает.
//!
//! Портал читается в своём потоке: холодный старт xdg-desktop-portal (а без
//! бэкенда — зависание) длится секунды, и окно не должно его ждать.

use std::thread;
use std::time::Duration;

use zbus::blocking::proxy::{Builder as ProxyBuilder, SignalIterator};
use zbus::blocking::{Connection, Proxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::{OwnedValue, Value};

pub const DEST: &str = "org.freedesktop.portal.Desktop";
pub const PATH: &str = "/org/freedesktop/portal/desktop";
pub const IFACE: &str = "org.freedesktop.portal.Settings";
/// Пространство ключей оформления.
pub const NS: &str = "org.freedesktop.appearance";
pub const SCHEME_KEY: &str = "color-scheme";
pub const ACCENT_KEY: &str = "accent-color";

/// Дольше ответа не ждём: без бэкенда портал может молчать до 25 с.
const TIMEOUT: Duration = Duration::from_secs(5);

const UNKNOWN_METHOD: &str = "org.freedesktop.DBus.Error.UnknownMethod";
const SERVICE_UNKNOWN: &str = "org.freedesktop.DBus.Error.ServiceUnknown";
const NAME_HAS_NO_OWNER: &str = "org.freedesktop.DBus.Error.NameHasNoOwner";
const NOT_FOUND: &str = "org.freedesktop.portal.Error.NotFound";

/// color-scheme портала.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    /// 0 (и любое незнакомое число) — система не выражает предпочтения.
    NoPreference,
    /// 1.
    Dark,
    /// 2.
    Light,
}

/// Что портал сообщает об оформлении.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Appearance {
    /// `None` — портал такого ключа не знает.
    pub scheme: Option<Scheme>,
    /// sRGB; `None` — ключа нет или цвет не задан.
    pub accent: Option<[u8; 3]>,
}

impl Appearance {
    /// Учесть один ключ из `org.freedesktop.appearance`; `true` — что-то изменилось.
    pub fn set(&mut self, key: &str, value: &Value<'_>) -> bool {
        let old = *self;
        match key {
            SCHEME_KEY => self.scheme = parse_scheme(value),
            ACCENT_KEY => self.accent = parse_accent(value),
            _ => {}
        }
        *self != old
    }
}

/// Состояние портала для окна.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PortalState {
    /// Ещё не ответил: это не «недоступен», просто подождать.
    #[default]
    Pending,
    /// Нет шины, нет портала или он не ответил вовремя; строка — причина.
    Unavailable(String),
    Ready(Appearance),
}

/// Снять обёртки variant: `Read` заворачивает значение дважды, `ReadOne` и
/// `SettingChanged` — один раз (эту обёртку снимает уже сам разбор `v`).
pub fn unwrap_variant<'v, 'a>(mut value: &'v Value<'a>) -> &'v Value<'a> {
    while let Value::Value(inner) = value {
        value = inner;
    }
    value
}

/// color-scheme: `u` 1 — тёмная, 2 — светлая, остальное — предпочтения нет.
/// Другой тип — `None`, как будто ключа нет.
pub fn parse_scheme(value: &Value<'_>) -> Option<Scheme> {
    match unwrap_variant(value) {
        Value::U32(1) => Some(Scheme::Dark),
        Value::U32(2) => Some(Scheme::Light),
        Value::U32(_) => Some(Scheme::NoPreference),
        _ => None,
    }
}

/// accent-color: `(ddd)`, каждый канал 0..=1. Вне диапазона (портал шлёт
/// (-1, -1, -1), когда цвет не задан) и другой тип — `None`.
pub fn parse_accent(value: &Value<'_>) -> Option<[u8; 3]> {
    let Value::Structure(s) = unwrap_variant(value) else {
        return None;
    };
    let [Value::F64(r), Value::F64(g), Value::F64(b)] = s.fields() else {
        return None;
    };
    let channel = |c: f64| (0.0..=1.0).contains(&c).then(|| (c * 255.0).round() as u8);
    Some([channel(*r)?, channel(*g)?, channel(*b)?])
}

/// Следить за порталом в отдельном потоке «portal». `on_change` получает
/// первое состояние, затем каждое изменение. `address` — шина для тестов,
/// обычно `None` (сессионная). Поток живёт, пока жива шина.
pub fn spawn(
    address: Option<String>,
    mut on_change: impl FnMut(PortalState) + Send + 'static,
) -> std::io::Result<()> {
    thread::Builder::new()
        .name("portal".into())
        .spawn(move || watch(address.as_deref(), &mut on_change))
        .map(drop)
}

fn watch(address: Option<&str>, on_change: &mut dyn FnMut(PortalState)) {
    let (proxy, signals) = match subscribe(address) {
        Ok(pair) => pair,
        Err(reason) => return unavailable(on_change, reason),
    };
    // Подписка уже есть: изменение между чтением и циклом не потеряется.
    let mut current = read_all(&proxy);
    match &current {
        Ok(appearance) => {
            if appearance.scheme.is_none() {
                log::warn!("портал настроек не сообщает {SCHEME_KEY} — тёмная тема");
            }
            on_change(PortalState::Ready(*appearance));
        }
        Err(err) => unavailable(on_change, describe(err)),
    }
    for msg in signals {
        let body = msg.body();
        let Ok((namespace, key, value)) = body.deserialize::<(&str, &str, Value<'_>)>() else {
            continue;
        };
        if namespace != NS {
            continue;
        }
        match &mut current {
            Ok(appearance) => {
                if appearance.set(key, &value) {
                    on_change(PortalState::Ready(*appearance));
                }
            }
            // Портал запустился уже после нас: прочитать всё заново.
            Err(_) => {
                current = read_all(&proxy);
                if let Ok(appearance) = &current {
                    on_change(PortalState::Ready(*appearance));
                }
            }
        }
    }
}

/// Сказать «недоступен» — в журнал один раз за поток: сюда приходят только
/// при старте.
fn unavailable(on_change: &mut dyn FnMut(PortalState), reason: String) {
    log::warn!("портал настроек недоступен ({reason}) — тёмная тема");
    on_change(PortalState::Unavailable(reason));
}

fn connect(address: Option<&str>) -> zbus::Result<Connection> {
    let builder = match address {
        Some(address) => zbus::blocking::connection::Builder::address(address)?,
        None => zbus::blocking::connection::Builder::session()?,
    };
    builder.method_timeout(TIMEOUT).build()
}

/// Подписка на SettingChanged своего пространства — до первого чтения.
fn subscribe(address: Option<&str>) -> Result<(Proxy<'static>, SignalIterator<'static>), String> {
    let conn = connect(address).map_err(|err| format!("нет сессионной шины: {err}"))?;
    let proxy: Proxy<'static> = ProxyBuilder::new(&conn)
        .destination(DEST)
        .and_then(|b| b.path(PATH))
        .and_then(|b| b.interface(IFACE))
        .and_then(|b| b.cache_properties(CacheProperties::No).build())
        .map_err(|err| describe(&err))?;
    let signals = proxy
        .receive_signal_with_args("SettingChanged", &[(0, NS)])
        .map_err(|err| describe(&err))?;
    Ok((proxy, signals))
}

/// Причина для журнала и подсказки в окне.
fn describe(err: &zbus::Error) -> String {
    match err {
        zbus::Error::MethodError(name, ..)
            if matches!(name.as_str(), SERVICE_UNKNOWN | NAME_HAS_NO_OWNER) =>
        {
            format!("нет службы {DEST}")
        }
        zbus::Error::InputOutput(io) if io.kind() == std::io::ErrorKind::TimedOut => {
            format!("нет ответа за {} с", TIMEOUT.as_secs())
        }
        other => other.to_string(),
    }
}

/// Оба ключа оформления. Ошибка — портал недоступен.
fn read_all(proxy: &Proxy<'_>) -> zbus::Result<Appearance> {
    let mut legacy = false;
    let mut appearance = Appearance::default();
    for key in [SCHEME_KEY, ACCENT_KEY] {
        if let Some(value) = read_key(proxy, &mut legacy, key)? {
            appearance.set(key, &value);
        }
    }
    Ok(appearance)
}

/// Один ключ: `ReadOne`, а у старого портала (версия 1) — `Read`.
/// `Ok(None)` — портал такого ключа не знает.
fn read_key(proxy: &Proxy<'_>, legacy: &mut bool, key: &str) -> zbus::Result<Option<OwnedValue>> {
    if !*legacy {
        match proxy.call_method("ReadOne", &(NS, key)) {
            Ok(reply) => return reply.body().deserialize().map(Some),
            Err(zbus::Error::MethodError(name, ..)) if name.as_str() == UNKNOWN_METHOD => {
                *legacy = true;
            }
            Err(err) => return not_found(err),
        }
    }
    match proxy.call_method("Read", &(NS, key)) {
        Ok(reply) => reply.body().deserialize().map(Some),
        Err(err) => not_found(err),
    }
}

fn not_found(err: zbus::Error) -> zbus::Result<Option<OwnedValue>> {
    match &err {
        zbus::Error::MethodError(name, ..) if name.as_str() == NOT_FOUND => Ok(None),
        _ => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{BufRead, BufReader};
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver};

    use zbus::zvariant::Structure;

    use super::*;

    fn accent_value(r: f64, g: f64, b: f64) -> Value<'static> {
        Value::Structure(Structure::from((r, g, b)))
    }

    #[test]
    fn unwraps_variants() {
        let plain = Value::U32(1);
        assert_eq!(unwrap_variant(&plain), &Value::U32(1));
        let once = Value::Value(Box::new(Value::U32(1)));
        assert_eq!(unwrap_variant(&once), &Value::U32(1));
        let twice = Value::Value(Box::new(Value::Value(Box::new(Value::U32(2)))));
        assert_eq!(unwrap_variant(&twice), &Value::U32(2));
        assert_eq!(parse_scheme(&twice), Some(Scheme::Light));
        let accent = Value::Value(Box::new(Value::Value(Box::new(accent_value(
            0.0, 0.0, 1.0,
        )))));
        assert_eq!(parse_accent(&accent), Some([0, 0, 255]));
    }

    #[test]
    fn scheme_values() {
        assert_eq!(parse_scheme(&Value::U32(0)), Some(Scheme::NoPreference));
        assert_eq!(parse_scheme(&Value::U32(1)), Some(Scheme::Dark));
        assert_eq!(parse_scheme(&Value::U32(2)), Some(Scheme::Light));
        assert_eq!(parse_scheme(&Value::U32(7)), Some(Scheme::NoPreference));
        assert_eq!(parse_scheme(&Value::from("dark")), None);
        assert_eq!(parse_scheme(&Value::I32(1)), None);
    }

    #[test]
    fn accent_values() {
        assert_eq!(parse_accent(&accent_value(0.0, 0.0, 0.0)), Some([0, 0, 0]));
        assert_eq!(
            parse_accent(&accent_value(1.0, 1.0, 1.0)),
            Some([255, 255, 255])
        );
        // Округление, а не отбрасывание дробной части: 0.5 · 255 = 127.5.
        assert_eq!(
            parse_accent(&accent_value(1.0, 0.5, 0.0)),
            Some([255, 128, 0])
        );
        assert_eq!(
            parse_accent(&accent_value(0.2392, 0.6823, 0.9137)),
            Some([61, 174, 233])
        );
        // «Не задан».
        assert_eq!(parse_accent(&accent_value(-1.0, -1.0, -1.0)), None);
        assert_eq!(parse_accent(&accent_value(0.5, -0.01, 0.5)), None);
        assert_eq!(parse_accent(&accent_value(1.01, 0.5, 0.5)), None);
        assert_eq!(parse_accent(&accent_value(f64::NAN, 0.5, 0.5)), None);
        // Не тот тип.
        assert_eq!(parse_accent(&Value::U32(1)), None);
        assert_eq!(
            parse_accent(&Value::Structure(Structure::from((1u32, 0u32, 0u32)))),
            None
        );
        assert_eq!(
            parse_accent(&Value::Structure(Structure::from((1.0, 0.0)))),
            None
        );
    }

    #[test]
    fn appearance_set_reports_changes() {
        let mut a = Appearance::default();
        assert!(a.set(SCHEME_KEY, &Value::U32(1)));
        assert!(!a.set(SCHEME_KEY, &Value::U32(1)), "то же значение");
        assert!(!a.set("contrast", &Value::U32(1)), "чужой ключ");
        assert!(a.set(ACCENT_KEY, &accent_value(1.0, 0.5, 0.0)));
        assert!(a.set(ACCENT_KEY, &accent_value(-1.0, -1.0, -1.0)));
        assert_eq!(
            a,
            Appearance {
                scheme: Some(Scheme::Dark),
                accent: None
            }
        );
    }

    // ─── свой dbus-daemon с поддельным порталом ───

    /// Без servicedirs: иначе на машине с настоящим xdg-desktop-portal шина
    /// запустила бы его сама. Сокет — в каталоге теста (`{dir}`), он удаляется
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

    /// Отдельная шина на время теста; при выходе (и при панике) — kill и wait.
    struct Bus {
        child: Child,
        dir: PathBuf,
        address: String,
    }

    impl Bus {
        /// `None` — dbus-daemon нет.
        fn start() -> Option<Bus> {
            static N: AtomicUsize = AtomicUsize::new(0);
            // Путь сокета — не длиннее 108 байт.
            let base = Some(std::env::temp_dir())
                .filter(|t| t.as_os_str().len() <= 60)
                .unwrap_or_else(|| PathBuf::from("/tmp"));
            let dir = base.join(format!(
                "shiftfix-portal-{}-{}",
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

        /// Следить за порталом этой шины; состояния — в канал.
        fn watch(&self) -> Receiver<PortalState> {
            let (tx, rx) = mpsc::channel();
            spawn(Some(self.address.clone()), move |state| {
                let _ = tx.send(state);
            })
            .unwrap();
            rx
        }

        fn serve<I: zbus::object_server::Interface>(&self, iface: I) -> Connection {
            zbus::blocking::connection::Builder::address(self.address.as_str())
                .unwrap()
                .name(DEST)
                .unwrap()
                .serve_at(PATH, iface)
                .unwrap()
                .build()
                .unwrap()
        }
    }

    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[derive(Debug, zbus::DBusError)]
    #[zbus(prefix = "org.freedesktop.portal.Error")]
    enum PortalError {
        #[zbus(error)]
        ZBus(zbus::Error),
        NotFound(String),
    }

    /// Значения поддельного портала.
    #[derive(Default)]
    struct Values {
        scheme: Option<u32>,
        accent: Option<(f64, f64, f64)>,
    }

    impl Values {
        fn get(&self, namespace: &str, key: &str) -> Result<Value<'static>, PortalError> {
            let value = match (namespace, key) {
                (NS, SCHEME_KEY) => self.scheme.map(Value::U32),
                (NS, ACCENT_KEY) => self.accent.map(|(r, g, b)| accent_value(r, g, b)),
                _ => None,
            };
            value.ok_or_else(|| PortalError::NotFound("Requested setting not found".into()))
        }

        /// Как у настоящего портала: `Read` заворачивает в variant ещё раз.
        fn get_wrapped(&self, namespace: &str, key: &str) -> Result<OwnedValue, PortalError> {
            let value = Value::Value(Box::new(self.get(namespace, key)?));
            OwnedValue::try_from(value).map_err(|err| PortalError::ZBus(err.into()))
        }
    }

    /// Портал версии 2: `ReadOne` и `Read`.
    struct FakeSettings(Values);

    #[zbus::interface(name = "org.freedesktop.portal.Settings")]
    impl FakeSettings {
        fn read_one(&self, namespace: &str, key: &str) -> Result<OwnedValue, PortalError> {
            let value = self.0.get(namespace, key)?;
            OwnedValue::try_from(value).map_err(|err| PortalError::ZBus(err.into()))
        }

        fn read(&self, namespace: &str, key: &str) -> Result<OwnedValue, PortalError> {
            self.0.get_wrapped(namespace, key)
        }

        #[zbus(property)]
        fn version(&self) -> u32 {
            2
        }
    }

    /// Старый портал (версия 1): только `Read`.
    struct OldSettings(Values);

    #[zbus::interface(name = "org.freedesktop.portal.Settings")]
    impl OldSettings {
        fn read(&self, namespace: &str, key: &str) -> Result<OwnedValue, PortalError> {
            self.0.get_wrapped(namespace, key)
        }

        #[zbus(property)]
        fn version(&self) -> u32 {
            1
        }
    }

    /// Поменять значение поддельного портала и разослать SettingChanged.
    fn change(server: &Connection, key: &str, edit: impl FnOnce(&mut Values)) {
        let iface = server
            .object_server()
            .interface::<_, FakeSettings>(PATH)
            .unwrap();
        let value = {
            let mut fake = iface.get_mut();
            edit(&mut fake.0);
            fake.0.get(NS, key).unwrap()
        };
        emit(server, NS, key, value);
    }

    fn emit(server: &Connection, namespace: &str, key: &str, value: Value<'_>) {
        server
            .emit_signal(
                None::<&str>,
                PATH,
                IFACE,
                "SettingChanged",
                &(namespace, key, value),
            )
            .unwrap();
    }

    const WAIT: Duration = Duration::from_secs(10);

    fn ready(scheme: Option<Scheme>, accent: Option<[u8; 3]>) -> PortalState {
        PortalState::Ready(Appearance { scheme, accent })
    }

    #[test]
    fn follows_fake_portal() {
        let Some(bus) = Bus::start() else {
            eprintln!("нет dbus-daemon — пропуск");
            return;
        };

        // Шина есть, портала нет — недоступен, и сразу (активировать нечего).
        let early = bus.watch();
        match early.recv_timeout(WAIT).unwrap() {
            PortalState::Unavailable(reason) => assert!(reason.contains(DEST), "{reason}"),
            other => panic!("ждали Unavailable, пришло {other:?}"),
        }

        let server = bus.serve(FakeSettings(Values {
            scheme: Some(1),
            accent: Some((1.0, 0.5, 0.0)),
        }));
        let rx = bus.watch();
        assert_eq!(
            rx.recv_timeout(WAIT).unwrap(),
            ready(Some(Scheme::Dark), Some([255, 128, 0]))
        );

        change(&server, SCHEME_KEY, |v| v.scheme = Some(2));
        assert_eq!(
            rx.recv_timeout(WAIT).unwrap(),
            ready(Some(Scheme::Light), Some([255, 128, 0]))
        );
        // Портал появился после первого наблюдателя: по сигналу он перечитал всё.
        assert_eq!(
            early.recv_timeout(WAIT).unwrap(),
            ready(Some(Scheme::Light), Some([255, 128, 0]))
        );

        change(&server, ACCENT_KEY, |v| v.accent = Some((-1.0, -1.0, -1.0)));
        assert_eq!(
            rx.recv_timeout(WAIT).unwrap(),
            ready(Some(Scheme::Light), None)
        );

        // Чужое пространство и незнакомый ключ — изменений нет.
        emit(
            &server,
            "org.kde.kdeglobals.General",
            SCHEME_KEY,
            Value::U32(1),
        );
        emit(&server, NS, "contrast", Value::U32(1));
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());

        // То же значение ещё раз — тоже без события; 0 — «нет предпочтения».
        change(&server, SCHEME_KEY, |v| v.scheme = Some(2));
        change(&server, SCHEME_KEY, |v| v.scheme = Some(0));
        assert_eq!(
            rx.recv_timeout(WAIT).unwrap(),
            ready(Some(Scheme::NoPreference), None)
        );
        drop(server);
    }

    #[test]
    fn reads_old_portal() {
        let Some(bus) = Bus::start() else {
            eprintln!("нет dbus-daemon — пропуск");
            return;
        };
        // Только Read (ответ завёрнут дважды) и без accent-color (NotFound).
        let _server = bus.serve(OldSettings(Values {
            scheme: Some(2),
            accent: None,
        }));
        let rx = bus.watch();
        assert_eq!(
            rx.recv_timeout(WAIT).unwrap(),
            ready(Some(Scheme::Light), None)
        );
    }

    #[test]
    fn missing_keys_are_not_errors() {
        let Some(bus) = Bus::start() else {
            eprintln!("нет dbus-daemon — пропуск");
            return;
        };
        // Портал есть, но бэкенд ничего не сообщает (как Hyprland без gtk-портала).
        let _server = bus.serve(FakeSettings(Values::default()));
        let rx = bus.watch();
        assert_eq!(rx.recv_timeout(WAIT).unwrap(), ready(None, None));
    }

    #[test]
    fn no_bus_is_unavailable() {
        let rx = {
            let (tx, rx) = mpsc::channel();
            spawn(
                Some("unix:path=/nonexistent/shiftfix-bus".into()),
                move |s| {
                    let _ = tx.send(s);
                },
            )
            .unwrap();
            rx
        };
        assert!(matches!(
            rx.recv_timeout(WAIT).unwrap(),
            PortalState::Unavailable(_)
        ));
    }
}

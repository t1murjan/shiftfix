use std::sync::mpsc::{self, Receiver};

use zbus::zvariant::Structure;

use super::*;
use crate::testutil::{Bus, skip};

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

// ─── поддельный портал на своём dbus-daemon ───

impl Bus {
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

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.portal.Error")]
enum PortalError {
    #[zbus(error)]
    ZBus(zbus::Error),
    NotFound(String),
    Failed(String),
}

/// Значения поддельного портала.
#[derive(Default)]
struct Values {
    scheme: Option<u32>,
    accent: Option<(f64, f64, f64)>,
    /// Чтение accent-color кончается ошибкой (не NotFound).
    accent_fails: bool,
}

impl Values {
    fn get(&self, namespace: &str, key: &str) -> Result<Value<'static>, PortalError> {
        if self.accent_fails && (namespace, key) == (NS, ACCENT_KEY) {
            return Err(PortalError::Failed("accent-color сломан".into()));
        }
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
        return skip("нет dbus-daemon");
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
        ..Values::default()
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
        return skip("нет dbus-daemon");
    };
    // Только Read (ответ завёрнут дважды) и без accent-color (NotFound).
    let _server = bus.serve(OldSettings(Values {
        scheme: Some(2),
        ..Values::default()
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
        return skip("нет dbus-daemon");
    };
    // Портал есть, но бэкенд ничего не сообщает (как Hyprland без gtk-портала).
    let _server = bus.serve(FakeSettings(Values::default()));
    let rx = bus.watch();
    assert_eq!(rx.recv_timeout(WAIT).unwrap(), ready(None, None));
}

#[test]
fn broken_accent_keeps_scheme() {
    let Some(bus) = Bus::start() else {
        return skip("нет dbus-daemon");
    };
    // Ошибка accent-color — только без акцента, тема системы остаётся.
    let _server = bus.serve(FakeSettings(Values {
        scheme: Some(2),
        accent_fails: true,
        ..Values::default()
    }));
    let rx = bus.watch();
    assert_eq!(
        rx.recv_timeout(WAIT).unwrap(),
        ready(Some(Scheme::Light), None)
    );
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

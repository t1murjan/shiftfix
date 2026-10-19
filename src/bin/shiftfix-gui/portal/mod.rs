//! Тема системы из XDG Desktop Portal: color-scheme и accent-color из
//! `org.freedesktop.portal.Settings` — для режима темы «как в системе».
//!
//! Портал читается в своём потоке: холодный старт xdg-desktop-portal (а без
//! бэкенда — зависание) длится секунды, и окно не должно его ждать.
//!
//! `tests.rs` — разбор значений и поддельный портал на своём dbus-daemon.

#[cfg(test)]
mod tests;

use std::thread;
use std::time::Duration;

use zbus::blocking::proxy::{Builder as ProxyBuilder, SignalIterator};
use zbus::blocking::{Connection, Proxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::{OwnedValue, Value};

const DEST: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.portal.Settings";
/// Пространство ключей оформления.
const NS: &str = "org.freedesktop.appearance";
const SCHEME_KEY: &str = "color-scheme";
const ACCENT_KEY: &str = "accent-color";

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
    fn set(&mut self, key: &str, value: &Value<'_>) -> bool {
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
fn unwrap_variant<'v, 'a>(mut value: &'v Value<'a>) -> &'v Value<'a> {
    while let Value::Value(inner) = value {
        value = inner;
    }
    value
}

/// color-scheme: `u` 1 — тёмная, 2 — светлая, остальное — предпочтения нет.
/// Другой тип — `None`, как будто ключа нет.
fn parse_scheme(value: &Value<'_>) -> Option<Scheme> {
    match unwrap_variant(value) {
        Value::U32(1) => Some(Scheme::Dark),
        Value::U32(2) => Some(Scheme::Light),
        Value::U32(_) => Some(Scheme::NoPreference),
        _ => None,
    }
}

/// accent-color: `(ddd)`, каждый канал 0..=1. Вне диапазона (портал шлёт
/// (-1, -1, -1), когда цвет не задан) и другой тип — `None`.
fn parse_accent(value: &Value<'_>) -> Option<[u8; 3]> {
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

/// Оба ключа оформления. Ошибка чтения color-scheme — портал недоступен;
/// accent-color — только без акцента системы: тема уже прочитана и важнее.
fn read_all(proxy: &Proxy<'_>) -> zbus::Result<Appearance> {
    let mut legacy = false;
    let mut appearance = Appearance::default();
    if let Some(value) = read_key(proxy, &mut legacy, SCHEME_KEY)? {
        appearance.set(SCHEME_KEY, &value);
    }
    match read_key(proxy, &mut legacy, ACCENT_KEY) {
        Ok(Some(value)) => {
            appearance.set(ACCENT_KEY, &value);
        }
        Ok(None) => {}
        Err(err) => log::warn!(
            "портал настроек: {ACCENT_KEY} не прочитан ({}) — без акцента системы",
            describe(&err)
        ),
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

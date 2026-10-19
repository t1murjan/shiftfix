//! Клиент DBus с ленивым подключением и переподключением после ошибки:
//! шина могла перезапуститься, а служба живёт дольше сеанса.

use std::fmt::Display;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use zbus::blocking::Connection;
use zbus::zvariant::{DynamicType, OwnedValue};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bus {
    Session,
    System,
}

pub(crate) struct Client {
    bus: Bus,
    /// Адрес шины для тестов; обычно — стандартная шина.
    address: Option<String>,
    conn: Option<Connection>,
}

impl Client {
    pub const fn new(bus: Bus) -> Client {
        Client {
            bus,
            address: None,
            conn: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_address(address: &str) -> Client {
        Client {
            bus: Bus::Session,
            address: Some(address.to_owned()),
            conn: None,
        }
    }

    fn conn(&mut self) -> Result<&Connection> {
        if self.conn.is_none() {
            let builder = match (&self.address, self.bus) {
                (Some(address), _) => {
                    zbus::blocking::connection::Builder::address(address.as_str())
                }
                (None, Bus::Session) => zbus::blocking::connection::Builder::session(),
                (None, Bus::System) => zbus::blocking::connection::Builder::system(),
            };
            let conn = builder
                .and_then(|b| b.method_timeout(Duration::from_secs(2)).build())
                .with_context(|| match self.bus {
                    Bus::Session => "DBus: нет сессионной шины",
                    Bus::System => "DBus: нет системной шины",
                })?;
            self.conn = Some(conn);
        }
        Ok(self.conn.as_ref().expect("connection"))
    }

    pub(crate) fn call<B>(
        &mut self,
        dest: &str,
        path: &str,
        iface: &str,
        method: &str,
        body: &B,
    ) -> Result<zbus::Message>
    where
        B: serde::Serialize + DynamicType,
    {
        let reply = self
            .conn()?
            .call_method(Some(dest), path, Some(iface), method, body);
        reply.map_err(|err| {
            if !matches!(err, zbus::Error::MethodError(..) | zbus::Error::FDO(_)) {
                self.conn = None; // шина могла пропасть — в следующий раз переподключимся
            }
            anyhow!("{method}: {err}")
        })
    }

    /// Свойство объекта (`org.freedesktop.DBus.Properties.Get`).
    pub fn property<T>(&mut self, dest: &str, path: &str, iface: &str, name: &str) -> Result<T>
    where
        T: TryFrom<OwnedValue>,
        T::Error: Display,
    {
        let msg = self.call(
            dest,
            path,
            "org.freedesktop.DBus.Properties",
            "Get",
            &(iface, name),
        )?;
        let value: OwnedValue = msg
            .body()
            .deserialize()
            .with_context(|| format!("{name}: неожиданный ответ"))?;
        T::try_from(value).map_err(|err| anyhow!("{name}: {err}"))
    }

    /// Есть ли у имени владелец (запущен ли сервис).
    pub(crate) fn has_owner(&mut self, name: &str) -> bool {
        self.call(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            &(name,),
        )
        .ok()
        .and_then(|msg| msg.body().deserialize::<bool>().ok())
        .unwrap_or(false)
    }
}

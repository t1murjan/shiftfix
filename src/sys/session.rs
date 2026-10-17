//! Можно ли сейчас исправлять. Группа `input` даёт читать клавиатуру и тогда,
//! когда за компьютером другой пользователь или открыта текстовая консоль:
//! в такой момент Backspace и буквы ушли бы в чужой сеанс.

use crate::sys::dbus::{Bus, Client};
use crate::sys::env;

/// Состояние сеанса пользователя по данным logind.
pub struct Logind {
    client: Client,
}

impl Default for Logind {
    fn default() -> Self {
        Logind {
            client: Client::new(Bus::System),
        }
    }
}

impl Logind {
    /// Для тестов: logind на отдельной шине.
    pub fn with_address(address: &str) -> Logind {
        Logind {
            client: Client::with_address(address),
        }
    }

    /// `Some(false)` — сеанс неактивен; `None` — logind недоступен.
    /// `session/auto` — сеанс вызывающего, а для службы — «главный» сеанс пользователя.
    pub fn active(&mut self) -> Option<bool> {
        self.client
            .property::<bool>(
                "org.freedesktop.login1",
                "/org/freedesktop/login1/session/auto",
                "org.freedesktop.login1.Session",
                "Active",
            )
            .ok()
    }
}

/// Почему исправлять нельзя; `None` — можно.
pub fn blocked(logind: &mut Logind) -> Option<String> {
    if !env::graphical_session() {
        return Some("нет графического сеанса".into());
    }
    if logind.active() == Some(false) {
        return Some("сеанс неактивен: работает другой пользователь или открыта консоль".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{TestBus, skip};

    struct FakeSession {
        active: bool,
    }

    #[zbus::interface(name = "org.freedesktop.login1.Session")]
    impl FakeSession {
        #[zbus(property)]
        fn active(&self) -> bool {
            self.active
        }
    }

    #[test]
    fn reads_active_flag_from_logind() {
        let Some(bus) = TestBus::start() else {
            return skip("нет dbus-daemon");
        };

        // logind ещё нет — значит, не мешаем.
        let mut logind = Logind::with_address(&bus.address);
        assert_eq!(logind.active(), None);

        let server = zbus::blocking::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .name("org.freedesktop.login1")
            .unwrap()
            .serve_at(
                "/org/freedesktop/login1/session/auto",
                FakeSession { active: false },
            )
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(logind.active(), Some(false));
        let iface = server
            .object_server()
            .interface::<_, FakeSession>("/org/freedesktop/login1/session/auto")
            .unwrap();
        iface.get_mut().active = true;
        assert_eq!(logind.active(), Some(true));
    }
}

//! Связь с композитором Wayland текущего сеанса.

use std::os::unix::net::UnixStream;

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle};

use crate::sys::env;

/// Подключиться к композитору сеанса; `None` — его нет или он не отвечает.
pub fn connect() -> Option<Connection> {
    let display = env::wayland_display()?;
    let path = if display.starts_with('/') {
        display.into()
    } else {
        env::runtime_dir().join(display)
    };
    Connection::from_socket(UnixStream::connect(path).ok()?).ok()
}

/// Только список глобальных объектов композитора.
struct Globals;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Globals {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// Есть ли у композитора протокол data-control (без него wl-paste не может
/// следить за выделением; так в GNOME). `None` — к композитору не подключиться.
pub fn data_control() -> Option<bool> {
    let conn = connect()?;
    let (globals, _queue) = registry_queue_init::<Globals>(&conn).ok()?;
    Some(globals.contents().with_list(|list| {
        list.iter().any(|g| {
            matches!(
                g.interface.as_str(),
                "zwlr_data_control_manager_v1" | "ext_data_control_manager_v1"
            )
        })
    }))
}

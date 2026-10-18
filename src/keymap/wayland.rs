//! Keymap от композитора Wayland: он присылает её любому клиенту (событие
//! `wl_keyboard.keymap`), в ней все раскладки сеанса в том же порядке, что и
//! у переключателя, и присылает новую при каждой смене списка раскладок.

use std::fs::File;
use std::os::unix::fs::FileExt;

use log::{debug, info};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_keyboard, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};

use super::{Keymap, xkb};
use crate::sys::wayland::connect;

#[derive(Default)]
struct State {
    keyboard: Option<wl_keyboard::WlKeyboard>,
    /// Новая keymap, ещё не разобранная.
    keymap: Option<String>,
    /// Последняя присланная. Ту же самую композитор присылает заново при смене
    /// активной клавиатуры (wlroots, Hyprland) — и когда слово перенабирает
    /// виртуальная.
    last: String,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
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

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
            && caps.contains(wl_seat::Capability::Keyboard)
            && state.keyboard.is_none()
        {
            state.keyboard = Some(seat.get_keyboard(qh, ()));
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Keymap {
            format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
            fd,
            size,
        } = event
        {
            // fd бывает общим для всех клиентов: читаем со смещения 0, не двигая позицию.
            let mut buf = vec![0u8; size as usize];
            if File::from(fd).read_exact_at(&mut buf, 0).is_ok() {
                let text = String::from_utf8_lossy(&buf).into_owned();
                if text != state.last {
                    state.last.clone_from(&text);
                    state.keymap = Some(text);
                }
            }
        }
    }
}

/// Один раз получить keymap композитора.
pub fn fetch() -> Option<Keymap> {
    let conn = connect()?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn).ok()?;
    let _seat: wl_seat::WlSeat = globals.bind(&queue.handle(), 1..=8, ()).ok()?;
    let mut state = State::default();
    for _ in 0..4 {
        queue.roundtrip(&mut state).ok()?;
        if let Some(text) = state.keymap.take() {
            return xkb::from_text(&text);
        }
    }
    None
}

/// Присылать keymap, пока жива связь с композитором.
pub(super) fn watch(conn: &Connection, on_change: &impl Fn(Keymap)) {
    let Ok((globals, mut queue)) = registry_queue_init::<State>(conn) else {
        return;
    };
    let Ok(_seat) = globals.bind::<wl_seat::WlSeat, _, _>(&queue.handle(), 1..=8, ()) else {
        return;
    };
    let mut state = State::default();
    while queue.blocking_dispatch(&mut state).is_ok() {
        if let Some(text) = state.keymap.take() {
            match xkb::from_text(&text) {
                Some(keymap) => {
                    let names: Vec<&str> = keymap.layouts.iter().map(|l| l.name.as_str()).collect();
                    info!("раскладки из keymap: {}", names.join(", "));
                    on_change(keymap);
                }
                None => debug!("keymap не разобрана (нет libxkbcommon?)"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::net::UnixStream;

    use wayland_client::Proxy;

    use super::*;
    use crate::testutil::TempDir;

    /// Ту же keymap заново не разбирать и не писать в журнал.
    #[test]
    fn same_keymap_is_taken_once() {
        let (socket, _server) = UnixStream::pair().unwrap();
        let conn = Connection::from_socket(socket).unwrap();
        let queue = conn.new_event_queue::<State>();
        let keyboard = wl_keyboard::WlKeyboard::inert(conn.backend().downgrade());
        let dir = TempDir::new("keymap");
        let mut state = State::default();
        let mut send = |text: &str| {
            let path = dir.join("keymap");
            fs::write(&path, text).unwrap();
            let event = wl_keyboard::Event::Keymap {
                format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
                fd: File::open(&path).unwrap().into(),
                size: text.len() as u32,
            };
            State::event(&mut state, &keyboard, event, &(), &conn, &queue.handle());
            state.keymap.take()
        };
        assert_eq!(send("первая").as_deref(), Some("первая"));
        assert_eq!(send("первая"), None);
        assert_eq!(send("вторая").as_deref(), Some("вторая"));
    }
}

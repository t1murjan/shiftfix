//! X11: переключение группы XKB — работает в любом окружении X11.

use anyhow::{Context, Result, bail};
use x11rb::protocol::xkb::{self, ConnectionExt as _};

use crate::engine::{Backend, Keys};
use crate::keys::norm_layout;
use crate::x11::{XConn, connect, rules_names};

/// Группа XKB текущего X-сервера; подключение — при первом обращении.
#[derive(Default)]
pub struct XkbLayout {
    x: Option<XConn>,
}

impl XkbLayout {
    fn with<R>(&mut self, f: impl FnOnce(&XConn) -> Result<R>) -> Result<R> {
        if self.x.is_none() {
            let x = connect()?;
            let reply = x.conn.xkb_use_extension(1, 0)?.reply()?;
            if !reply.supported {
                bail!("у X-сервера нет расширения XKB");
            }
            self.x = Some(x);
        }
        let result = f(self.x.as_ref().expect("connection"));
        if result.is_err() {
            self.x = None; // X-сервер мог перезапуститься
        }
        result
    }
}

impl Backend for XkbLayout {
    fn name(&self) -> &'static str {
        "X11"
    }

    fn layouts(&mut self) -> Result<Vec<String>> {
        self.with(|x| {
            let names = rules_names(x)?;
            let layouts = names.get(2).map(String::as_str).unwrap_or_default();
            if layouts.is_empty() {
                bail!("раскладки X11 неизвестны");
            }
            Ok(layouts.split(',').map(norm_layout).collect())
        })
    }

    fn current(&mut self) -> Result<Option<usize>> {
        self.with(|x| {
            let state = x
                .conn
                .xkb_get_state(xkb::ID::USE_CORE_KBD.into())?
                .reply()?;
            Ok(Some(usize::from(u8::from(state.locked_group))))
        })
    }

    fn set(&mut self, index: usize, _keys: &mut dyn Keys) -> Result<()> {
        let group = u8::try_from(index)
            .ok()
            .filter(|g| *g < 4)
            .context("в X11 не больше четырёх раскладок")?;
        self.with(|x| {
            x.conn
                .xkb_latch_lock_state(
                    xkb::ID::USE_CORE_KBD.into(),
                    0u8.into(),
                    0u8.into(),
                    true,
                    xkb::Group::from(group),
                    0u8.into(),
                    false,
                    0,
                )?
                .check()?;
            Ok(())
        })
    }

    fn toggle(&mut self, keys: &mut dyn Keys) -> Result<()> {
        let count = self.layouts()?.len().max(1);
        let current = self.current()?.unwrap_or(0);
        self.set((current + 1) % count, keys)
    }
}

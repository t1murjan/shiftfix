//! KDE Plasma: KWin (Plasma 6) и kded (Plasma 5) по DBus.

use anyhow::{Context, Result, bail};

use crate::engine::{Backend, Keys};
use crate::keymap::norm_layout;
use crate::sys::dbus::{Bus, Client};

/// KWin (Plasma 6) и kded (Plasma 5): `org.kde.keyboard /Layouts`.
pub struct Kde {
    client: Client,
}

impl Default for Kde {
    fn default() -> Self {
        Kde {
            client: Client::new(Bus::Session),
        }
    }
}

impl Kde {
    fn reply(
        &mut self,
        method: &str,
        body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
    ) -> Result<zbus::Message> {
        self.client
            .call(
                "org.kde.keyboard",
                "/Layouts",
                "org.kde.KeyboardLayouts",
                method,
                body,
            )
            .map_err(|err| err.context("KDE"))
    }

    fn call<R>(
        &mut self,
        method: &str,
        body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
    ) -> Result<R>
    where
        R: for<'de> serde::Deserialize<'de> + zbus::zvariant::Type,
    {
        let msg = self.reply(method, body)?;
        msg.body()
            .deserialize()
            .with_context(|| format!("KDE {method}: неожиданный ответ"))
    }
}

impl Backend for Kde {
    fn name(&self) -> &'static str {
        "KDE Plasma"
    }

    fn layouts(&mut self) -> Result<Vec<String>> {
        let list: Vec<(String, String, String)> = self.call("getLayoutsList", &())?;
        Ok(list
            .into_iter()
            .map(|(short, _, _)| norm_layout(&short))
            .collect())
    }

    fn current(&mut self) -> Result<Option<usize>> {
        let index: u32 = self.call("getLayout", &())?;
        Ok(Some(index as usize))
    }

    fn set(&mut self, index: usize, _keys: &mut dyn Keys) -> Result<()> {
        let index = u32::try_from(index).context("номер раскладки")?;
        let ok: bool = self.call("setLayout", &(index,))?;
        if !ok {
            bail!("KDE: раскладка {index} не включилась");
        }
        Ok(())
    }

    fn toggle(&mut self, _keys: &mut dyn Keys) -> Result<()> {
        self.reply("switchToNextLayout", &())?; // ответ пустой
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::NoKeys;

    /// Поддельный KWin на отдельной шине DBus: имена методов и типы как в Plasma.
    struct FakeKwin {
        current: u32,
    }

    #[zbus::interface(name = "org.kde.KeyboardLayouts")]
    impl FakeKwin {
        #[zbus(name = "getLayout")]
        fn get_layout(&self) -> u32 {
            self.current
        }
        #[zbus(name = "setLayout")]
        fn set_layout(&mut self, index: u32) -> bool {
            self.current = index;
            index < 2
        }
        #[zbus(name = "switchToNextLayout")]
        fn switch_to_next_layout(&mut self) {
            self.current = (self.current + 1) % 2;
        }
        #[zbus(name = "getLayoutsList")]
        fn get_layouts_list(&self) -> Vec<(String, String, String)> {
            vec![
                ("us".into(), String::new(), "English (US)".into()),
                ("ru".into(), String::new(), "Russian".into()),
            ]
        }
    }

    #[test]
    fn kde_over_dbus() {
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};
        let Ok(mut bus) = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address"])
            .stdout(Stdio::piped())
            .spawn()
        else {
            eprintln!("нет dbus-daemon — пропуск");
            return;
        };
        let mut address = String::new();
        BufReader::new(bus.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let address = address.trim().to_owned();
        let _server = zbus::blocking::connection::Builder::address(address.as_str())
            .unwrap()
            .name("org.kde.keyboard")
            .unwrap()
            .serve_at("/Layouts", FakeKwin { current: 0 })
            .unwrap()
            .build()
            .unwrap();

        let mut kde = Kde {
            client: Client::with_address(&address),
        };
        assert_eq!(kde.layouts().unwrap(), ["us", "ru"]);
        assert_eq!(kde.current().unwrap(), Some(0));
        kde.set(1, &mut NoKeys).unwrap();
        assert_eq!(kde.current().unwrap(), Some(1));
        kde.toggle(&mut NoKeys).unwrap(); // void-ответ
        assert_eq!(kde.current().unwrap(), Some(0));
        assert!(kde.set(5, &mut NoKeys).is_err()); // KWin ответил false
        let _ = bus.kill();
    }
}

//! Переключение раскладки: KWin по DBus, Hyprland, Sway или эмуляция хоткея.
//! В режиме «авто» окружение определяется при первом обращении и заново —
//! если оно сменилось или служба стартовала раньше сессии.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use evdev::KeyCode;
use log::info;
use serde_json::Value;

use crate::config::{BackendKind, Config};
use crate::engine::{Backend, Keys};
use crate::env;
use crate::keys::{code_from_long_name, norm_layout};
use crate::proc::run_text;

// ─── KDE Plasma ─────────────────────────────────────────────────────────────

/// KWin (Plasma 6) и kded (Plasma 5): `org.kde.keyboard /Layouts`.
#[derive(Default)]
pub struct Kde {
    conn: Option<zbus::blocking::Connection>,
    /// Адрес шины для тестов; обычно — сессионная шина.
    address: Option<String>,
}

impl Kde {
    fn reply(
        &mut self,
        method: &str,
        body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
    ) -> Result<zbus::Message> {
        if self.conn.is_none() {
            let builder = match &self.address {
                Some(address) => zbus::blocking::connection::Builder::address(address.as_str()),
                None => zbus::blocking::connection::Builder::session(),
            };
            let conn = builder
                .and_then(|b| b.method_timeout(Duration::from_secs(2)).build())
                .context("DBus: нет сессионной шины")?;
            self.conn = Some(conn);
        }
        let conn = self.conn.as_ref().expect("connection");
        conn.call_method(
            Some("org.kde.keyboard"),
            "/Layouts",
            Some("org.kde.KeyboardLayouts"),
            method,
            body,
        )
        .map_err(|err| {
            self.conn = None; // шина могла перезапуститься — переподключимся
            anyhow!("KDE {method}: {err}")
        })
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

// ─── Hyprland ───────────────────────────────────────────────────────────────

pub struct Hyprland {
    hyprctl: String,
}

impl Default for Hyprland {
    fn default() -> Self {
        Hyprland {
            hyprctl: "hyprctl".into(),
        }
    }
}

impl Hyprland {
    fn keyboards(&self) -> Result<Vec<Value>> {
        let out = run_text(&self.hyprctl, &["-j", "devices"])?;
        let value: Value = serde_json::from_str(&out).context("hyprctl: неожиданный ответ")?;
        let keyboards: Vec<Value> = value["keyboards"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|k| !k["name"].as_str().unwrap_or_default().contains("shiftfix"))
            .cloned()
            .collect();
        if keyboards.is_empty() {
            bail!("hyprctl: клавиатуры не найдены");
        }
        Ok(keyboards)
    }

    fn main_keyboard(&self) -> Result<Value> {
        let keyboards = self.keyboards()?;
        let main = keyboards
            .iter()
            .find(|k| k["main"].as_bool() == Some(true))
            .unwrap_or(&keyboards[0]);
        Ok(main.clone())
    }

    fn switch(&self, arg: &str) -> Result<()> {
        // Старые версии не знают `all` — тогда по одной клавиатуре.
        if run_text(&self.hyprctl, &["switchxkblayout", "all", arg])
            .is_ok_and(|out| out.to_lowercase().contains("ok"))
        {
            return Ok(());
        }
        for kb in self.keyboards()? {
            let name = kb["name"].as_str().unwrap_or_default();
            let out = run_text(&self.hyprctl, &["switchxkblayout", name, arg])?;
            if !out.to_lowercase().contains("ok") {
                bail!("hyprctl switchxkblayout {name}: {}", out.trim());
            }
        }
        Ok(())
    }
}

fn split_layouts(list: &str) -> Vec<String> {
    list.split(',').map(norm_layout).collect()
}

impl Backend for Hyprland {
    fn name(&self) -> &'static str {
        "Hyprland"
    }

    fn layouts(&mut self) -> Result<Vec<String>> {
        Ok(split_layouts(
            self.main_keyboard()?["layout"].as_str().unwrap_or_default(),
        ))
    }

    fn current(&mut self) -> Result<Option<usize>> {
        let kb = self.main_keyboard()?;
        if let Some(index) = kb["active_layout_index"].as_u64() {
            return Ok(Some(index as usize));
        }
        let codes = split_layouts(kb["layout"].as_str().unwrap_or_default());
        let active = code_from_long_name(kb["active_keymap"].as_str().unwrap_or_default());
        Ok(active.and_then(|code| codes.iter().position(|c| c == code)))
    }

    fn set(&mut self, index: usize, _keys: &mut dyn Keys) -> Result<()> {
        self.switch(&index.to_string())
    }

    fn toggle(&mut self, _keys: &mut dyn Keys) -> Result<()> {
        self.switch("next")
    }
}

// ─── Sway ───────────────────────────────────────────────────────────────────

pub struct Sway {
    swaymsg: String,
}

impl Default for Sway {
    fn default() -> Self {
        Sway {
            swaymsg: "swaymsg".into(),
        }
    }
}

impl Sway {
    fn keyboard(&self) -> Result<Value> {
        let out = run_text(&self.swaymsg, &["-t", "get_inputs", "-r"])?;
        let inputs: Value = serde_json::from_str(&out).context("swaymsg: неожиданный ответ")?;
        inputs
            .as_array()
            .into_iter()
            .flatten()
            .find(|i| {
                i["type"] == "keyboard"
                    && !i["name"].as_str().unwrap_or_default().contains("shiftfix")
            })
            .cloned()
            .ok_or_else(|| anyhow!("swaymsg: клавиатуры не найдены"))
    }
}

impl Backend for Sway {
    fn name(&self) -> &'static str {
        "Sway"
    }

    fn layouts(&mut self) -> Result<Vec<String>> {
        let kb = self.keyboard()?;
        let names = kb["xkb_layout_names"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        Ok(names
            .iter()
            .map(|n| {
                let long = n.as_str().unwrap_or_default();
                code_from_long_name(long).map_or_else(|| long.to_owned(), str::to_owned)
            })
            .collect())
    }

    fn current(&mut self) -> Result<Option<usize>> {
        Ok(self.keyboard()?["xkb_active_layout_index"]
            .as_u64()
            .map(|i| i as usize))
    }

    fn set(&mut self, index: usize, _keys: &mut dyn Keys) -> Result<()> {
        run_text(
            &self.swaymsg,
            &[
                "input",
                "type:keyboard",
                "xkb_switch_layout",
                &index.to_string(),
            ],
        )
        .map(drop)
    }

    fn toggle(&mut self, _keys: &mut dyn Keys) -> Result<()> {
        run_text(
            &self.swaymsg,
            &["input", "type:keyboard", "xkb_switch_layout", "next"],
        )
        .map(drop)
    }
}

// ─── Хоткей ─────────────────────────────────────────────────────────────────

/// Запасной вариант: нажимает хоткей переключения, текущая раскладка неизвестна.
pub struct Hotkey {
    keys: Vec<KeyCode>,
    layouts: Vec<String>,
}

impl Backend for Hotkey {
    fn name(&self) -> &'static str {
        "хоткей"
    }

    fn layouts(&mut self) -> Result<Vec<String>> {
        Ok(self.layouts.clone())
    }

    fn current(&mut self) -> Result<Option<usize>> {
        Ok(None)
    }

    fn set(&mut self, _index: usize, keys: &mut dyn Keys) -> Result<()> {
        self.toggle(keys)
    }

    fn toggle(&mut self, keys: &mut dyn Keys) -> Result<()> {
        keys.combo(&self.keys)
    }
}

// ─── Авто ───────────────────────────────────────────────────────────────────

/// Определяет окружение лениво и переопределяет, если оно перестало отвечать.
pub struct Auto {
    found: Option<Box<dyn Backend>>,
    fallback: Hotkey,
    last_probe: Option<Instant>,
}

impl Auto {
    fn probe() -> Option<Box<dyn Backend>> {
        let mut candidates: Vec<Box<dyn Backend>> = Vec::new();
        if env::hyprland_signature().is_some() {
            candidates.push(Box::new(Hyprland::default()));
        }
        if env::sway_socket().is_some() {
            candidates.push(Box::new(Sway::default()));
        }
        candidates.push(Box::new(Kde::default()));
        candidates
            .into_iter()
            .find_map(|mut backend| match backend.layouts() {
                Ok(list) if !list.is_empty() => {
                    info!(
                        "раскладку переключает {}: {}",
                        backend.name(),
                        list.join(", ")
                    );
                    Some(backend)
                }
                _ => None,
            })
    }

    fn backend(&mut self) -> &mut dyn Backend {
        let stale = self
            .last_probe
            .is_none_or(|t| t.elapsed() > Duration::from_secs(5));
        if self.found.is_none() && stale {
            self.last_probe = Some(Instant::now());
            self.found = Auto::probe();
        }
        match self.found.as_deref_mut() {
            Some(backend) => backend,
            None => &mut self.fallback,
        }
    }

    fn with<R>(&mut self, f: impl FnOnce(&mut dyn Backend) -> Result<R>) -> Result<R> {
        let result = f(self.backend());
        if result.is_err() && self.found.is_some() {
            self.found = None; // окружение сменилось — в следующий раз определим заново
            self.last_probe = None;
        }
        result
    }
}

impl Backend for Auto {
    fn name(&self) -> &'static str {
        self.found.as_ref().map_or("хоткей", |b| b.name())
    }

    fn layouts(&mut self) -> Result<Vec<String>> {
        self.with(|b| b.layouts())
    }

    fn current(&mut self) -> Result<Option<usize>> {
        self.with(|b| b.current())
    }

    fn set(&mut self, index: usize, keys: &mut dyn Keys) -> Result<()> {
        self.with(|b| b.set(index, keys))
    }

    fn toggle(&mut self, keys: &mut dyn Keys) -> Result<()> {
        self.with(|b| b.toggle(keys))
    }
}

pub fn make(cfg: &Config) -> Box<dyn Backend> {
    let hotkey = || Hotkey {
        keys: cfg.switch_hotkey.clone(),
        layouts: cfg.hotkey_layouts.clone(),
    };
    match cfg.backend {
        BackendKind::Auto => Box::new(Auto {
            found: None,
            fallback: hotkey(),
            last_probe: None,
        }),
        BackendKind::Kde => Box::new(Kde::default()),
        BackendKind::Hyprland => Box::new(Hyprland::default()),
        BackendKind::Sway => Box::new(Sway::default()),
        BackendKind::Hotkey => Box::new(hotkey()),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::*;
    use crate::engine::Stroke;

    struct NoKeys;

    impl Keys for NoKeys {
        fn backspace(&mut self, _: usize) -> Result<()> {
            Ok(())
        }
        fn type_strokes(&mut self, _: &[Stroke]) -> Result<()> {
            Ok(())
        }
        fn combo(&mut self, _: &[KeyCode]) -> Result<()> {
            Ok(())
        }
    }

    /// Временный исполняемый скрипт, подменяющий hyprctl/swaymsg.
    fn script(name: &str, body: &str) -> (PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("shiftfix-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        (dir, path.to_string_lossy().into_owned())
    }

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
            conn: None,
            address: Some(address),
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

    #[test]
    fn hyprland_over_hyprctl() {
        let (dir, hyprctl) = script(
            "hyprctl",
            r#"if [ "$1 $2" = "-j devices" ]; then
  echo '{"mice":[],"keyboards":[
    {"name":"shiftfix-virtual-keyboard","layout":"us,ru","active_keymap":"English (US)","main":false},
    {"name":"at-translated-set-2-keyboard","layout":"us,ru","variant":",","active_keymap":"Russian","main":true}]}'
elif [ "$1 $2" = "switchxkblayout all" ]; then
  echo "error: unknown device all"
elif [ "$1" = "switchxkblayout" ]; then
  echo "$2" >> "$(dirname "$0")/switched"; echo ok
fi"#,
        );
        let mut hypr = Hyprland { hyprctl };
        assert_eq!(hypr.layouts().unwrap(), ["us", "ru"]);
        assert_eq!(hypr.current().unwrap(), Some(1));
        hypr.set(0, &mut NoKeys).unwrap(); // старый Hyprland: по одной клавиатуре
        assert_eq!(
            fs::read_to_string(dir.join("switched")).unwrap(),
            "at-translated-set-2-keyboard\n"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sway_over_swaymsg() {
        let (dir, swaymsg) = script(
            "swaymsg",
            r#"if [ "$1 $2" = "-t get_inputs" ]; then
  echo '[{"name":"Power Button","type":"switch"},
         {"name":"AT Keyboard","type":"keyboard","xkb_layout_names":["English (US)","Russian"],"xkb_active_layout_index":0}]'
else
  echo '[{"success":true}]'
fi"#,
        );
        let mut sway = Sway { swaymsg };
        assert_eq!(sway.layouts().unwrap(), ["us", "ru"]);
        assert_eq!(sway.current().unwrap(), Some(0));
        sway.set(1, &mut NoKeys).unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn auto_falls_back_to_hotkey() {
        // В песочнице тестов нет ни KDE, ни Hyprland, ни Sway.
        let cfg = Config::default();
        let mut auto = make(&cfg);
        if auto.layouts().is_ok_and(|l| l == ["us", "ru"]) {
            assert_eq!(auto.current().unwrap(), None);
            assert_eq!(auto.name(), "хоткей");
        }
    }
}

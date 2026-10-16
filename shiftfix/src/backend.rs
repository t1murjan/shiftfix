//! Переключение раскладки: KWin по DBus, GNOME, Hyprland, Sway, XKB в X11
//! или эмуляция хоткея.
//! В режиме «авто» окружение определяется при первом обращении и заново —
//! если оно сменилось или служба стартовала раньше сессии.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use evdev::KeyCode;
use log::info;
use serde_json::Value;

use crate::config::{BackendKind, Config};
use crate::dbus::{Bus, Client};
use crate::engine::{Backend, Keys};
use crate::env;
use crate::keys::{code_from_long_name, norm_layout};
use crate::proc::run_text;
use crate::x11::XkbLayout;

// ─── KDE Plasma ─────────────────────────────────────────────────────────────

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
    /// Клавиатуры Hyprland; `ours` — вместе с виртуальной клавиатурой shiftfix.
    fn keyboards(&self, ours: bool) -> Result<Vec<Value>> {
        let out = run_text(&self.hyprctl, &["-j", "devices"])?;
        let value: Value = serde_json::from_str(&out).context("hyprctl: неожиданный ответ")?;
        let keyboards: Vec<Value> = value["keyboards"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|k| ours || !k["name"].as_str().unwrap_or_default().contains("shiftfix"))
            .cloned()
            .collect();
        if keyboards.is_empty() {
            bail!("hyprctl: клавиатуры не найдены");
        }
        Ok(keyboards)
    }

    fn main_keyboard(&self) -> Result<Value> {
        let keyboards = self.keyboards(false)?;
        let main = keyboards
            .iter()
            .find(|k| k["main"].as_bool() == Some(true))
            .unwrap_or(&keyboards[0]);
        Ok(main.clone())
    }

    fn switch(&self, arg: &str) -> Result<()> {
        // Старые версии не знают `all` — тогда по одной клавиатуре. В Hyprland у
        // каждой клавиатуры своя раскладка, и виртуальную надо переключить тоже:
        // это через неё перенабирается слово.
        if run_text(&self.hyprctl, &["switchxkblayout", "all", arg])
            .is_ok_and(|out| out.to_lowercase().contains("ok"))
        {
            return Ok(());
        }
        for kb in self.keyboards(true)? {
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

// ─── GNOME ──────────────────────────────────────────────────────────────────

/// Строки в одинарных кавычках из вывода `gsettings get` (формат GVariant).
fn gvariant_strings(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\'' {
            continue;
        }
        let mut value = String::new();
        while let Some(c) = chars.next() {
            match c {
                '\\' => value.extend(chars.next()),
                '\'' => break,
                c => value.push(c),
            }
        }
        out.push(value);
    }
    out
}

/// `<Super>space` → (модификаторы, клавиша).
pub fn parse_accel(accel: &str) -> Option<(Vec<KeyCode>, KeyCode)> {
    let mut mods = Vec::new();
    let mut rest = accel.trim();
    while let Some(tail) = rest.strip_prefix('<') {
        let (name, after) = tail.split_once('>')?;
        let key = match name.to_lowercase().as_str() {
            "super" | "mod4" | "meta" | "hyper" => KeyCode::KEY_LEFTMETA,
            "primary" | "control" | "ctrl" => KeyCode::KEY_LEFTCTRL,
            "alt" | "mod1" => KeyCode::KEY_LEFTALT,
            "shift" => KeyCode::KEY_LEFTSHIFT,
            _ => return None,
        };
        mods.push(key);
        rest = after;
    }
    let key = match rest.to_lowercase().as_str() {
        "space" => KeyCode::KEY_SPACE,
        "tab" => KeyCode::KEY_TAB,
        "caps_lock" => KeyCode::KEY_CAPSLOCK,
        "shift_l" => KeyCode::KEY_LEFTSHIFT,
        "shift_r" => KeyCode::KEY_RIGHTSHIFT,
        "alt_l" => KeyCode::KEY_LEFTALT,
        "alt_r" => KeyCode::KEY_RIGHTALT,
        "control_l" => KeyCode::KEY_LEFTCTRL,
        "control_r" => KeyCode::KEY_RIGHTCTRL,
        "super_l" => KeyCode::KEY_LEFTMETA,
        "super_r" => KeyCode::KEY_RIGHTMETA,
        "grave" => KeyCode::KEY_GRAVE,
        name if name.len() == 1 && name.chars().all(|c| c.is_ascii_alphanumeric()) => {
            format!("KEY_{}", name.to_uppercase()).parse().ok()?
        }
        name if name.starts_with('f')
            && name[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)) =>
        {
            format!("KEY_{}", name.to_uppercase()).parse().ok()?
        }
        _ => return None,
    };
    Some((mods, key))
}

/// GNOME: источники ввода из gsettings, переключение — сочетанием из настроек
/// GNOME (обычно Super+Space) с проверкой, что раскладка правда сменилась.
pub struct Gnome {
    gsettings: String,
    /// Сочетание из конфига на случай, если в GNOME его не прочитать.
    fallback: Vec<KeyCode>,
}

impl Gnome {
    pub fn new(fallback: Vec<KeyCode>) -> Self {
        Gnome {
            gsettings: "gsettings".into(),
            fallback,
        }
    }

    fn get(&self, schema: &str, key: &str) -> Result<String> {
        run_text(&self.gsettings, &["get", schema, key])
    }

    /// Пары (тип, id): `('xkb', 'us')`, `('ibus', 'mozc-jp')`.
    fn pairs(&self, key: &str) -> Result<Vec<(String, String)>> {
        let text = self.get("org.gnome.desktop.input-sources", key)?;
        let strings = gvariant_strings(&text);
        Ok(strings
            .chunks_exact(2)
            .map(|pair| (pair[0].clone(), pair[1].clone()))
            .collect())
    }

    fn sources(&self) -> Result<Vec<(String, String)>> {
        let sources = self.pairs("sources")?;
        if sources.is_empty() {
            bail!("в GNOME не настроены источники ввода");
        }
        Ok(sources)
    }

    /// Порядок, в котором GNOME перебирает источники: недавние первыми, текущий — нулевой.
    fn mru_order(&self, sources: &[(String, String)]) -> Vec<usize> {
        let mut order: Vec<usize> = self
            .pairs("mru-sources")
            .unwrap_or_default()
            .iter()
            .filter_map(|m| sources.iter().position(|s| s == m))
            .collect();
        order.dedup();
        for i in 0..sources.len() {
            if !order.contains(&i) {
                order.push(i);
            }
        }
        order
    }

    fn hotkey(&self) -> Option<(Vec<KeyCode>, KeyCode)> {
        let text = self
            .get("org.gnome.desktop.wm.keybindings", "switch-input-source")
            .ok()?;
        gvariant_strings(&text).iter().find_map(|a| parse_accel(a))
    }
}

impl Backend for Gnome {
    fn name(&self) -> &'static str {
        "GNOME"
    }

    fn layouts(&mut self) -> Result<Vec<String>> {
        Ok(self
            .sources()?
            .iter()
            .map(|(kind, id)| {
                if kind == "xkb" {
                    norm_layout(id.split('+').next().unwrap_or(id))
                } else {
                    id.clone()
                }
            })
            .collect())
    }

    fn current(&mut self) -> Result<Option<usize>> {
        let sources = self.sources()?;
        Ok(self.mru_order(&sources).first().copied())
    }

    fn set(&mut self, index: usize, keys: &mut dyn Keys) -> Result<()> {
        let sources = self.sources()?;
        if index >= sources.len() {
            bail!("GNOME: нет источника ввода №{}", index + 1);
        }
        let order = self.mru_order(&sources);
        let steps = order.iter().position(|&i| i == index).unwrap_or(1);
        if steps == 0 {
            return Ok(());
        }
        let (hold, tap) = self
            .hotkey()
            .or_else(|| {
                let (&tap, hold) = self.fallback.split_last()?;
                Some((hold.to_vec(), tap))
            })
            .context("GNOME: неизвестно сочетание переключения источников ввода")?;
        // Super+Space N раз при зажатом Super — как в переключателе GNOME.
        keys.chord(&hold, tap, steps)?;
        let deadline = Instant::now() + Duration::from_millis(700);
        loop {
            if self.mru_order(&sources).first() == Some(&index) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!(
                    "GNOME не переключил раскладку — проверьте сочетание «Переключиться на \\
                     следующий источник ввода» в настройках клавиатуры"
                );
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn toggle(&mut self, keys: &mut dyn Keys) -> Result<()> {
        let count = self.sources()?.len();
        let current = self.current()?.unwrap_or(0);
        self.set((current + 1) % count.max(1), keys)
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
    fn probe(hotkey: &[KeyCode]) -> Option<Box<dyn Backend>> {
        let mut candidates: Vec<Box<dyn Backend>> = Vec::new();
        if env::hyprland_signature().is_some() {
            candidates.push(Box::new(Hyprland::default()));
        }
        if env::sway_socket().is_some() {
            candidates.push(Box::new(Sway::default()));
        }
        candidates.push(Box::new(Kde::default()));
        if env::in_path("gsettings") && Client::new(Bus::Session).has_owner("org.gnome.Shell") {
            candidates.push(Box::new(Gnome::new(hotkey.to_vec())));
        }
        // Группа XKB у XWayland — не раскладка сеанса Wayland: только чистый X11.
        if env::wayland_display().is_none() && env::x11_display().is_some() {
            candidates.push(Box::new(XkbLayout::default()));
        }
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
            self.found = Auto::probe(&self.fallback.keys);
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
        BackendKind::Gnome => Box::new(Gnome::new(cfg.switch_hotkey.clone())),
        BackendKind::Hyprland => Box::new(Hyprland::default()),
        BackendKind::Sway => Box::new(Sway::default()),
        BackendKind::X11 => Box::new(XkbLayout::default()),
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
        // Старый Hyprland: по одной клавиатуре, включая виртуальную shiftfix —
        // через неё перенабирается слово.
        hypr.set(0, &mut NoKeys).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("switched")).unwrap(),
            "shiftfix-virtual-keyboard\nat-translated-set-2-keyboard\n"
        );
        // А раскладку определяет настоящая клавиатура, не виртуальная.
        assert_eq!(hypr.current().unwrap(), Some(1));
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

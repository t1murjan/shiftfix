//! Hyprland: раскладки и переключение через `hyprctl`.

use anyhow::{Result, bail};
use serde_json::Value;

use super::{is_ours, run_json};
use crate::engine::{Backend, Keys};
use crate::keymap::{code_from_long_name, norm_layout};
use crate::sys::proc::run_text;

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
        let value = run_json(&self.hyprctl, &["-j", "devices"])?;
        let keyboards: Vec<Value> = value["keyboards"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|k| ours || !is_ours(k))
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
            .is_ok_and(|out| succeeded(&out))
        {
            return Ok(());
        }
        for kb in self.keyboards(true)? {
            let name = kb["name"].as_str().unwrap_or_default();
            let out = run_text(&self.hyprctl, &["switchxkblayout", name, arg])?;
            if !succeeded(&out) {
                bail!("hyprctl switchxkblayout {name}: {}", out.trim());
            }
        }
        Ok(())
    }
}

/// Удача — ответ «ok» (по строке на клавиатуру, если `all` отвечает за
/// каждую). Искать «ok» внутри ответа нельзя: оно бывает в тексте ошибки.
fn succeeded(out: &str) -> bool {
    let mut lines = out.lines().map(str::trim).filter(|l| !l.is_empty());
    !out.trim().is_empty() && lines.all(|l| l.eq_ignore_ascii_case("ok"))
}

fn split_layouts(list: &str) -> Vec<String> {
    list.split(',').map(norm_layout).collect()
}

impl Backend for Hyprland {
    fn name(&self) -> &'static str {
        "Hyprland"
    }

    fn layouts(&mut self) -> Result<Vec<String>> {
        let kb = self.main_keyboard()?;
        let list = kb["layout"].as_str().unwrap_or_default();
        if list.is_empty() {
            bail!("hyprctl: раскладки клавиатуры неизвестны");
        }
        Ok(split_layouts(list))
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::testutil::{NoKeys, fake_command};

    #[test]
    fn hyprland_over_hyprctl() {
        let (dir, hyprctl) = fake_command(
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
    }

    #[test]
    fn hyprland_error_with_ok_inside_is_error() {
        // Удача — ровно «ok», а не любой ответ, где встречается «ok».
        let (_dir, hyprctl) = fake_command(
            "hyprctl",
            r#"if [ "$1 $2" = "-j devices" ]; then
  echo '{"keyboards":[{"name":"kb","layout":"us,ru","main":true}]}'
else
  echo "error: keyboard lookup failed"
fi"#,
        );
        let mut hypr = Hyprland { hyprctl };
        assert!(hypr.set(1, &mut NoKeys).is_err());
    }

    #[test]
    fn hyprland_without_layouts_is_error() {
        // Иначе «авто» выбрал бы Hyprland со списком из одной пустой раскладки.
        let (_dir, hyprctl) = fake_command(
            "hyprctl",
            r#"echo '{"keyboards":[{"name":"kb","layout":"","main":true}]}'"#,
        );
        let mut hypr = Hyprland { hyprctl };
        assert!(hypr.layouts().is_err());
    }

    #[test]
    fn hyprland_switches_all_keyboards_at_once() {
        // Современный Hyprland: номер активной раскладки в ответе и `all`.
        let (dir, hyprctl) = fake_command(
            "hyprctl",
            r#"if [ "$1 $2" = "-j devices" ]; then
  echo '{"keyboards":[
    {"name":"at-translated-set-2-keyboard","layout":"us,ru","active_keymap":"English (US)","active_layout_index":1,"main":true}]}'
elif [ "$1" = "switchxkblayout" ]; then
  echo "$2 $3" >> "$(dirname "$0")/switched"; echo ok
fi"#,
        );
        let mut hypr = Hyprland { hyprctl };
        assert_eq!(hypr.current().unwrap(), Some(1));
        hypr.set(0, &mut NoKeys).unwrap();
        hypr.toggle(&mut NoKeys).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("switched")).unwrap(),
            "all 0\nall next\n"
        );
    }
}

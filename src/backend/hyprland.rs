//! Hyprland: раскладки и переключение через `hyprctl`.

use anyhow::{Context, Result, bail};
use serde_json::Value;

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
}

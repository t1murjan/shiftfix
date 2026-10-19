//! Sway: раскладки и переключение через `swaymsg`.

use std::cmp::Reverse;

use anyhow::{Result, anyhow};
use log::warn;
use serde_json::Value;

use super::{is_ours, run_json};
use crate::engine::{Backend, Keys};
use crate::keymap::code_from_long_name;
use crate::sys::proc::run_text;

pub struct Sway {
    swaymsg: String,
    /// О раскладках виртуальной клавиатуры уже предупредили.
    warned: bool,
}

impl Default for Sway {
    fn default() -> Self {
        Sway {
            swaymsg: "swaymsg".into(),
            warned: false,
        }
    }
}

impl Sway {
    /// Клавиатура, у которой берём раскладки. «Power Button», «Video Bus» и т. п.
    /// в sway тоже клавиатуры и обычно идут первыми, а раскладки, заданные для
    /// конкретной клавиатуры, есть только у неё, — берём ту, где их больше.
    fn keyboard(&mut self) -> Result<Value> {
        let inputs = run_json(&self.swaymsg, &["-t", "get_inputs", "-r"])?;
        let keyboards: Vec<&Value> = inputs
            .as_array()
            .into_iter()
            .flatten()
            .filter(|i| i["type"] == "keyboard")
            .collect();
        let count = |kb: &Value| kb["xkb_layout_names"].as_array().map_or(0, Vec::len);
        let main = keyboards
            .iter()
            .filter(|kb| !is_ours(kb))
            .min_by_key(|kb| Reverse(count(kb)))
            .copied()
            .ok_or_else(|| anyhow!("swaymsg: клавиатуры не найдены"))?;
        // Слово перенабирается через виртуальную клавиатуру shiftfix: без тех же
        // раскладок она напечатает его в своей.
        if !self.warned
            && keyboards
                .iter()
                .any(|kb| is_ours(kb) && count(kb) < count(main))
        {
            self.warned = true;
            warn!(
                "в sway раскладки заданы не для всех клавиатур, и виртуальная клавиатура \
                 shiftfix их не получила — задайте их через input type:keyboard {{ xkb_layout … }}"
            );
        }
        Ok(main.clone())
    }

    fn switch(&self, arg: &str) -> Result<()> {
        run_text(
            &self.swaymsg,
            &["input", "type:keyboard", "xkb_switch_layout", arg],
        )
        .map(drop)
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
    fn sway_over_swaymsg() {
        // Как в sway: Power Button — тоже клавиатура и идёт первой, а раскладки
        // заданы только настоящей (input "1:1:AT_…" { xkb_layout us,ru }).
        let (dir, swaymsg) = fake_command(
            "swaymsg",
            r#"if [ "$1 $2" = "-t get_inputs" ]; then
  echo '[{"name":"Power Button","type":"keyboard","xkb_layout_names":["English (US)"],"xkb_active_layout_index":0},
         {"name":"Lid Switch","type":"switch"},
         {"name":"AT Keyboard","type":"keyboard","xkb_layout_names":["English (US)","Russian"],"xkb_active_layout_index":1},
         {"name":"shiftfix virtual keyboard","type":"keyboard","xkb_layout_names":["English (US)"],"xkb_active_layout_index":0}]'
else
  echo "$*" >> "$(dirname "$0")/switched"; echo '[{"success":true}]'
fi"#,
        );
        let mut sway = Sway {
            swaymsg,
            warned: false,
        };
        assert_eq!(sway.layouts().unwrap(), ["us", "ru"]);
        assert_eq!(sway.current().unwrap(), Some(1));
        assert!(sway.warned, "у виртуальной клавиатуры одна раскладка");
        sway.set(1, &mut NoKeys).unwrap();
        sway.toggle(&mut NoKeys).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("switched")).unwrap(),
            "input type:keyboard xkb_switch_layout 1\n\
             input type:keyboard xkb_switch_layout next\n"
        );
    }
}

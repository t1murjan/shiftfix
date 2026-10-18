//! GNOME: источники ввода из gsettings, переключение — сочетанием клавиш GNOME.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use evdev::KeyCode;

use crate::engine::{Backend, Keys};
use crate::keymap::norm_layout;
use crate::sys::proc::run_text;

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
fn parse_accel(accel: &str) -> Option<(Vec<KeyCode>, KeyCode)> {
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

    fn pairs(&self, key: &str) -> Result<Vec<Source>> {
        Ok(source_pairs(
            &self.get("org.gnome.desktop.input-sources", key)?,
        ))
    }

    fn sources(&self) -> Result<Vec<Source>> {
        non_empty(self.pairs("sources")?)
    }

    /// `sources` и `mru-sources` одним запуском gsettings: `current()` нужен
    /// в начале каждого слова.
    fn sources_and_mru(&self) -> Result<(Vec<Source>, Vec<Source>)> {
        let text = run_text(
            &self.gsettings,
            &["list-recursively", "org.gnome.desktop.input-sources"],
        )?;
        let (mut sources, mut mru) = (Vec::new(), Vec::new());
        for line in text.lines() {
            // «org.gnome.desktop.input-sources sources [('xkb', 'us'), …]»
            let mut parts = line.splitn(3, ' ');
            match (parts.next(), parts.next(), parts.next()) {
                (Some(_), Some("sources"), Some(value)) => sources = source_pairs(value),
                (Some(_), Some("mru-sources"), Some(value)) => mru = source_pairs(value),
                _ => {}
            }
        }
        Ok((non_empty(sources)?, mru))
    }

    /// Порядок источников по `mru-sources` прямо сейчас.
    fn order_now(&self, sources: &[Source]) -> Vec<usize> {
        mru_order(sources, &self.pairs("mru-sources").unwrap_or_default())
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
        let (sources, mru) = self.sources_and_mru()?;
        Ok(mru_order(&sources, &mru).first().copied())
    }

    fn set(&mut self, index: usize, keys: &mut dyn Keys) -> Result<()> {
        let (sources, mru) = self.sources_and_mru()?;
        if index >= sources.len() {
            bail!("GNOME: нет источника ввода №{}", index + 1);
        }
        let order = mru_order(&sources, &mru);
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
            if self.order_now(&sources).first() == Some(&index) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!(
                    "GNOME не переключил раскладку — проверьте сочетание «Переключиться на \
                     следующий источник ввода» в настройках клавиатуры"
                );
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn toggle(&mut self, keys: &mut dyn Keys) -> Result<()> {
        let (sources, mru) = self.sources_and_mru()?;
        let current = mru_order(&sources, &mru).first().copied().unwrap_or(0);
        self.set((current + 1) % sources.len(), keys)
    }
}

/// Источник ввода GNOME: (тип, id) — `('xkb', 'us')`, `('ibus', 'mozc-jp')`.
type Source = (String, String);

fn source_pairs(gvariant: &str) -> Vec<Source> {
    let strings = gvariant_strings(gvariant);
    let (pairs, _) = strings.as_chunks::<2>();
    pairs
        .iter()
        .map(|[kind, id]| (kind.clone(), id.clone()))
        .collect()
}

fn non_empty(sources: Vec<Source>) -> Result<Vec<Source>> {
    if sources.is_empty() {
        bail!("в GNOME не настроены источники ввода");
    }
    Ok(sources)
}

/// Порядок, в котором GNOME перебирает источники: недавние первыми, текущий — нулевой.
fn mru_order(sources: &[Source], mru: &[Source]) -> Vec<usize> {
    let mut order: Vec<usize> = mru
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::keys::Stroke;
    use crate::testutil::fake_command;

    #[test]
    fn gnome_over_gsettings() {
        let (dir, gsettings) = fake_command(
            "gsettings",
            r#"d=$(dirname "$0")
src="[('xkb', 'us'), ('xkb', 'ru+phonetic')]"
mru=$(cat "$d/mru")
if [ "$1" = list-recursively ]; then
  echo list >>"$d/calls"
  echo "org.gnome.desktop.input-sources current uint32 0"
  echo "org.gnome.desktop.input-sources mru-sources $mru"
  echo "org.gnome.desktop.input-sources per-window false"
  echo "org.gnome.desktop.input-sources sources $src"
elif [ "$3" = sources ]; then echo "$src"
elif [ "$3" = mru-sources ]; then echo "$mru"
elif [ "$3" = switch-input-source ]; then echo "['<Super>space', 'XF86Keyboard']"
fi"#,
        );
        fs::write(dir.join("mru"), "[('xkb', 'ru+phonetic'), ('xkb', 'us')]").unwrap();
        let mut gnome = Gnome {
            gsettings,
            fallback: Vec::new(),
        };
        assert_eq!(gnome.layouts().unwrap(), ["us", "ru"]);
        assert_eq!(gnome.current().unwrap(), Some(1));
        assert_eq!(
            fs::read_to_string(dir.join("calls")).unwrap(),
            "list\n",
            "current() — один запуск gsettings"
        );

        /// Как GNOME Shell: Super+Space делает текущим следующий по MRU источник.
        struct Shell(PathBuf);
        impl Keys for Shell {
            fn backspace(&mut self, _: usize) -> Result<()> {
                Ok(())
            }
            fn type_strokes(&mut self, _: &[Stroke]) -> Result<()> {
                Ok(())
            }
            fn combo(&mut self, _: &[KeyCode]) -> Result<()> {
                Ok(())
            }
            fn chord(&mut self, hold: &[KeyCode], tap: KeyCode, times: usize) -> Result<()> {
                assert_eq!(
                    (hold, tap, times),
                    (&[KeyCode::KEY_LEFTMETA][..], KeyCode::KEY_SPACE, 1)
                );
                fs::write(
                    self.0.join("mru"),
                    "[('xkb', 'us'), ('xkb', 'ru+phonetic')]",
                )?;
                Ok(())
            }
        }
        gnome.set(0, &mut Shell(dir.to_path_buf())).unwrap();
        assert_eq!(gnome.current().unwrap(), Some(0));
    }
}

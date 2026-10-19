//! Отчёт для окна: последние исправления и сведения о раскладках.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::Engine;

/// Сколько последних исправлений служба хранит и отдаёт окну по IPC (окно
/// показывает из них не все).
const ACTIONS_MAX: usize = 30;
/// Сколько секунд GUI получает сведения о раскладках из кеша: в GNOME и Hyprland
/// каждое обращение к переключателю — запуск процесса, а GUI спрашивает часто.
pub(super) const LAYOUT_INFO_TTL: f64 = 1.5;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    #[default]
    Word,
    Selection,
    /// Повторный ⇧⇧ сразу после исправления выделения: тот же фрагмент — в
    /// следующую раскладку. При двух раскладках это возврат, при трёх — по кругу.
    Undo,
}

/// Одно исправление — для списка в GUI. Текста здесь нет, только длина.
/// Полей, которых служба не прислала, — по умолчанию, как в `ipc::Status`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Action {
    /// Unix-время, секунды.
    pub time: u64,
    pub kind: ActionKind,
    /// Длина исправленного в символах, считая пробелы: у слова — вместе с
    /// пробелами после него.
    pub chars: usize,
    pub from: String,
    pub to: String,
}

/// Сведения о раскладках для GUI; полей, которых служба не прислала, — по
/// умолчанию, как в `ipc::Status`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutInfo {
    pub backend: String,
    pub codes: Vec<String>,
    pub current: Option<usize>,
    /// Таблицы взяты из настоящей keymap, а не встроенные.
    pub from_keymap: bool,
    pub error: Option<String>,
}

impl Engine {
    pub fn actions(&self) -> Vec<Action> {
        self.actions.iter().cloned().collect()
    }

    /// Раскладки и текущая раскладка — для GUI.
    pub(crate) fn layout_info(&mut self) -> LayoutInfo {
        let now = (self.hooks.clock)();
        if let Some((at, info)) = &self.info_cache
            && now - at < LAYOUT_INFO_TTL
        {
            return info.clone();
        }
        let (codes, current, error) = match self.backend.layouts() {
            Ok(codes) => (codes, self.current_layout(), None),
            Err(err) => (Vec::new(), None, Some(format!("{err:#}"))),
        };
        // Имя — после layouts(): Auto выбирает переключатель при первом обращении.
        let backend = self.backend.name().to_owned();
        let from_keymap = self.keymap_layouts(&codes).is_some();
        if !codes.is_empty() {
            self.codes.clone_from(&codes);
        }
        let info = LayoutInfo {
            backend,
            codes,
            current,
            from_keymap,
            error,
        };
        self.info_cache = Some((now, info.clone()));
        info
    }

    /// Как назвать раскладку в журнале и в окне: код, номер, если кода нет,
    /// или «?», если раскладка неизвестна.
    pub(super) fn label(&self, index: Option<usize>) -> String {
        match index {
            Some(i) => self
                .codes
                .get(i)
                .cloned()
                .unwrap_or_else(|| format!("№{}", i + 1)),
            None => "?".into(),
        }
    }

    pub(super) fn record(
        &mut self,
        kind: ActionKind,
        chars: usize,
        from: Option<usize>,
        to: Option<usize>,
    ) {
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let action = Action {
            time,
            kind,
            chars,
            from: self.label(from),
            to: self.label(to),
        };
        if self.actions.len() == ACTIONS_MAX {
            self.actions.pop_front();
        }
        self.actions.push_back(action);
    }
}

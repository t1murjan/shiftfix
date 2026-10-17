//! Логика без ввода-вывода: события клавиатуры и мыши → решения.
//!
//! Буфер хранит нажатые клавиши (скан-коды + Shift), а не символы. Исправить
//! слово — значит переключить раскладку, стереть N символов и нажать те же
//! клавиши ещё раз; таблицы для этого не нужны.

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use evdev::KeyCode;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};

use crate::config::{Config, SelectionMethod};
use crate::keymap::{self, Keymap, Layout};
use crate::keys::{is_char, is_ctrl, is_modifier, is_nav, is_neutral, is_shift};

/// Сколько последних нажатий помнить.
pub const BUF_MAX: usize = 256;
/// Вставку длиннее нельзя «прокрутить» повторным ⇧⇧.
pub const SEGMENT_MAX: usize = 200;
/// Касание тачпада короче этого считаем кликом (тап).
const TOUCH_TAP_MAX: f64 = 0.3;
/// Столько секунд после нашей вставки события PRIMARY не учитываются.
const SELF_CHANGE_GRACE: f64 = 2.0;
/// Сколько последних исправлений показывать в GUI.
const ACTIONS_MAX: usize = 30;
/// Сколько секунд GUI получает сведения о раскладках из кеша: в GNOME и Hyprland
/// каждое обращение к переключателю — запуск процесса, а GUI спрашивает часто.
const LAYOUT_INFO_TTL: f64 = 1.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stroke {
    pub key: KeyCode,
    pub shift: bool,
    /// Индекс раскладки, в которой клавиша нажата (если известен).
    pub layout: Option<usize>,
}

/// Эмуляция нажатий.
pub trait Keys {
    /// Новые настройки (задержка эмуляции и т.п.).
    fn configure(&mut self, _cfg: &Config) {}
    fn backspace(&mut self, count: usize) -> Result<()>;
    fn type_strokes(&mut self, strokes: &[Stroke]) -> Result<()>;
    fn combo(&mut self, keys: &[KeyCode]) -> Result<()>;
    /// Зажать `hold`, нажать `tap` `times` раз, отпустить (Super+Space+Space в GNOME).
    fn chord(&mut self, hold: &[KeyCode], tap: KeyCode, times: usize) -> Result<()> {
        let keys: Vec<KeyCode> = hold.iter().copied().chain([tap]).collect();
        (0..times).try_for_each(|_| self.combo(&keys))
    }
}

/// Переключение раскладки.
pub trait Backend {
    fn name(&self) -> &'static str;
    fn layouts(&mut self) -> Result<Vec<String>>;
    /// `None` — текущая раскладка неизвестна (переключение хоткеем).
    fn current(&mut self) -> Result<Option<usize>>;
    fn set(&mut self, index: usize, keys: &mut dyn Keys) -> Result<()>;
    fn toggle(&mut self, keys: &mut dyn Keys) -> Result<()>;
}

/// Выделенный текст и вставка.
pub trait Selection {
    fn read_primary(&mut self) -> Option<String>;
    fn paste(&mut self, text: &str, keys: &mut dyn Keys) -> Result<()>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Word,
    Selection,
    /// Повторный ⇧⇧ сразу после исправления выделения.
    Undo,
}

/// Одно исправление — для списка в GUI. Текста здесь нет, только длина.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Action {
    /// Unix-время, секунды.
    pub time: u64,
    pub kind: ActionKind,
    pub chars: usize,
    pub from: String,
    pub to: String,
}

/// Сведения о раскладках для GUI.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LayoutInfo {
    pub backend: String,
    pub codes: Vec<String>,
    pub names: Vec<String>,
    pub current: Option<usize>,
    /// Таблицы взяты из настоящей keymap, а не встроенные.
    pub from_keymap: bool,
    pub error: Option<String>,
}

#[derive(Clone, Copy)]
struct Held {
    any: bool,
    shift: bool,
    modifier: bool,
}

/// Почему исправлять сейчас нельзя (`None` — можно): неактивный сеанс и т.п.
pub type Guard = Box<dyn FnMut() -> Option<String>>;

pub struct Engine {
    cfg: Config,
    backend: Box<dyn Backend>,
    keys: Box<dyn Keys>,
    clip: Option<Box<dyn Selection>>,
    caps_on: Box<dyn FnMut() -> bool>,
    clock: Box<dyn Fn() -> f64>,
    guard: Guard,
    keymap: Keymap,
    enabled: bool,
    buf: Vec<Stroke>,
    /// Буфер — только что вставленный фрагмент целиком.
    segment: bool,
    /// Зажатые физические клавиши.
    down: HashSet<KeyCode>,
    /// «Чистое» нажатие Shift: в этот момент больше ничего не зажато.
    shift_press: Option<f64>,
    /// Отпускание первого Shift двойного нажатия.
    first_tap: Option<f64>,
    /// Одиночный Ctrl/Alt/Meta без других клавиш.
    lone_mod: Option<KeyCode>,
    touch_down: HashMap<String, f64>,
    /// В PRIMARY актуальное выделение.
    sel_valid: bool,
    sel_ignore_until: f64,
    /// Раскладка, в которой сейчас набираются буквы.
    layout: Option<usize>,
    layout_dirty: bool,
    /// Коды раскладок с последнего обращения к переключателю (для журнала).
    codes: Vec<String>,
    actions: VecDeque<Action>,
    /// Последний ответ `layout_info()` и когда он получен.
    info_cache: Option<(f64, LayoutInfo)>,
}

impl Engine {
    pub fn new(
        cfg: Config,
        backend: Box<dyn Backend>,
        keys: Box<dyn Keys>,
        clip: Option<Box<dyn Selection>>,
        caps_on: Box<dyn FnMut() -> bool>,
        clock: Box<dyn Fn() -> f64>,
        guard: Guard,
    ) -> Self {
        Engine {
            cfg,
            backend,
            keys,
            clip,
            caps_on,
            clock,
            guard,
            keymap: Keymap::default(),
            enabled: true,
            buf: Vec::new(),
            segment: false,
            down: HashSet::new(),
            shift_press: None,
            first_tap: None,
            lone_mod: None,
            touch_down: HashMap::new(),
            sel_valid: false,
            sel_ignore_until: 0.0,
            layout: None,
            layout_dirty: true,
            codes: Vec::new(),
            actions: VecDeque::new(),
            info_cache: None,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn selection_enabled(&self) -> bool {
        self.clip.is_some()
    }

    pub fn set_clip(&mut self, clip: Option<Box<dyn Selection>>) {
        self.clip = clip;
        self.sel_valid = false;
    }

    pub fn set_config(&mut self, cfg: Config) {
        self.keys.configure(&cfg);
        self.cfg = cfg;
    }

    pub fn set_backend(&mut self, backend: Box<dyn Backend>) {
        self.backend = backend;
        self.info_cache = None;
        self.layout_dirty = true;
        self.reset("сменился переключатель раскладки");
    }

    pub fn set_keymap(&mut self, keymap: Keymap) {
        self.keymap = keymap;
        self.info_cache = None;
    }

    pub fn actions(&self) -> Vec<Action> {
        self.actions.iter().cloned().collect()
    }

    /// Раскладки и текущая раскладка — для GUI.
    pub fn layout_info(&mut self) -> LayoutInfo {
        let now = (self.clock)();
        if let Some((at, info)) = &self.info_cache
            && now - at < LAYOUT_INFO_TTL
        {
            return info.clone();
        }
        let (codes, current, error) = match self.backend.layouts() {
            Ok(codes) => {
                let current = self.backend.current().ok().flatten();
                (codes, current, None)
            }
            Err(err) => (Vec::new(), None, Some(format!("{err:#}"))),
        };
        // Имя — после layouts(): Auto выбирает переключатель при первом обращении.
        let backend = self.backend.name().to_owned();
        let from_keymap =
            !self.keymap.layouts.is_empty() && self.keymap.layouts.len() == codes.len();
        let names = if from_keymap {
            self.keymap.layouts.iter().map(|l| l.name.clone()).collect()
        } else {
            codes.clone()
        };
        if !codes.is_empty() {
            self.codes.clone_from(&codes);
        }
        let info = LayoutInfo {
            backend,
            codes,
            names,
            current,
            from_keymap,
            error,
        };
        self.info_cache = Some((now, info.clone()));
        info
    }

    // ─── входящие события ───
    pub fn key(&mut self, key: KeyCode, value: i32, ts: f64) {
        match value {
            1 => {
                let held = Held {
                    any: !self.down.is_empty(),
                    shift: self.down.iter().any(|&k| is_shift(k)),
                    modifier: self.down.iter().any(|&k| is_modifier(k)),
                };
                self.down.insert(key);
                if self.enabled {
                    self.press(key, held, ts);
                }
            }
            0 => {
                self.down.remove(&key);
                if self.enabled {
                    self.release(key, ts);
                }
            }
            // Автоповтор на Wayland делает само приложение со своей частотой:
            // сколько символов оно напечатало или стёрло — не узнать.
            2 if self.enabled && (is_char(key) || key == KeyCode::KEY_BACKSPACE) => {
                self.reset("автоповтор")
            }
            _ => {}
        }
    }

    pub fn button(&mut self, value: i32, _ts: f64) {
        if value != 1 {
            return;
        }
        self.clear_taps();
        self.layout_dirty = true;
        if self.enabled {
            self.sel_valid = false; // клик снимает выделение; новое придёт событием PRIMARY
            self.reset("клик");
        }
    }

    pub fn touch(&mut self, device: &str, value: i32, ts: f64) {
        match value {
            1 => {
                self.touch_down.insert(device.to_owned(), ts);
            }
            0 => {
                let tapped = self
                    .touch_down
                    .remove(device)
                    .is_some_and(|start| ts - start <= TOUCH_TAP_MAX);
                if tapped {
                    self.button(1, ts); // тап по тачпаду = клик
                }
            }
            _ => {}
        }
    }

    pub fn primary_changed(&mut self, size: usize) {
        if self.enabled && (self.clock)() >= self.sel_ignore_until {
            self.sel_valid = size > 0;
        }
    }

    /// После потери событий: зажатые клавиши берутся у ядра, буфер забывается.
    pub fn resync(&mut self, held: HashSet<KeyCode>) {
        self.down = held;
        self.clear_taps();
        self.layout_dirty = true;
        self.reset("пересинхронизация");
    }

    pub fn set_enabled(&mut self, flag: bool) {
        self.enabled = flag;
        self.sel_valid = false;
        self.clear_taps();
        self.layout_dirty = true;
        self.reset(if flag {
            "снова работаю"
        } else {
            "пауза"
        });
    }

    // ─── разбор нажатий ───

    fn press(&mut self, key: KeyCode, held: Held, ts: f64) {
        if is_shift(key) {
            self.lone_mod = None;
            if held.any {
                // Alt+Shift, Ctrl+Shift, Shift+Shift — возможно, смена раскладки.
                self.shift_press = None;
                self.first_tap = None;
                self.layout_dirty = true;
            } else {
                self.shift_press = Some(ts);
            }
            return;
        }
        self.shift_press = None;
        self.first_tap = None;

        if is_modifier(key) {
            self.lone_mod = if held.any { None } else { Some(key) };
            self.layout_dirty = true;
            return;
        }
        self.lone_mod = None;
        if is_neutral(key) {
            self.layout_dirty |= key == KeyCode::KEY_CAPSLOCK;
            return;
        }
        if held.modifier {
            // Ctrl+V, Alt+Tab, Ctrl+Backspace… Выделение переживает только копирование
            // и Ctrl+Shift+стрелки; Ctrl+A пришлёт новое событием PRIMARY.
            let keeps_selection =
                key == KeyCode::KEY_C || key == KeyCode::KEY_INSERT || (held.shift && is_nav(key));
            if !keeps_selection {
                self.sel_valid = false;
            }
            self.reset("сочетание клавиш");
            self.layout_dirty = true;
            return;
        }
        if is_char(key) {
            self.typed(key, held.shift);
        } else if key == KeyCode::KEY_BACKSPACE {
            self.sel_valid = false;
            self.segment = false;
            self.buf.pop();
        } else {
            // Стрелки, Home/End, Enter, Tab, Esc, Delete, F1…
            if !(held.shift && is_nav(key)) {
                self.sel_valid = false; // Shift+стрелки выделение только расширяют
            }
            self.reset("курсор сдвинулся");
            self.layout_dirty = true;
        }
    }

    fn release(&mut self, key: KeyCode, ts: f64) {
        if is_shift(key) {
            match self.shift_press.take() {
                Some(pressed) if ts - pressed <= self.cfg.tap_max_hold => {
                    if self
                        .first_tap
                        .is_some_and(|first| pressed - first <= self.cfg.double_tap)
                    {
                        self.first_tap = None;
                        self.trigger();
                    } else {
                        self.first_tap = Some(ts);
                    }
                }
                _ => self.first_tap = None,
            }
        } else if self.lone_mod == Some(key) {
            self.lone_mod = None;
            if !is_ctrl(key) {
                // Одиночный Alt открывает меню, Meta — лаунчер: фокус ушёл.
                self.sel_valid = false;
                self.reset("одиночный Alt/Meta");
            }
        }
    }

    fn typed(&mut self, key: KeyCode, shift: bool) {
        self.sel_valid = false;
        self.segment = false;
        let word_start = self.buf.last().is_none_or(|s| s.key == KeyCode::KEY_SPACE);
        if key != KeyCode::KEY_SPACE && (self.layout_dirty || word_start) {
            self.layout = self.current_layout();
            self.layout_dirty = false;
        }
        // Мёртвая клавиша (´, ¨ в раскладках с акцентами) печатает не символ, а
        // часть следующего: сколько символов стирать — уже не узнать.
        let dead = self
            .layout
            .and_then(|i| self.keymap.layouts.get(i))
            .is_some_and(|l| l.is_dead(key, shift));
        if dead {
            self.reset("мёртвая клавиша");
            return;
        }
        self.buf.push(Stroke {
            key,
            shift,
            layout: self.layout,
        });
        if self.buf.len() > BUF_MAX {
            let extra = self.buf.len() - BUF_MAX;
            self.buf.drain(..extra);
        }
    }

    fn clear_taps(&mut self) {
        self.shift_press = None;
        self.first_tap = None;
        self.lone_mod = None;
    }

    fn reset(&mut self, why: &str) {
        if !self.buf.is_empty() {
            debug!("буфер сброшен: {why}");
        }
        self.buf.clear();
        self.segment = false;
    }

    fn current_layout(&mut self) -> Option<usize> {
        self.backend.current().unwrap_or_else(|err| {
            debug!("текущая раскладка неизвестна: {err:#}");
            None
        })
    }

    // ─── исправление ───

    pub fn trigger(&mut self) {
        if let Some(reason) = (self.guard)() {
            info!("⇧⇧ пропущен: {reason}");
            self.sel_valid = false;
            self.reset("исправлять нельзя");
            return;
        }
        let result = if self.sel_valid && self.clip.is_some() {
            self.fix_selection()
        } else if !self.buf.is_empty() {
            self.fix_word()
        } else {
            debug!("⇧⇧: исправлять нечего");
            Ok(())
        };
        if let Err(err) = result {
            error!("{err:#}");
            self.reset("ошибка");
        }
    }

    fn fix_word(&mut self) -> Result<()> {
        let undo = self.segment;
        let start = if self.segment {
            0
        } else {
            let mut end = self.buf.len();
            while end > 0 && self.buf[end - 1].key == KeyCode::KEY_SPACE {
                end -= 1;
            }
            let mut start = end;
            while start > 0 && self.buf[start - 1].key != KeyCode::KEY_SPACE {
                start -= 1;
            }
            if start == end {
                debug!("⇧⇧: после пробелов нет слова");
                return Ok(());
            }
            if end - start > self.cfg.max_word_len {
                debug!("⇧⇧: слово длиннее max_word_len, пропускаю");
                return Ok(());
            }
            start
        };
        // Слово вместе с пробелами после него.
        let count = self.buf.len() - start;
        let source = self.buf[start].layout;
        let target = self.next_layout(source)?;
        if let Some(t) = target
            && self.has_dead_key(t, &self.buf[start..])
        {
            // Мёртвая клавиша напечатает не символ, а часть следующего: слово
            // вышло бы другим, а повторный ⇧⇧ стёр бы лишнее.
            info!(
                "⇧⇧: в раскладке {} среди этих клавиш есть мёртвая — слово не исправить",
                self.label(Some(t))
            );
            return Ok(());
        }
        self.switch(target)?;
        self.keys.backspace(count)?;
        self.keys.type_strokes(&self.buf[start..])?;
        for stroke in &mut self.buf[start..] {
            stroke.layout = target;
        }
        self.layout = target;
        self.layout_dirty = target.is_none();
        let kind = if undo {
            ActionKind::Undo
        } else {
            ActionKind::Word
        };
        self.record(kind, count, source, target);
        info!(
            "слово: {count} нажатий, {} → {}",
            self.label(source),
            self.label(target)
        );
        Ok(())
    }

    /// Раскладка после `src`; `None` — текущая неизвестна (переключение хоткеем).
    fn next_layout(&mut self, src: Option<usize>) -> Result<Option<usize>> {
        self.codes = self.backend.layouts()?;
        let count = self.codes.len();
        let src = match src {
            Some(s) if s < count => Some(s),
            _ => self.backend.current()?,
        };
        match src {
            None => Ok(None),
            Some(_) if count < 2 => bail!("настроена только одна раскладка"),
            Some(s) => Ok(Some((s + 1) % count)),
        }
    }

    /// Переключить на `target`; `None` — просто на следующую.
    fn switch(&mut self, target: Option<usize>) -> Result<()> {
        self.info_cache = None;
        match target {
            None => self.backend.toggle(self.keys.as_mut())?,
            Some(t) => self.backend.set(t, self.keys.as_mut())?,
        }
        thread::sleep(self.cfg.switch_delay);
        Ok(())
    }

    /// Есть ли среди `strokes` мёртвая клавиша раскладки `layout`.
    fn has_dead_key(&self, layout: usize, strokes: &[Stroke]) -> bool {
        self.keymap
            .layouts
            .get(layout)
            .is_some_and(|l| strokes.iter().any(|s| l.is_dead(s.key, s.shift)))
    }

    fn switch_to(&mut self, target: usize, current: Option<usize>) -> Result<()> {
        self.info_cache = None;
        match current {
            None => self.backend.toggle(self.keys.as_mut())?,
            Some(c) if c != target => self.backend.set(target, self.keys.as_mut())?,
            Some(_) => return Ok(()),
        }
        thread::sleep(self.cfg.switch_delay);
        Ok(())
    }

    /// Таблицы по индексам раскладок: из keymap, если она совпадает со списком
    /// переключателя, иначе встроенные по кодам.
    fn tables(&self, codes: &[String]) -> Vec<Option<Arc<Layout>>> {
        if !self.keymap.layouts.is_empty() && self.keymap.layouts.len() == codes.len() {
            self.keymap.layouts.iter().cloned().map(Some).collect()
        } else {
            codes.iter().map(|c| keymap::builtin(c)).collect()
        }
    }

    fn fix_selection(&mut self) -> Result<()> {
        self.sel_valid = false;
        let Some(text) = self
            .clip
            .as_mut()
            .and_then(|c| c.read_primary())
            .filter(|t| !t.is_empty())
        else {
            debug!("⇧⇧: выделение пустое");
            return Ok(());
        };
        let length = text.chars().count();
        if length > self.cfg.max_selection {
            info!("выделение длиннее max_selection ({length}), пропускаю");
            return Ok(());
        }
        self.codes = self.backend.layouts()?;
        let current = self.backend.current()?;
        let tables = self.tables(&self.codes);
        let pair = keymap::detect_source(&text, &tables, current)
            .and_then(|src| keymap::next_with_table(src, &tables).map(|tgt| (src, tgt)));
        let Some((src, tgt)) = pair else {
            warn!(
                "нет таблиц для раскладок {:?} — выделение не исправить",
                self.codes
            );
            return Ok(());
        };
        let (Some(src_layout), Some(tgt_layout)) = (tables[src].clone(), tables[tgt].clone())
        else {
            return Ok(());
        };
        // CapsLock, переключающий раскладку, регистр не меняет.
        let caps = !self.keymap.caps_switches_group && (self.caps_on)();
        let plan = keymap::plan_strokes(&text, &src_layout, &tgt_layout, caps, tgt);
        // Наша вставка сама меняет PRIMARY и буфер обмена.
        self.sel_ignore_until = (self.clock)() + SELF_CHANGE_GRACE;

        let how = match &plan {
            Some(strokes) if self.cfg.selection_method == SelectionMethod::Type => {
                self.switch_to(tgt, current)?;
                self.keys.type_strokes(strokes)?; // набор поверх выделения заменяет его
                "набор"
            }
            _ => {
                if self.cfg.selection_method == SelectionMethod::Type {
                    info!(
                        "выделение не набрать клавишами (переводы строк или символы вне \
                         раскладки) — вставляю через буфер обмена"
                    );
                }
                let converted = keymap::convert_text(&text, &src_layout, &tgt_layout);
                if let Some(clip) = self.clip.as_mut() {
                    clip.paste(&converted, self.keys.as_mut())?;
                }
                self.switch_to(tgt, current)?;
                "вставка"
            }
        };
        match plan {
            Some(strokes) if strokes.len() <= SEGMENT_MAX => {
                self.buf = strokes; // повторный ⇧⇧ вернёт как было
                self.segment = true;
            }
            _ => self.reset("выделение заменено"),
        }
        self.layout = current.map(|_| tgt);
        self.layout_dirty = current.is_none();
        self.record(ActionKind::Selection, length, Some(src), Some(tgt));
        info!(
            "выделение: {length} символов, {} → {} ({how})",
            self.label(Some(src)),
            self.label(Some(tgt))
        );
        Ok(())
    }

    fn label(&self, index: Option<usize>) -> String {
        match index {
            Some(i) => self
                .codes
                .get(i)
                .cloned()
                .unwrap_or_else(|| format!("№{}", i + 1)),
            None => "?".into(),
        }
    }

    fn record(&mut self, kind: ActionKind, chars: usize, from: Option<usize>, to: Option<usize>) {
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

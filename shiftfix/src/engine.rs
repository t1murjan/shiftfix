//! Логика без ввода-вывода: события клавиатуры и мыши → решения.
//!
//! Буфер хранит нажатые клавиши (скан-коды + Shift), а не символы. Исправить
//! слово — значит переключить раскладку, стереть N символов и нажать те же
//! клавиши ещё раз; таблицы для этого не нужны.

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

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use super::*;

    const LS: KeyCode = KeyCode::KEY_LEFTSHIFT;

    /// Модель поля ввода: текст, курсор, выделение, текущая раскладка.
    struct World {
        layouts: Vec<String>,
        render: HashMap<String, &'static str>,
        cur: usize,
        text: Vec<char>,
        cursor: usize,
        sel: Option<(usize, usize)>,
        primary: Option<String>,
        clipboard: String,
        caps: bool,
        known_layout: bool,
        paste_keys: Vec<KeyCode>,
        log: Vec<String>,
    }

    impl World {
        fn text(&self) -> String {
            self.text.iter().collect()
        }

        fn set_text(&mut self, text: &str) {
            self.text = text.chars().collect();
            self.cursor = self.text.len();
            self.sel = None;
        }

        fn delete_selection(&mut self) {
            if let Some((a, b)) = self.sel.take() {
                self.text.drain(a..b);
                self.cursor = a;
            }
        }

        fn insert(&mut self, s: &str) {
            self.delete_selection();
            for ch in s.chars() {
                self.text.insert(self.cursor, ch);
                self.cursor += 1;
            }
        }

        fn backspace(&mut self) {
            if self.sel.is_some() {
                self.delete_selection();
            } else if self.cursor > 0 {
                self.cursor -= 1;
                self.text.remove(self.cursor);
            }
        }

        /// Приложение получило нажатие и печатает символ текущей раскладки.
        fn key(&mut self, key: KeyCode, shift: bool) {
            if key == KeyCode::KEY_BACKSPACE {
                return self.backspace();
            }
            let name = &self.layouts[self.cur];
            let layout = keymap::builtin(self.render.get(name).copied().unwrap_or(name)).unwrap();
            let letter = layout.char_at(key, false).unwrap().is_alphabetic();
            let ch = layout.char_at(key, shift ^ (self.caps && letter)).unwrap();
            self.insert(&ch.to_string());
        }
    }

    struct FakeKeys(Rc<RefCell<World>>);

    impl Keys for FakeKeys {
        fn backspace(&mut self, count: usize) -> Result<()> {
            let mut w = self.0.borrow_mut();
            w.log.push(format!("bs {count}"));
            (0..count).for_each(|_| w.backspace());
            Ok(())
        }
        fn type_strokes(&mut self, strokes: &[Stroke]) -> Result<()> {
            let mut w = self.0.borrow_mut();
            w.log.push(format!("type {}", strokes.len()));
            strokes.iter().for_each(|s| w.key(s.key, s.shift));
            Ok(())
        }
        fn combo(&mut self, keys: &[KeyCode]) -> Result<()> {
            let mut w = self.0.borrow_mut();
            w.log.push("combo".into());
            if keys == w.paste_keys.as_slice() {
                let clip = w.clipboard.clone();
                w.insert(&clip);
            }
            Ok(())
        }
    }

    struct FakeBackend(Rc<RefCell<World>>);

    impl Backend for FakeBackend {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn layouts(&mut self) -> Result<Vec<String>> {
            Ok(self.0.borrow().layouts.clone())
        }
        fn current(&mut self) -> Result<Option<usize>> {
            let w = self.0.borrow();
            Ok(w.known_layout.then_some(w.cur))
        }
        fn set(&mut self, index: usize, _keys: &mut dyn Keys) -> Result<()> {
            self.0.borrow_mut().cur = index;
            Ok(())
        }
        fn toggle(&mut self, _keys: &mut dyn Keys) -> Result<()> {
            let mut w = self.0.borrow_mut();
            w.cur = (w.cur + 1) % w.layouts.len();
            Ok(())
        }
    }

    struct FakeClip(Rc<RefCell<World>>);

    impl Selection for FakeClip {
        fn read_primary(&mut self) -> Option<String> {
            self.0.borrow().primary.clone()
        }
        fn paste(&mut self, text: &str, keys: &mut dyn Keys) -> Result<()> {
            let (old, paste_keys) = {
                let mut w = self.0.borrow_mut();
                (
                    std::mem::replace(&mut w.clipboard, text.to_owned()),
                    w.paste_keys.clone(),
                )
            };
            keys.combo(&paste_keys)?;
            self.0.borrow_mut().clipboard = old;
            Ok(())
        }
    }

    struct Rig {
        w: Rc<RefCell<World>>,
        eng: Engine,
        t: f64,
        clock: Rc<Cell<f64>>,
    }

    impl Rig {
        fn new() -> Rig {
            Rig::build(&["us", "ru"], SelectionMethod::Paste, true)
        }

        fn build(layouts: &[&str], method: SelectionMethod, known_layout: bool) -> Rig {
            let cfg = Config {
                switch_delay: std::time::Duration::ZERO,
                selection_method: method,
                ..Config::default()
            };
            let w = Rc::new(RefCell::new(World {
                layouts: layouts.iter().map(|s| s.to_string()).collect(),
                render: HashMap::from([("us2".to_string(), "us")]),
                cur: 0,
                text: Vec::new(),
                cursor: 0,
                sel: None,
                primary: None,
                clipboard: "OLD".into(),
                caps: false,
                known_layout,
                paste_keys: cfg.paste_keys.clone(),
                log: Vec::new(),
            }));
            let clock = Rc::new(Cell::new(100.0));
            let (cw, ck) = (w.clone(), clock.clone());
            let eng = Engine::new(
                cfg,
                Box::new(FakeBackend(w.clone())),
                Box::new(FakeKeys(w.clone())),
                Some(Box::new(FakeClip(w.clone()))),
                Box::new(move || cw.borrow().caps),
                Box::new(move || ck.get()),
                Box::new(|| None),
            );
            Rig {
                w,
                eng,
                t: 1000.0,
                clock,
            }
        }

        fn ev(&mut self, key: KeyCode, value: i32, dt: f64) {
            self.t += dt;
            self.eng.key(key, value, self.t);
        }

        /// Нажатие, которое доходит и до приложения.
        fn tap(&mut self, key: KeyCode, shift: bool) {
            if shift {
                self.ev(LS, 1, 0.04);
            }
            self.ev(key, 1, 0.04);
            self.w.borrow_mut().key(key, shift);
            self.ev(key, 0, 0.04);
            if shift {
                self.ev(LS, 0, 0.04);
            }
        }

        /// Клавиша без символа (стрелки, модификаторы): только события.
        fn press(&mut self, key: KeyCode) {
            self.ev(key, 1, 0.04);
            self.ev(key, 0, 0.04);
        }

        /// Пальцы печатают `text` так, будто включена раскладка `layout`.
        fn type_as(&mut self, text: &str, layout: &str) {
            for ch in text.chars() {
                let (key, shift) = keymap::builtin(layout).unwrap().key_for(ch).unwrap();
                self.tap(key, shift);
            }
        }

        fn type_text(&mut self, text: &str) {
            let name = {
                let w = self.w.borrow();
                let name = w.layouts[w.cur].clone();
                w.render.get(&name).map(|s| s.to_string()).unwrap_or(name)
            };
            self.type_as(text, &name);
        }

        fn shift2_timed(&mut self, hold: f64, gap: f64) {
            self.ev(LS, 1, 0.1);
            self.ev(LS, 0, hold);
            self.ev(LS, 1, gap);
            self.ev(LS, 0, hold);
        }

        fn shift2(&mut self) {
            self.shift2_timed(0.05, 0.1);
        }

        fn click(&mut self) {
            self.t += 0.1;
            self.eng.button(1, self.t);
            self.eng.button(0, self.t + 0.05);
        }

        /// Выделить всё мышью: клик, приложение публикует PRIMARY.
        fn select_all(&mut self) {
            self.click();
            {
                let mut w = self.w.borrow_mut();
                let len = w.text.len();
                w.sel = Some((0, len));
                w.cursor = len;
                w.primary = Some(w.text());
            }
            self.clock.set(self.clock.get() + 3.0);
            let size = self.w.borrow().primary.as_ref().map_or(0, |p| p.len());
            self.eng.primary_changed(size);
        }

        fn text(&self) -> String {
            self.w.borrow().text()
        }

        fn layout(&self) -> usize {
            self.w.borrow().cur
        }

        fn actions(&self) -> usize {
            self.w.borrow().log.len()
        }
    }

    /// Встроенная таблица, но с мёртвыми клавишами (как us(intl) или de).
    fn with_dead(name: &str, dead: &[(KeyCode, bool)]) -> Arc<Layout> {
        let base = keymap::builtin(name).unwrap();
        let pairs = keymap::ROW_KEYS
            .iter()
            .map(|&k| (k, [base.char_at(k, false), base.char_at(k, true)]));
        Arc::new(Layout::from_pairs(name, pairs).with_dead(dead.iter().copied()))
    }

    #[test]
    fn layout_info_names_backend_after_asking_it() {
        /// Как Auto: переключатель выбирается при первом обращении.
        struct Lazy(bool);
        impl Backend for Lazy {
            fn name(&self) -> &'static str {
                if self.0 { "KDE" } else { "хоткей" }
            }
            fn layouts(&mut self) -> Result<Vec<String>> {
                self.0 = true;
                Ok(vec!["us".into(), "ru".into()])
            }
            fn current(&mut self) -> Result<Option<usize>> {
                Ok(Some(0))
            }
            fn set(&mut self, _: usize, _: &mut dyn Keys) -> Result<()> {
                Ok(())
            }
            fn toggle(&mut self, _: &mut dyn Keys) -> Result<()> {
                Ok(())
            }
        }
        let mut r = Rig::new();
        r.eng.set_backend(Box::new(Lazy(false)));
        assert_eq!(r.eng.layout_info().backend, "KDE");
    }

    #[test]
    fn layout_info_is_cached_until_switch() {
        let mut r = Rig::new();
        assert_eq!(r.eng.layout_info().current, Some(0));
        // Раскладку переключили снаружи — GUI узнает об этом не сразу…
        r.w.borrow_mut().cur = 1;
        assert_eq!(r.eng.layout_info().current, Some(0));
        r.clock.set(r.clock.get() + LAYOUT_INFO_TTL);
        assert_eq!(r.eng.layout_info().current, Some(1));
        // …а после своего переключения — сразу.
        r.type_as("ghbdtn", "us");
        r.shift2();
        assert_eq!(r.eng.layout_info().current, Some(0));
    }

    #[test]
    fn dead_key_in_target_layout_blocks_fix() {
        let mut r = Rig::new();
        r.eng.set_keymap(Keymap {
            layouts: vec![
                with_dead("us", &[(KeyCode::KEY_APOSTROPHE, false)]),
                keymap::builtin("ru").unwrap(),
            ],
            caps_switches_group: false,
        });
        // Включена ru, а пальцы набрали «it's».
        r.w.borrow_mut().cur = 1;
        r.type_as("it's", "us");
        assert_eq!(r.text(), "шеэы");
        let before = r.actions();
        r.shift2();
        // В us(intl) апостроф мёртвый: «itś» и Backspace×4 на повторном ⇧⇧ — не надо.
        assert_eq!((r.text().as_str(), r.layout()), ("шеэы", 1));
        assert_eq!(r.actions(), before, "ни переключения, ни нажатий");

        // Слово без мёртвых клавиш исправляется как обычно.
        r.tap(KeyCode::KEY_SPACE, false);
        r.type_as("ok", "us");
        r.shift2();
        assert_eq!((r.text().as_str(), r.layout()), ("шеэы ok", 0));
    }

    #[test]
    fn fixes_last_word_and_toggles_back() {
        let mut r = Rig::new();
        r.type_as("привет", "ru");
        assert_eq!(r.text(), "ghbdtn");
        r.shift2();
        assert_eq!((r.text().as_str(), r.layout()), ("привет", 1));
        r.shift2();
        assert_eq!((r.text().as_str(), r.layout()), ("ghbdtn", 0));
    }

    #[test]
    fn keeps_capitals_and_trailing_space() {
        let mut r = Rig::new();
        r.type_as("Привет ", "ru");
        assert_eq!(r.text(), "Ghbdtn ");
        r.shift2();
        assert_eq!(r.text(), "Привет ");
    }

    #[test]
    fn only_last_word_with_punctuation() {
        let mut r = Rig::new();
        r.type_as("привет как дела?", "ru");
        assert_eq!(r.text(), "ghbdtn rfr ltkf&");
        r.shift2();
        assert_eq!(r.text(), "ghbdtn rfr дела?");
    }

    #[test]
    fn follows_backspace() {
        let mut r = Rig::new();
        r.type_text("ghbdtx");
        r.tap(KeyCode::KEY_BACKSPACE, false);
        r.type_text("n");
        r.shift2();
        assert_eq!(r.text(), "привет");
    }

    #[test]
    fn continues_in_new_layout() {
        let mut r = Rig::new();
        r.type_text("ghbdtn");
        r.shift2();
        r.type_text(" мир");
        assert_eq!(r.text(), "привет мир");
        r.shift2();
        assert_eq!((r.text().as_str(), r.layout()), ("привет vbh", 0));
    }

    #[test]
    fn uses_layout_of_typing_not_current() {
        let mut r = Rig::new();
        r.type_text("ghbdtn");
        // Пользователь сам переключил раскладку (Alt+Shift) уже после набора.
        r.ev(KeyCode::KEY_LEFTALT, 1, 0.1);
        r.ev(LS, 1, 0.05);
        r.w.borrow_mut().cur = 1;
        r.ev(LS, 0, 0.05);
        r.ev(KeyCode::KEY_LEFTALT, 0, 0.05);
        r.shift2();
        assert_eq!((r.text().as_str(), r.layout()), ("привет", 1));
    }

    #[test]
    fn tap_timing() {
        let mut r = Rig::new();
        r.type_text("ghbdtn");
        r.shift2_timed(0.5, 0.1); // держали слишком долго
        r.shift2_timed(0.05, 0.6); // слишком медленно
        assert_eq!(r.text(), "ghbdtn");
        // Shift, буква, Shift — не двойное нажатие.
        r.ev(LS, 1, 0.5);
        r.ev(LS, 0, 0.05);
        r.type_text("x");
        r.ev(LS, 1, 0.05);
        r.ev(LS, 0, 0.05);
        assert_eq!(r.text(), "ghbdtnx");
        // Левый, затем правый — считается.
        r.ev(LS, 1, 0.5);
        r.ev(LS, 0, 0.05);
        r.ev(KeyCode::KEY_RIGHTSHIFT, 1, 0.1);
        r.ev(KeyCode::KEY_RIGHTSHIFT, 0, 0.05);
        assert_eq!(r.text(), "приветч");
    }

    #[test]
    fn shift_as_modifier_is_not_a_tap() {
        let mut r = Rig::new();
        r.type_text("ghbdtn");
        r.ev(LS, 1, 0.1);
        r.ev(LS, 0, 0.05);
        r.tap(KeyCode::KEY_G, true); // Shift+G — заглавная, а не второй тап
        assert_eq!(r.text(), "ghbdtnG");
        r.shift2();
        assert_eq!(r.text(), "приветП");
    }

    #[test]
    fn resync_uses_keys_held_by_kernel() {
        let mut r = Rig::new();
        r.eng.resync(HashSet::from([KeyCode::KEY_LEFTCTRL])); // Ctrl зажат, пока события терялись
        r.type_text("x"); // значит, это Ctrl+X, а не буква
        r.shift2();
        assert_eq!(r.actions(), 0);
    }

    #[test]
    fn cursor_moves_and_shortcuts_reset_buffer() {
        for action in [0, 1, 2, 3, 4] {
            let mut r = Rig::new();
            r.type_text("ghbdtn");
            match action {
                0 => r.click(),
                1 => r.press(KeyCode::KEY_LEFT),
                2 => {
                    r.ev(KeyCode::KEY_LEFTCTRL, 1, 0.05);
                    r.press(KeyCode::KEY_Z);
                    r.ev(KeyCode::KEY_LEFTCTRL, 0, 0.05);
                }
                3 => r.press(KeyCode::KEY_LEFTMETA), // лаунчер
                _ => {
                    r.ev(KeyCode::KEY_BACKSPACE, 1, 0.05);
                    r.ev(KeyCode::KEY_BACKSPACE, 2, 0.5); // автоповтор
                    r.ev(KeyCode::KEY_BACKSPACE, 0, 0.05);
                }
            }
            r.shift2();
            assert_eq!(r.actions(), 0, "действие {action}");
        }
    }

    #[test]
    fn lone_ctrl_and_capslock_keep_buffer() {
        let mut r = Rig::new();
        r.type_text("ghbdtn");
        r.press(KeyCode::KEY_LEFTCTRL);
        r.press(KeyCode::KEY_CAPSLOCK);
        r.press(KeyCode::KEY_VOLUMEUP);
        r.shift2();
        assert_eq!(r.text(), "привет");
    }

    #[test]
    fn touchpad_tap_is_a_click_but_drag_is_not() {
        let mut r = Rig::new();
        r.type_text("ghbdtn");
        r.eng.touch("pad", 1, r.t + 0.1);
        r.eng.touch("pad", 0, r.t + 1.5); // долгое касание — просто движение
        r.shift2();
        assert_eq!(r.text(), "привет");
        r.eng.touch("pad", 1, r.t + 0.1);
        r.eng.touch("pad", 0, r.t + 0.2); // тап
        r.shift2();
        assert_eq!(r.text(), "привет");
    }

    #[test]
    fn ignores_long_words_and_spaces_only() {
        let mut r = Rig::new();
        r.type_text(&"w".repeat(60));
        r.shift2();
        r.click();
        r.type_text("   ");
        r.shift2();
        assert_eq!(r.actions(), 0);
    }

    #[test]
    fn selection_is_pasted_and_cycles_back() {
        let mut r = Rig::new();
        r.w.borrow_mut().set_text("Ghbdtn vbh");
        r.select_all();
        r.shift2();
        assert_eq!((r.text().as_str(), r.layout()), ("Привет мир", 1));
        assert_eq!(r.w.borrow().clipboard, "OLD");
        r.shift2();
        assert_eq!((r.text().as_str(), r.layout()), ("Ghbdtn vbh", 0));
    }

    #[test]
    fn selection_mixed_and_reverse() {
        let mut r = Rig::new();
        r.w.borrow_mut().set_text("ghbdtn мир");
        r.select_all();
        r.shift2();
        assert_eq!(r.text(), "привет мир");

        let mut r = Rig::new();
        r.w.borrow_mut().cur = 1;
        r.w.borrow_mut().set_text("руддщ цщкдв");
        r.select_all();
        r.shift2();
        assert_eq!((r.text().as_str(), r.layout()), ("hello world", 0));
    }

    #[test]
    fn multiline_selection_is_pasted_without_cycle() {
        let mut r = Rig::new();
        r.w.borrow_mut().set_text("ghbdtn\nvbh");
        r.select_all();
        r.shift2();
        assert_eq!(r.text(), "привет\nмир");
        let before = r.actions();
        r.shift2();
        assert_eq!(r.actions(), before);
    }

    #[test]
    fn selection_by_typing_handles_capslock() {
        let mut r = Rig::build(&["us", "ru"], SelectionMethod::Type, true);
        r.w.borrow_mut().caps = true;
        r.w.borrow_mut().set_text("Ghbdtn");
        r.select_all();
        r.shift2();
        assert_eq!(r.text(), "Привет");
        assert!(!r.w.borrow().log.contains(&"combo".to_string()));
    }

    #[test]
    fn stale_primary_after_click_is_ignored() {
        let mut r = Rig::new();
        r.w.borrow_mut().set_text("ghbdtn");
        r.select_all();
        r.click(); // выделение снято, но PRIMARY ещё хранит старый текст
        r.w.borrow_mut().sel = None;
        r.shift2();
        assert_eq!((r.text().as_str(), r.actions()), ("ghbdtn", 0));
    }

    #[test]
    fn own_paste_does_not_revalidate_selection() {
        let mut r = Rig::new();
        r.w.borrow_mut().set_text("ghbdtn");
        r.select_all();
        r.shift2();
        r.eng.primary_changed(6); // эхо нашей же вставки
        r.shift2(); // значит, это повтор по буферу, а не новое выделение
        assert_eq!(r.text(), "ghbdtn");
    }

    #[test]
    fn typing_over_selection_switches_to_word_mode() {
        let mut r = Rig::new();
        r.w.borrow_mut().set_text("abc");
        r.select_all();
        r.type_text("x");
        r.shift2();
        assert_eq!(r.text(), "ч");
    }

    #[test]
    fn three_layouts_cycle() {
        let mut r = Rig::build(&["us", "ru", "us2"], SelectionMethod::Paste, true);
        r.type_text("ghbdtn");
        r.shift2();
        assert_eq!((r.text().as_str(), r.layout()), ("привет", 1));
        r.shift2();
        assert_eq!((r.text().as_str(), r.layout()), ("ghbdtn", 2));
        r.shift2();
        assert_eq!((r.text().as_str(), r.layout()), ("ghbdtn", 0));
    }

    #[test]
    fn unknown_layout_just_toggles() {
        let mut r = Rig::build(&["us", "ru"], SelectionMethod::Paste, false);
        r.type_text("ghbdtn");
        r.shift2();
        assert_eq!(r.text(), "привет");
        r.shift2();
        assert_eq!(r.text(), "ghbdtn");
    }

    #[test]
    fn paused_engine_does_nothing() {
        let mut r = Rig::new();
        r.eng.set_enabled(false);
        r.type_text("ghbdtn");
        r.shift2();
        assert_eq!(r.actions(), 0);
        r.eng.set_enabled(true);
        r.type_text(" ntcn");
        r.shift2();
        assert_eq!(r.text(), "ghbdtn тест");
    }
}

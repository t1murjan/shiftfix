//! Логика без ввода-вывода: события клавиатуры и мыши → решения.
//!
//! Буфер хранит нажатые клавиши (скан-коды + Shift), а не символы. Исправить
//! слово — значит переключить раскладку, стереть N символов и нажать те же
//! клавиши ещё раз; таблицы для этого не нужны.
//!
//! Здесь — разбор событий и исправление, остальное — рядом:
//!
//! - `ports.rs` — через что движок нажимает клавиши, переключает раскладку,
//!   читает выделение и спрашивает службу (`Hooks`);
//! - `taps.rs` — двойной Shift и одиночные Ctrl/Alt/Meta;
//! - `buffer.rs` — буфер нажатий и последнее слово в нём;
//! - `report.rs` — что движок сообщает окну;
//! - `tests.rs` — модель поля ввода и тесты на ней.

mod buffer;
mod ports;
mod report;
mod taps;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::thread;

use anyhow::{Result, bail};
use evdev::KeyCode;
use log::{debug, error, info, warn};

pub use ports::{Backend, Hooks, Keys, Selection};
pub use report::{Action, ActionKind, LayoutInfo};

use crate::config::{Config, SelectionMethod};
use crate::keymap::{self, Keymap, Layout};
use crate::keys::{Stroke, is_char, is_ctrl, is_modifier, is_nav, is_neutral, is_shift};
use buffer::Buffer;
use taps::Taps;

/// Вставку длиннее нельзя «прокрутить» повторным ⇧⇧.
pub const SEGMENT_MAX: usize = 200;
/// Касание тачпада короче этого считаем кликом (тап).
const TOUCH_TAP_MAX: f64 = 0.3;
/// Столько секунд после нашей вставки события PRIMARY не учитываются.
const SELF_CHANGE_GRACE: f64 = 2.0;

#[derive(Clone, Copy)]
struct Held {
    any: bool,
    shift: bool,
    modifier: bool,
}

pub struct Engine {
    cfg: Config,
    backend: Box<dyn Backend>,
    keys: Box<dyn Keys>,
    selection: Option<Box<dyn Selection>>,
    hooks: Hooks,
    keymap: Keymap,
    enabled: bool,
    buf: Buffer,
    /// Зажатые физические клавиши.
    down: HashSet<KeyCode>,
    /// ⇧⇧ и одиночные Ctrl/Alt/Meta.
    taps: Taps,
    touch_down: HashMap<String, f64>,
    /// В PRIMARY актуальное выделение.
    selection_valid: bool,
    /// До этого времени события PRIMARY — эхо нашей вставки.
    selection_ignore_until: f64,
    /// Раскладка, в которой сейчас набираются буквы.
    layout: Option<usize>,
    layout_dirty: bool,
    /// Коды раскладок с последнего обращения к переключателю (для журнала и
    /// сверки с keymap); пусто — список неизвестен.
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
        selection: Option<Box<dyn Selection>>,
        hooks: Hooks,
    ) -> Self {
        Engine {
            cfg,
            backend,
            keys,
            selection,
            hooks,
            keymap: Keymap::default(),
            enabled: true,
            buf: Buffer::default(),
            down: HashSet::new(),
            taps: Taps::default(),
            touch_down: HashMap::new(),
            selection_valid: false,
            selection_ignore_until: 0.0,
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
        self.selection.is_some()
    }

    pub fn set_selection(&mut self, selection: Option<Box<dyn Selection>>) {
        self.selection = selection;
        self.selection_valid = false;
    }

    pub fn set_config(&mut self, cfg: Config) {
        self.keys.configure(&cfg);
        self.cfg = cfg;
    }

    pub fn set_backend(&mut self, backend: Box<dyn Backend>) {
        self.backend = backend;
        self.codes.clear(); // список прежнего переключателя
        self.info_cache = None;
        self.layout_dirty = true;
        self.reset("сменился переключатель раскладки");
    }

    pub fn set_keymap(&mut self, keymap: Keymap) {
        self.keymap = keymap;
        self.info_cache = None;
    }

    // Входящие события.

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
        self.taps.clear();
        self.layout_dirty = true;
        if self.enabled {
            self.selection_valid = false; // клик снимает выделение; новое придёт событием PRIMARY
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
        if self.enabled && (self.hooks.clock)() >= self.selection_ignore_until {
            self.selection_valid = size > 0;
        }
    }

    /// После потери событий: зажатые клавиши берутся у ядра, буфер забывается.
    pub fn resync(&mut self, held: HashSet<KeyCode>) {
        self.down = held;
        self.taps.clear();
        self.layout_dirty = true;
        self.reset("пересинхронизация");
    }

    pub fn set_enabled(&mut self, flag: bool) {
        self.enabled = flag;
        self.selection_valid = false;
        self.taps.clear();
        self.layout_dirty = true;
        self.reset(if flag {
            "снова работаю"
        } else {
            "пауза"
        });
    }

    // Разбор нажатий.

    fn press(&mut self, key: KeyCode, held: Held, ts: f64) {
        self.taps.pressed(key, !held.any, ts);
        if is_shift(key) {
            if held.any {
                // Alt+Shift, Ctrl+Shift, Shift+Shift — возможно, смена раскладки.
                self.layout_dirty = true;
            }
            return;
        }
        if is_modifier(key) {
            self.layout_dirty = true;
            return;
        }
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
                self.selection_valid = false;
            }
            self.reset("сочетание клавиш");
            self.layout_dirty = true;
            return;
        }
        if is_char(key) {
            self.typed(key, held.shift);
        } else if key == KeyCode::KEY_BACKSPACE {
            self.selection_valid = false;
            self.buf.pop();
        } else {
            // Стрелки, Home/End, Enter, Tab, Esc, Delete, F1…
            if !(held.shift && is_nav(key)) {
                self.selection_valid = false; // Shift+стрелки выделение только расширяют
            }
            self.reset("курсор сдвинулся");
            self.layout_dirty = true;
        }
    }

    fn release(&mut self, key: KeyCode, ts: f64) {
        if is_shift(key) {
            if self.taps.shift_released(ts, &self.cfg) {
                self.trigger();
            }
        } else if self.taps.lone_mod_released(key) && !is_ctrl(key) {
            // Одиночный Alt открывает меню, Meta — лаунчер: фокус ушёл.
            self.selection_valid = false;
            self.reset("одиночный Alt/Meta");
        }
    }

    fn typed(&mut self, key: KeyCode, shift: bool) {
        self.selection_valid = false;
        if key != KeyCode::KEY_SPACE && (self.layout_dirty || self.buf.at_word_start()) {
            self.layout = self.current_layout();
            self.layout_dirty = false;
        }
        // Мёртвая клавиша (´, ¨ в раскладках с акцентами) печатает не символ, а
        // часть следующего: сколько символов стирать — уже не узнать. Пока
        // список переключателя неизвестен, сверить keymap не с чем — она как есть.
        let layouts = if self.codes.is_empty() {
            Some(self.keymap.layouts.as_slice())
        } else {
            self.keymap_layouts(&self.codes)
        };
        let dead = self
            .layout
            .and_then(|i| layouts?.get(i))
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
    }

    fn reset(&mut self, why: &str) {
        if !self.buf.is_empty() {
            debug!("буфер сброшен: {why}");
        }
        self.buf.clear();
    }

    fn current_layout(&mut self) -> Option<usize> {
        self.backend.current().unwrap_or_else(|err| {
            debug!("текущая раскладка неизвестна: {err:#}");
            None
        })
    }

    // Исправление.

    pub fn trigger(&mut self) {
        if let Some(reason) = (self.hooks.guard)() {
            info!("⇧⇧ пропущен: {reason}");
            self.selection_valid = false;
            self.reset("исправлять нельзя");
            return;
        }
        let result = if self.selection_valid && self.selection.is_some() {
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
        let undo = self.buf.is_segment();
        let start = match self.buf.fix_start(self.cfg.max_word_len) {
            Ok(start) => start,
            Err(why) => {
                debug!("⇧⇧: {why}");
                return Ok(());
            }
        };
        // Слово вместе с пробелами после него.
        let count = self.buf.strokes().len() - start;
        let source = self.buf.strokes()[start].layout;
        let target = self.next_layout(source)?;
        if let Some(t) = target
            && self.has_dead_key(t, &self.buf.strokes()[start..])
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
        self.keys.type_strokes(&self.buf.strokes()[start..])?;
        self.buf.retyped(start, target);
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

    /// Переключить с `current` на `target`: текущая неизвестна — просто на
    /// следующую, `target` уже включена — ничего не делать.
    fn switch_from(&mut self, current: Option<usize>, target: usize) -> Result<()> {
        if current == Some(target) {
            return Ok(());
        }
        self.switch(current.map(|_| target))
    }

    /// Есть ли среди `strokes` мёртвая клавиша раскладки `layout`.
    fn has_dead_key(&self, layout: usize, strokes: &[Stroke]) -> bool {
        self.keymap_layouts(&self.codes)
            .and_then(|l| l.get(layout))
            .is_some_and(|l| strokes.iter().any(|s| l.is_dead(s.key, s.shift)))
    }

    /// Раскладки keymap, если их столько же, сколько у переключателя (`codes`).
    /// Иначе один и тот же номер в keymap и у переключателя может означать
    /// разные раскладки.
    fn keymap_layouts(&self, codes: &[String]) -> Option<&[Arc<Layout>]> {
        let layouts = &self.keymap.layouts;
        (!layouts.is_empty() && layouts.len() == codes.len()).then_some(layouts.as_slice())
    }

    /// Таблицы по индексам раскладок: из keymap, если она совпадает со списком
    /// переключателя, иначе встроенные по кодам.
    fn tables(&self, codes: &[String]) -> Vec<Option<Arc<Layout>>> {
        match self.keymap_layouts(codes) {
            Some(layouts) => layouts.iter().cloned().map(Some).collect(),
            None => codes.iter().map(|c| keymap::builtin(c)).collect(),
        }
    }

    fn fix_selection(&mut self) -> Result<()> {
        self.selection_valid = false;
        let Some(text) = self
            .selection
            .as_mut()
            .and_then(|s| s.read_primary())
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
        let caps = !self.keymap.caps_switches_group && (self.hooks.caps_on)();
        let plan = keymap::plan_strokes(&text, &src_layout, &tgt_layout, caps, tgt);
        // Наша вставка сама меняет PRIMARY и буфер обмена. Отсчёт — и до неё:
        // если она оборвётся ошибкой, второй установки ниже не будет.
        self.selection_ignore_until = (self.hooks.clock)() + SELF_CHANGE_GRACE;

        let how = match &plan {
            Some(strokes) if self.cfg.selection_method == SelectionMethod::Type => {
                self.switch_from(current, tgt)?;
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
                if let Some(selection) = self.selection.as_mut() {
                    selection.paste(&converted, self.keys.as_mut())?;
                }
                self.switch_from(current, tgt)?;
                "вставка"
            }
        };
        // Вставка и набор бывают долгими (до нескольких секунд), а события
        // PRIMARY за это время движок разберёт только теперь: отсчёт заново.
        self.selection_ignore_until = (self.hooks.clock)() + SELF_CHANGE_GRACE;
        match plan {
            Some(strokes) if strokes.len() <= SEGMENT_MAX => {
                self.buf.set_segment(strokes); // повторный ⇧⇧ вернёт как было
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
}

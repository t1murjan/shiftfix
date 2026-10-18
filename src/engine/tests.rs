use std::cell::{Cell, RefCell, RefMut};
use std::rc::Rc;

use super::buffer::BUF_MAX;
use super::report::LAYOUT_INFO_TTL;
use super::*;
use crate::keys::ROW_KEYS;

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
    /// Переключатель раскладки не отвечает.
    backend_down: bool,
    /// Сеанс неактивен: вошёл другой пользователь, открыта консоль.
    inactive: bool,
    /// Сколько секунд идёт вставка через буфер обмена.
    paste_secs: f64,
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
    fn chord(&mut self, _: &[KeyCode], _: KeyCode, _: usize) -> Result<()> {
        self.0.borrow_mut().log.push("chord".into());
        Ok(())
    }
}

struct FakeBackend(Rc<RefCell<World>>);

impl FakeBackend {
    fn world(&self) -> Result<RefMut<'_, World>> {
        let w = self.0.borrow_mut();
        if w.backend_down {
            bail!("переключатель не отвечает");
        }
        Ok(w)
    }
}

impl Backend for FakeBackend {
    fn name(&self) -> &'static str {
        "fake"
    }
    fn layouts(&mut self) -> Result<Vec<String>> {
        Ok(self.world()?.layouts.clone())
    }
    fn current(&mut self) -> Result<Option<usize>> {
        let w = self.world()?;
        Ok(w.known_layout.then_some(w.cur))
    }
    fn set(&mut self, index: usize, _keys: &mut dyn Keys) -> Result<()> {
        self.world()?.cur = index;
        Ok(())
    }
    fn toggle(&mut self, _keys: &mut dyn Keys) -> Result<()> {
        let mut w = self.world()?;
        w.cur = (w.cur + 1) % w.layouts.len();
        Ok(())
    }
}

struct FakeSelection(Rc<RefCell<World>>, Rc<Cell<f64>>);

impl Selection for FakeSelection {
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
        let mut w = self.0.borrow_mut();
        w.clipboard = old;
        self.1.set(self.1.get() + w.paste_secs);
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
            backend_down: false,
            inactive: false,
            paste_secs: 0.0,
            paste_keys: cfg.paste_keys.clone(),
            log: Vec::new(),
        }));
        let clock = Rc::new(Cell::new(100.0));
        let (cw, ck, gw) = (w.clone(), clock.clone(), w.clone());
        let eng = Engine::new(
            cfg,
            Box::new(FakeBackend(w.clone())),
            Box::new(FakeKeys(w.clone())),
            Some(Box::new(FakeSelection(w.clone(), clock.clone()))),
            Hooks {
                caps_on: Box::new(move || cw.borrow().caps),
                clock: Box::new(move || ck.get()),
                guard: Box::new(move || gw.borrow().inactive.then(|| "сеанс неактивен".into())),
            },
        );
        let mut r = Rig {
            w,
            eng,
            t: 1000.0,
            clock,
        };
        // Как служба при запуске: список раскладок известен до первого нажатия.
        r.eng.layout_info();
        r
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

    /// Сочетание: зажать `hold`, нажать `key`, отпустить в обратном порядке.
    fn press_with(&mut self, hold: &[KeyCode], key: KeyCode) {
        hold.iter().for_each(|&k| self.ev(k, 1, 0.04));
        self.press(key);
        hold.iter().rev().for_each(|&k| self.ev(k, 0, 0.04));
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

    /// В поле `text`, его выделили мышью: клик, приложение публикует PRIMARY.
    fn select_text(&mut self, text: &str) {
        self.w.borrow_mut().set_text(text);
        self.click();
        {
            let mut w = self.w.borrow_mut();
            let len = w.text.len();
            w.sel = Some((0, len));
            w.cursor = len;
            w.primary = Some(w.text());
        }
        // Выделяют не сразу после прошлой вставки: это не её эхо.
        self.clock.set(self.clock.get() + SELF_CHANGE_GRACE + 1.0);
        self.eng.primary_changed(text.len());
    }

    fn text(&self) -> String {
        self.w.borrow().text()
    }

    fn layout(&self) -> usize {
        self.w.borrow().cur
    }

    /// Сколько раз движок что-то нажал: Backspace, набор, сочетание.
    fn emitted(&self) -> usize {
        self.w.borrow().log.len()
    }

    /// В поле `text`, включена раскладка `layout`.
    #[track_caller]
    fn assert_shows(&self, text: &str, layout: usize) {
        assert_eq!((self.text().as_str(), self.layout()), (text, layout));
    }
}

/// Встроенная таблица, но с мёртвыми клавишами (как us(intl) или de).
fn with_dead(name: &str, dead: &[(KeyCode, bool)]) -> Arc<Layout> {
    let base = keymap::builtin(name).unwrap();
    let pairs = ROW_KEYS
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
fn layout_info_reports_backend_error() {
    let mut r = Rig::new();
    r.w.borrow_mut().backend_down = true;
    r.clock.set(r.clock.get() + LAYOUT_INFO_TTL);
    let info = r.eng.layout_info();
    assert_eq!(info.error.as_deref(), Some("переключатель не отвечает"));
    assert!(info.codes.is_empty() && info.current.is_none());
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
    let before = r.emitted();
    r.shift2();
    // В us(intl) апостроф мёртвый: «itś» и Backspace×4 на повторном ⇧⇧ — не надо.
    r.assert_shows("шеэы", 1);
    assert_eq!(r.emitted(), before, "ни переключения, ни нажатий");

    // Слово без мёртвых клавиш исправляется как обычно.
    r.tap(KeyCode::KEY_SPACE, false);
    r.type_as("ok", "us");
    r.shift2();
    r.assert_shows("шеэы ok", 0);
}

#[test]
fn keymap_for_other_layout_list_is_ignored() {
    // В keymap две раскладки, у переключателя три: номера не совпадают, и
    // мёртвый апостроф us(intl) под №1 в keymap — не про ru под №1.
    let mut r = Rig::build(&["us", "ru", "us2"], SelectionMethod::Paste, true);
    r.eng.set_keymap(Keymap {
        layouts: vec![
            keymap::builtin("ru").unwrap(),
            with_dead("us", &[(KeyCode::KEY_APOSTROPHE, false)]),
        ],
        caps_switches_group: false,
    });
    // Исправление на ru: «э» на клавише апострофа.
    r.type_as("это", "ru");
    r.shift2();
    r.assert_shows("это", 1);
    // Набор в ru.
    r.type_text(" это");
    r.shift2();
    r.assert_shows("это 'nj", 2);
}

#[test]
fn dead_key_while_typing_forgets_word() {
    let mut r = Rig::new();
    r.eng.set_keymap(Keymap {
        layouts: vec![
            with_dead("us", &[(KeyCode::KEY_APOSTROPHE, false)]),
            keymap::builtin("ru").unwrap(),
        ],
        caps_switches_group: false,
    });
    // В us(intl) апостроф мёртвый: с ним и следующей буквой приложение
    // напечатает один символ — сколько стирать, уже не узнать.
    r.type_text("it'");
    r.shift2();
    r.assert_shows("it'", 0);
    assert_eq!(r.emitted(), 0);
}

#[test]
fn dead_key_while_typing_without_layout_list() {
    // Список раскладок переключателя неизвестен: сверить keymap не с чем, и
    // мёртвые клавиши берутся из неё как есть.
    let keymap = Keymap {
        layouts: vec![
            with_dead("us", &[(KeyCode::KEY_APOSTROPHE, false)]),
            keymap::builtin("ru").unwrap(),
        ],
        caps_switches_group: false,
    };
    // Служба запустилась раньше переключателя: на её вопрос он не ответил.
    let mut r = Rig::new();
    r.eng.codes.clear();
    r.eng.set_keymap(keymap.clone());
    r.type_text("it'");
    r.shift2();
    r.assert_shows("it'", 0);
    assert_eq!(r.emitted(), 0);

    // Сменился переключатель: прежний список из трёх раскладок — уже не его.
    let mut r = Rig::build(&["us", "ru", "us2"], SelectionMethod::Paste, true);
    r.eng.set_keymap(keymap);
    r.w.borrow_mut().layouts.pop();
    r.eng.set_backend(Box::new(FakeBackend(r.w.clone())));
    r.type_text("it'");
    r.shift2();
    r.assert_shows("it'", 0);
    assert_eq!(r.emitted(), 0);
}

#[test]
fn fixes_last_word_and_toggles_back() {
    let mut r = Rig::new();
    r.type_as("привет", "ru");
    assert_eq!(r.text(), "ghbdtn");
    r.shift2();
    r.assert_shows("привет", 1);
    r.shift2();
    r.assert_shows("ghbdtn", 0);
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
    r.assert_shows("привет vbh", 0);
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
    r.assert_shows("привет", 1);
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
    assert_eq!(r.emitted(), 0);
}

#[test]
fn inactive_session_blocks_fix_and_forgets() {
    let mut r = Rig::new();
    r.type_text("ghbdtn");
    r.w.borrow_mut().inactive = true;
    r.shift2();
    r.assert_shows("ghbdtn", 0);
    assert_eq!(r.emitted(), 0, "ни Backspace, ни букв в чужой сеанс");
    // Сеанс вернулся, но слово набиралось до ухода — его уже не исправить.
    r.w.borrow_mut().inactive = false;
    r.shift2();
    assert_eq!(r.emitted(), 0);

    // Выделение тоже забывается.
    r.select_text("ghbdtn");
    r.w.borrow_mut().inactive = true;
    r.shift2();
    r.w.borrow_mut().inactive = false;
    r.shift2();
    assert_eq!((r.text().as_str(), r.emitted()), ("ghbdtn", 0));
}

#[test]
fn backend_error_forgets_word_and_types_nothing() {
    for layouts in [&["us"][..], &["us", "ru"]] {
        let mut r = Rig::build(layouts, SelectionMethod::Paste, true);
        r.type_text("ghbdtn");
        // Одна раскладка — переключать некуда; две — переключатель не отвечает.
        r.w.borrow_mut().backend_down = layouts.len() == 2;
        r.shift2();
        r.assert_shows("ghbdtn", 0);
        assert_eq!(r.emitted(), 0, "{layouts:?}");
        // Всё починилось, но слово уже забыто.
        {
            let mut w = r.w.borrow_mut();
            w.backend_down = false;
            w.layouts = vec!["us".into(), "ru".into()];
        }
        r.shift2();
        assert_eq!(
            (r.text().as_str(), r.emitted()),
            ("ghbdtn", 0),
            "{layouts:?}"
        );
    }
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
        assert_eq!(r.emitted(), 0, "действие {action}");
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
    r.type_text(&"w".repeat(r.eng.cfg.max_word_len + 1));
    r.shift2();
    r.click();
    r.type_text("   ");
    r.shift2();
    assert_eq!(r.emitted(), 0);
}

#[test]
fn word_cut_by_buffer_limit_is_not_fixed() {
    // Начало слова вытеснено из буфера: исправился бы только хвост. Backspace
    // буфер укорачивает, но начала слова не возвращает.
    for backspace in [false, true] {
        // Длинное слово, за ним столько пробелов, что хвост короче max_word_len.
        let mut r = Rig::new();
        r.type_text(&"g".repeat(r.eng.cfg.max_word_len + 10));
        r.type_text(&" ".repeat(BUF_MAX - 20));
        if backspace {
            r.tap(KeyCode::KEY_BACKSPACE, false);
        }
        r.shift2();
        assert_eq!(r.emitted(), 0, "Backspace: {backspace}");
        // После сброса буфера слово с его начала — целое.
        r.click();
        r.type_text("ghbdtn");
        r.shift2();
        assert!(r.text().ends_with(" привет"), "{}", r.text());

        // Слово длиннее буфера — при обычном max_word_len и при max_word_len
        // больше BUF_MAX.
        for max in [Config::default().max_word_len, BUF_MAX * 2] {
            let mut r = Rig::new();
            r.eng.cfg.max_word_len = max;
            r.type_text(&"g".repeat(BUF_MAX + 10));
            if backspace {
                r.tap(KeyCode::KEY_BACKSPACE, false);
            }
            r.shift2();
            assert_eq!(
                r.emitted(),
                0,
                "max_word_len = {max}, Backspace: {backspace}"
            );
            // Следующее слово — целое.
            r.type_text(" ghbdtn");
            r.shift2();
            assert!(r.text().ends_with("g привет"), "{}", r.text());
        }
    }
}

#[test]
fn selection_is_pasted_and_cycles_back() {
    let mut r = Rig::new();
    r.select_text("Ghbdtn vbh");
    r.shift2();
    r.assert_shows("Привет мир", 1);
    assert_eq!(r.w.borrow().clipboard, "OLD");
    r.shift2();
    r.assert_shows("Ghbdtn vbh", 0);
}

#[test]
fn selection_mixed_and_reverse() {
    let mut r = Rig::new();
    r.select_text("ghbdtn мир");
    r.shift2();
    assert_eq!(r.text(), "привет мир");

    let mut r = Rig::new();
    r.w.borrow_mut().cur = 1;
    r.select_text("руддщ цщкдв");
    r.shift2();
    r.assert_shows("hello world", 0);
}

#[test]
fn multiline_selection_is_pasted_without_cycle() {
    let mut r = Rig::new();
    r.select_text("ghbdtn\nvbh");
    r.shift2();
    assert_eq!(r.text(), "привет\nмир");
    let before = r.emitted();
    r.shift2();
    assert_eq!(r.emitted(), before);
}

#[test]
fn selection_by_typing_handles_capslock() {
    let mut r = Rig::build(&["us", "ru"], SelectionMethod::Type, true);
    r.w.borrow_mut().caps = true;
    r.select_text("Ghbdtn");
    r.shift2();
    assert_eq!(r.text(), "Привет");
    assert!(!r.w.borrow().log.contains(&"combo".to_string()));
}

#[test]
fn copying_and_extending_keep_selection() {
    const CTRL: KeyCode = KeyCode::KEY_LEFTCTRL;
    let keep: [(&[KeyCode], KeyCode); 4] = [
        (&[CTRL], KeyCode::KEY_C),
        (&[CTRL], KeyCode::KEY_INSERT),
        (&[LS], KeyCode::KEY_LEFT),
        (&[CTRL, LS], KeyCode::KEY_RIGHT),
    ];
    for (hold, key) in keep {
        let mut r = Rig::new();
        r.select_text("ghbdtn");
        r.press_with(hold, key);
        r.shift2();
        assert_eq!(r.text(), "привет", "{hold:?}+{key:?}");
    }
    // Вставка, стрелка без Shift, Shift+Enter — выделения больше нет.
    let drop: [(&[KeyCode], KeyCode); 3] = [
        (&[CTRL], KeyCode::KEY_V),
        (&[], KeyCode::KEY_LEFT),
        (&[LS], KeyCode::KEY_ENTER),
    ];
    for (hold, key) in drop {
        let mut r = Rig::new();
        r.select_text("ghbdtn");
        r.press_with(hold, key);
        r.shift2();
        assert_eq!(r.emitted(), 0, "{hold:?}+{key:?}");
    }
}

#[test]
fn selection_length_limits() {
    // Длиннее max_selection — не трогается.
    let mut r = Rig::new();
    let long = "g".repeat(r.eng.cfg.max_selection + 1);
    r.select_text(&long);
    r.shift2();
    assert_eq!((r.text(), r.emitted()), (long, 0));

    // Замену длиннее SEGMENT_MAX повторный ⇧⇧ уже не возвращает.
    for (len, back) in [(SEGMENT_MAX, "g"), (SEGMENT_MAX + 1, "п")] {
        let mut r = Rig::new();
        r.select_text(&"g".repeat(len));
        r.shift2();
        assert_eq!(r.text(), "п".repeat(len));
        r.shift2();
        assert_eq!(r.text(), back.repeat(len), "{len} символов");
    }
}

#[test]
fn stale_primary_after_click_is_ignored() {
    let mut r = Rig::new();
    r.select_text("ghbdtn");
    r.click(); // выделение снято, но PRIMARY ещё хранит старый текст
    r.w.borrow_mut().sel = None;
    r.shift2();
    assert_eq!((r.text().as_str(), r.emitted()), ("ghbdtn", 0));
}

#[test]
fn own_paste_does_not_revalidate_selection() {
    let mut r = Rig::new();
    r.select_text("ghbdtn");
    r.shift2();
    r.eng.primary_changed(6); // эхо нашей же вставки
    r.shift2(); // значит, это повтор по буферу, а не новое выделение
    assert_eq!(r.text(), "ghbdtn");
}

#[test]
fn echo_of_slow_paste_is_ignored() {
    let mut r = Rig::new();
    // Буфер обмена отвечает не сразу: вставка дольше SELF_CHANGE_GRACE.
    r.w.borrow_mut().paste_secs = SELF_CHANGE_GRACE + 1.0;
    r.select_text("ghbdtn");
    r.shift2();
    assert_eq!(r.text(), "привет");
    r.eng.primary_changed(6); // эхо вставки, пришедшее сразу после неё
    r.shift2(); // повтор по буферу, а не новое выделение
    assert_eq!(r.text(), "ghbdtn");
}

#[test]
fn typing_over_selection_switches_to_word_mode() {
    let mut r = Rig::new();
    r.select_text("abc");
    r.type_text("x");
    r.shift2();
    assert_eq!(r.text(), "ч");
}

#[test]
fn records_what_was_fixed() {
    let mut r = Rig::new();
    r.type_text("ghbdtn ");
    r.shift2();
    r.shift2(); // повтор по слову — тоже слово
    r.select_text("Ghbdtn vbh");
    r.shift2();
    r.shift2(); // повтор по выделению — возврат
    let actions = r.eng.actions();
    let got: Vec<_> = actions
        .iter()
        .map(|a| (a.kind, a.chars, a.from.as_str(), a.to.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            (ActionKind::Word, 7, "us", "ru"),
            (ActionKind::Word, 7, "ru", "us"),
            (ActionKind::Selection, 10, "us", "ru"),
            (ActionKind::Undo, 10, "ru", "us"),
        ]
    );

    // Раскладка неизвестна (переключение хоткеем) — «?».
    let mut r = Rig::build(&["us", "ru"], SelectionMethod::Paste, false);
    r.type_text("ghbdtn");
    r.shift2();
    let actions = r.eng.actions();
    assert_eq!(
        (actions[0].from.as_str(), actions[0].to.as_str()),
        ("?", "?")
    );
}

#[test]
fn three_layouts_cycle() {
    let mut r = Rig::build(&["us", "ru", "us2"], SelectionMethod::Paste, true);
    r.type_text("ghbdtn");
    r.shift2();
    r.assert_shows("привет", 1);
    r.shift2();
    r.assert_shows("ghbdtn", 2);
    r.shift2();
    r.assert_shows("ghbdtn", 0);
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
    assert_eq!(r.emitted(), 0);
    r.eng.set_enabled(true);
    r.type_text(" ntcn");
    r.shift2();
    assert_eq!(r.text(), "ghbdtn тест");
}

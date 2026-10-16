//! Таблицы раскладок из настоящей keymap.
//!
//! Композитор отдаёт keymap любому Wayland-клиенту (событие `wl_keyboard.keymap`),
//! в ней все раскладки сессии в том же порядке, что и у переключателя. Разбирает
//! её системная libxkbcommon, загружаемая через dlopen: без неё (или вне Wayland)
//! остаются встроенные таблицы us/ru. Так перекодировка выделения работает для
//! любых раскладок и вариантов — украинской, Dvorak, «фонетической» и т.д.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::os::unix::net::UnixStream;
use std::ptr;
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

use evdev::KeyCode;
use libloading::Library;
use log::{debug, info};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_keyboard, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};

use crate::keys::{Layout, ROW_KEYS};

/// Раскладки сессии: таблица и человеческое имя каждой группы.
#[derive(Clone, Debug, Default)]
pub struct Keymap {
    pub layouts: Vec<Arc<Layout>>,
    /// CapsLock переключает раскладку, а не регистр (grp:caps_toggle и подобные).
    pub caps_switches_group: bool,
}

// ─── libxkbcommon через dlopen ──────────────────────────────────────────────

type Ctx = *mut c_void;
type Map = *mut c_void;

#[repr(C)]
struct RuleNames {
    rules: *const c_char,
    model: *const c_char,
    layout: *const c_char,
    variant: *const c_char,
    options: *const c_char,
}

struct Xkb {
    _lib: Library,
    context_new: unsafe extern "C" fn(c_int) -> Ctx,
    context_unref: unsafe extern "C" fn(Ctx),
    keymap_new_from_string: unsafe extern "C" fn(Ctx, *const c_char, c_int, c_int) -> Map,
    keymap_new_from_names: unsafe extern "C" fn(Ctx, *const RuleNames, c_int) -> Map,
    keymap_unref: unsafe extern "C" fn(Map),
    num_layouts: unsafe extern "C" fn(Map) -> u32,
    num_layouts_for_key: unsafe extern "C" fn(Map, u32) -> u32,
    layout_get_name: unsafe extern "C" fn(Map, u32) -> *const c_char,
    syms_by_level: unsafe extern "C" fn(Map, u32, u32, u32, *mut *const u32) -> c_int,
    keysym_to_utf32: unsafe extern "C" fn(u32) -> u32,
}

const KEYMAP_FORMAT_TEXT_V1: c_int = 1;
/// Keysym'ы, которыми клавиша переключает группу раскладок.
const GROUP_SWITCH_SYMS: [u32; 5] = [0xfe08, 0xfe09, 0xfe0a, 0xfe0b, 0xfe0c]; // ISO_Next_Group… ISO_Last_Group_Lock
const KEY_CAPS_XKB: u32 = 58 + 8;

fn library() -> Option<&'static Xkb> {
    static XKB: OnceLock<Option<Xkb>> = OnceLock::new();
    XKB.get_or_init(|| {
        // SAFETY: системная libxkbcommon; сигнатуры совпадают с xkbcommon.h.
        unsafe {
            let lib = Library::new("libxkbcommon.so.0").ok()?;
            macro_rules! sym {
                ($name:literal) => {
                    *lib.get(concat!($name, "\0").as_bytes()).ok()?
                };
            }
            Some(Xkb {
                context_new: sym!("xkb_context_new"),
                context_unref: sym!("xkb_context_unref"),
                keymap_new_from_string: sym!("xkb_keymap_new_from_string"),
                keymap_new_from_names: sym!("xkb_keymap_new_from_names"),
                keymap_unref: sym!("xkb_keymap_unref"),
                num_layouts: sym!("xkb_keymap_num_layouts"),
                num_layouts_for_key: sym!("xkb_keymap_num_layouts_for_key"),
                layout_get_name: sym!("xkb_keymap_layout_get_name"),
                syms_by_level: sym!("xkb_keymap_key_get_syms_by_level"),
                keysym_to_utf32: sym!("xkb_keysym_to_utf32"),
                _lib: lib,
            })
        }
    })
    .as_ref()
}

/// Скомпилированная keymap; освобождается вместе с контекстом.
struct Compiled<'a> {
    xkb: &'a Xkb,
    ctx: Ctx,
    map: Map,
}

impl Drop for Compiled<'_> {
    fn drop(&mut self) {
        // SAFETY: указатели получены от этой же библиотеки и освобождаются один раз.
        unsafe {
            if !self.map.is_null() {
                (self.xkb.keymap_unref)(self.map);
            }
            (self.xkb.context_unref)(self.ctx);
        }
    }
}

impl Compiled<'_> {
    fn new(build: impl FnOnce(&Xkb, Ctx) -> Map) -> Option<Compiled<'static>> {
        let xkb = library()?;
        // SAFETY: 0 — XKB_CONTEXT_NO_FLAGS.
        let ctx = unsafe { (xkb.context_new)(0) };
        if ctx.is_null() {
            return None;
        }
        let map = build(xkb, ctx);
        let compiled = Compiled { xkb, ctx, map };
        (!compiled.map.is_null()).then_some(compiled)
    }

    fn sym(&self, key: u32, group: u32, level: u32) -> Option<u32> {
        let mut syms: *const u32 = ptr::null();
        // SAFETY: keymap жива; xkb пишет указатель на свой внутренний массив.
        unsafe {
            let groups = (self.xkb.num_layouts_for_key)(self.map, key);
            if groups == 0 {
                return None;
            }
            // Клавиши с меньшим числом групп «заворачиваются», как это делает сам xkb.
            let count = (self.xkb.syms_by_level)(self.map, key, group % groups, level, &mut syms);
            (count >= 1 && !syms.is_null()).then(|| *syms)
        }
    }

    fn char_at(&self, key: KeyCode, group: u32, level: u32) -> Option<char> {
        let sym = self.sym(u32::from(key.code()) + 8, group, level)?;
        // SAFETY: чистая функция.
        let code = unsafe { (self.xkb.keysym_to_utf32)(sym) };
        char::from_u32(code).filter(|c| *c != '\0' && !c.is_control())
    }

    fn keymap(&self) -> Keymap {
        // SAFETY: keymap жива; имена — C-строки внутри keymap.
        let count = unsafe { (self.xkb.num_layouts)(self.map) };
        let layouts = (0..count)
            .map(|group| {
                let name = unsafe { (self.xkb.layout_get_name)(self.map, group) };
                let name = if name.is_null() {
                    format!("Раскладка {}", group + 1)
                } else {
                    unsafe { CStr::from_ptr(name) }
                        .to_string_lossy()
                        .into_owned()
                };
                let keys = ROW_KEYS.iter().chain([&KeyCode::KEY_SPACE]);
                let pairs = keys.map(|&key| {
                    (
                        key,
                        [self.char_at(key, group, 0), self.char_at(key, group, 1)],
                    )
                });
                Arc::new(Layout::from_pairs(name, pairs))
            })
            .collect();
        let caps_switches_group = (0..count.max(1)).any(|group| {
            self.sym(KEY_CAPS_XKB, group, 0)
                .is_some_and(|sym| GROUP_SWITCH_SYMS.contains(&sym))
        });
        Keymap {
            layouts,
            caps_switches_group,
        }
    }
}

/// Разобрать keymap в текстовом формате xkb (как её присылает композитор).
pub fn keymap_from_text(text: &str) -> Option<Keymap> {
    let text = CString::new(text.trim_end_matches('\0')).ok()?;
    let compiled = Compiled::new(|xkb, ctx| {
        // SAFETY: строка с нулём на конце живёт до конца вызова.
        unsafe { (xkb.keymap_new_from_string)(ctx, text.as_ptr(), KEYMAP_FORMAT_TEXT_V1, 0) }
    })?;
    Some(compiled.keymap())
}

/// Скомпилировать keymap по именам: `layouts = "us,ru"`, `variants = ",phonetic"`,
/// `options = "grp:caps_toggle"`.
pub fn keymap_from_names(layouts: &str, variants: &str, options: &str) -> Option<Keymap> {
    let layout = CString::new(layouts).ok()?;
    let variant = CString::new(variants).ok()?;
    let option = CString::new(options).ok()?;
    let compiled = Compiled::new(|xkb, ctx| {
        let names = RuleNames {
            rules: ptr::null(),
            model: ptr::null(),
            layout: layout.as_ptr(),
            variant: variant.as_ptr(),
            options: option.as_ptr(),
        };
        // SAFETY: строки живут до конца вызова; NULL — значения по умолчанию.
        unsafe { (xkb.keymap_new_from_names)(ctx, &names, 0) }
    })?;
    Some(compiled.keymap())
}

// ─── keymap от композитора ──────────────────────────────────────────────────

#[derive(Default)]
struct State {
    keyboard: Option<wl_keyboard::WlKeyboard>,
    keymap: Option<String>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
            && caps.contains(wl_seat::Capability::Keyboard)
            && state.keyboard.is_none()
        {
            state.keyboard = Some(seat.get_keyboard(qh, ()));
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Keymap {
            format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
            fd,
            size,
        } = event
        {
            // fd бывает общим для всех клиентов: читаем со смещения 0, не двигая позицию.
            let mut buf = vec![0u8; size as usize];
            if File::from(fd).read_exact_at(&mut buf, 0).is_ok() {
                state.keymap = Some(String::from_utf8_lossy(&buf).into_owned());
            }
        }
    }
}

fn connect() -> Option<Connection> {
    let display = crate::env::wayland_display()?;
    let path = if display.starts_with('/') {
        display.into()
    } else {
        crate::env::runtime_dir().join(display)
    };
    Connection::from_socket(UnixStream::connect(path).ok()?).ok()
}

/// Один раз получить keymap композитора.
pub fn fetch() -> Option<Keymap> {
    let conn = connect()?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn).ok()?;
    let _seat: wl_seat::WlSeat = globals.bind(&queue.handle(), 1..=8, ()).ok()?;
    let mut state = State::default();
    for _ in 0..4 {
        queue.roundtrip(&mut state).ok()?;
        if let Some(text) = state.keymap.take() {
            return keymap_from_text(&text);
        }
    }
    None
}

/// Следить за keymap: при смене списка раскладок композитор пришлёт новую.
pub fn spawn_watcher(on_change: impl Fn(Keymap) + Send + 'static) {
    let spawned = thread::Builder::new().name("keymap".into()).spawn(move || {
        loop {
            if let Some(conn) = connect() {
                watch(&conn, &on_change);
            }
            thread::sleep(Duration::from_secs(10)); // сессии ещё нет или композитор перезапущен
        }
    });
    if let Err(err) = spawned {
        log::warn!("поток keymap не запустился: {err}");
    }
}

fn watch(conn: &Connection, on_change: &impl Fn(Keymap)) {
    let Ok((globals, mut queue)) = registry_queue_init::<State>(conn) else {
        return;
    };
    let Ok(_seat) = globals.bind::<wl_seat::WlSeat, _, _>(&queue.handle(), 1..=8, ()) else {
        return;
    };
    let mut state = State::default();
    while queue.blocking_dispatch(&mut state).is_ok() {
        if let Some(text) = state.keymap.take() {
            match keymap_from_text(&text) {
                Some(keymap) => {
                    let names: Vec<&str> = keymap.layouts.iter().map(|l| l.name.as_str()).collect();
                    info!("раскладки из keymap: {}", names.join(", "));
                    on_change(keymap);
                }
                None => debug!("keymap не разобрана (нет libxkbcommon?)"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{builtin, convert_text};

    /// Встроенные таблицы должны совпадать с тем, что даёт настоящий xkb.
    #[test]
    fn builtin_tables_match_xkb() {
        let Some(keymap) = keymap_from_names("us,ru", ",", "") else {
            eprintln!("libxkbcommon недоступна — пропуск");
            return;
        };
        assert_eq!(keymap.layouts.len(), 2);
        assert!(
            keymap.layouts[0].name.starts_with("English"),
            "{}",
            keymap.layouts[0].name
        );
        assert_eq!(keymap.layouts[1].name, "Russian");
        assert!(!keymap.caps_switches_group);
        for (xkb, code) in keymap.layouts.iter().zip(["us", "ru"]) {
            let ours = builtin(code).unwrap();
            for key in ROW_KEYS {
                for shift in [false, true] {
                    assert_eq!(
                        xkb.char_at(key, shift),
                        ours.char_at(key, shift),
                        "{code} {key:?} shift={shift}"
                    );
                }
            }
        }
    }

    #[test]
    fn variants_and_options_come_from_xkb() {
        let Some(keymap) = keymap_from_names("us,ua", "dvorak,", "grp:caps_toggle") else {
            return;
        };
        let (dvorak, ua) = (&keymap.layouts[0], &keymap.layouts[1]);
        assert_eq!(dvorak.char_at(KeyCode::KEY_Q, false), Some('\''));
        assert_eq!(ua.char_at(KeyCode::KEY_S, false), Some('і'));
        // «hello», набранное пальцами Dvorak в украинской раскладке.
        assert_eq!(convert_text("овззі", ua, dvorak), "hello");
        assert!(keymap.caps_switches_group);
    }

    #[test]
    fn keymap_text_round_trip() {
        let Some(from_names) = keymap_from_names("us,ru", ",", "") else {
            return;
        };
        assert!(keymap_from_text("не keymap").is_none());
        assert_eq!(from_names.layouts.len(), 2);
    }
}

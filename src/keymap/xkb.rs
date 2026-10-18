//! Разбор keymap системной libxkbcommon, загружаемой через dlopen: без неё
//! остаются встроенные таблицы us/ru. Так перекодировка выделения работает
//! для любых раскладок и вариантов — украинской, Dvorak, «фонетической» и т.д.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;
use std::sync::{Arc, OnceLock};

use evdev::KeyCode;
use libloading::Library;

use super::{Keymap, Layout};
use crate::keys::ROW_KEYS;

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
/// dead_grave … dead_lowline и прочие мёртвые keysym'ы.
const DEAD_SYMS: std::ops::RangeInclusive<u32> = 0xfe50..=0xfe93;

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
        let sym = self
            .sym(u32::from(key.code()) + 8, group, level)
            .filter(|sym| !DEAD_SYMS.contains(sym))?;
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
                let keys = || ROW_KEYS.iter().chain([&KeyCode::KEY_SPACE]);
                let pairs = keys().map(|&key| {
                    (
                        key,
                        [self.char_at(key, group, 0), self.char_at(key, group, 1)],
                    )
                });
                let dead = keys().flat_map(|&key| {
                    [false, true].into_iter().filter_map(move |shift| {
                        let sym = self.sym(u32::from(key.code()) + 8, group, u32::from(shift))?;
                        DEAD_SYMS.contains(&sym).then_some((key, shift))
                    })
                });
                Arc::new(Layout::from_pairs(name, pairs).with_dead(dead.collect::<Vec<_>>()))
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
pub fn from_text(text: &str) -> Option<Keymap> {
    let text = CString::new(text.trim_end_matches('\0')).ok()?;
    let compiled = Compiled::new(|xkb, ctx| {
        // SAFETY: строка с нулём на конце живёт до конца вызова.
        unsafe { (xkb.keymap_new_from_string)(ctx, text.as_ptr(), KEYMAP_FORMAT_TEXT_V1, 0) }
    })?;
    Some(compiled.keymap())
}

/// Скомпилировать keymap по именам: `layouts = "us,ru"`, `variants = ",phonetic"`,
/// `options = "grp:caps_toggle"`.
pub fn from_names(layouts: &str, variants: &str, options: &str) -> Option<Keymap> {
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

#[cfg(test)]
mod tests {
    use evdev::KeyCode;

    use super::*;

    use crate::keymap::{builtin, convert_text};
    use crate::keys::ROW_KEYS;
    use crate::testutil::skip;

    /// Встроенные таблицы должны совпадать с тем, что даёт настоящий xkb.
    #[test]
    fn builtin_tables_match_xkb() {
        let Some(keymap) = from_names("us,ru", ",", "") else {
            return skip("libxkbcommon недоступна");
        };
        assert_eq!(keymap.layouts.len(), 2);
        assert!(
            keymap.layouts[0].name.starts_with("English"),
            "{}",
            keymap.layouts[0].name
        );
        assert_eq!(keymap.layouts[1].name, "Russian");
        assert_eq!(
            keymap.names(),
            format!("{}, Russian", keymap.layouts[0].name)
        );
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
        let Some(keymap) = from_names("us,ua", "dvorak,", "grp:caps_toggle") else {
            return skip("libxkbcommon недоступна");
        };
        let (dvorak, ua) = (&keymap.layouts[0], &keymap.layouts[1]);
        assert_eq!(dvorak.char_at(KeyCode::KEY_Q, false), Some('\''));
        assert_eq!(ua.char_at(KeyCode::KEY_S, false), Some('і'));
        // «hello», набранное пальцами Dvorak в украинской раскладке.
        assert_eq!(convert_text("овззі", ua, dvorak), "hello");
        assert!(keymap.caps_switches_group);
    }

    #[test]
    fn dead_keys_are_marked() {
        let Some(keymap) = from_names("us,ru", "intl,", "") else {
            return skip("libxkbcommon недоступна");
        };
        let (intl, ru) = (&keymap.layouts[0], &keymap.layouts[1]);
        // US International: ' и Shift+6 — мёртвые, буквы — нет.
        assert!(intl.is_dead(KeyCode::KEY_APOSTROPHE, false));
        assert!(intl.is_dead(KeyCode::KEY_6, true));
        assert!(!intl.is_dead(KeyCode::KEY_A, false));
        assert_eq!(intl.char_at(KeyCode::KEY_APOSTROPHE, false), None);
        assert!(!ru.is_dead(KeyCode::KEY_APOSTROPHE, false));
    }

    #[test]
    fn keymap_text_round_trip() {
        let Some(from_names) = from_names("us,ru", ",", "") else {
            return skip("libxkbcommon недоступна");
        };
        assert!(from_text("не keymap").is_none());
        assert_eq!(from_names.layouts.len(), 2);
    }
}

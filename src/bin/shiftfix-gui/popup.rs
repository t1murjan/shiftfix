//! Окно shiftfix — маленькое, без рамки, поверх остальных, у значка в трее.
//! Что в нём рисуется — `App` (`app.rs`); здесь — как окно открывается, где
//! встаёт и когда закрывается.
//!
//! Окно одно (`Instance::Popup`). Щелчок по значку запускает `--popup`, и
//! такой запуск при открытом окне закрывает его: щелчок по значку и
//! открывает, и закрывает окно. Остальные запуски (`--page`, меню значка)
//! просят открытое окно показаться и открыть вкладку. Само окно закрывается
//! по Esc и когда теряет фокус — щёлкнули мимо; во втором случае оно
//! отмечает время закрытия, и `--popup` сразу после этого не открывается
//! (см. `same_click`).
//!
//! В X11 окно встаёт у точки щелчка (`--at`). В Wayland окно не может
//! поставить себя само: в Hyprland и KDE оно просит композитор поставить его
//! у курсора (`hyprland.rs`, `kwin.rs`), в остальных окружениях место
//! выбирает композитор. Где именно — `placement.rs`.

use std::fs::{self, File};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, SystemTime};

use eframe::egui::{self, Event, Key, Pos2, Ui, ViewportCommand, ViewportId, pos2};
use shiftfix::config::ThemeMode;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, Window};

use crate::app::{App, Page};
use crate::hyprland;
use crate::instance::{self, Instance, Start};
use crate::kwin;
use crate::placement;

/// Размер окна в логических пикселях.
pub const SIZE: (i32, i32) = (440, 540);

/// Класс окна (app_id в Wayland): по нему окно находят KWin-скрипт,
/// `hyprland.rs` и правила окна.
pub const APP_ID: &str = "shiftfix-popup";

/// Во сколько раз прокрутка быстрее, чем у egui по умолчанию: колесо мыши у
/// него сдвигает вкладку на 40 пикселей за щелчок, а вкладки длинные.
const SCROLL_SPEED: f32 = 2.5;

/// Сколько после закрытия по потере фокуса новый `--popup` считается тем же
/// щелчком по значку: служба запускает его чуть позже, чем панель забирает
/// фокус.
const SAME_CLICK: Duration = Duration::from_millis(500);

/// Как открывают окно.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Open {
    /// Вкладка (`--page`), иначе «Главная».
    pub page: Option<Page>,
    /// Где щёлкнули по значку (`--at X,Y`).
    pub at: Option<(i32, i32)>,
    /// Щелчок по значку (`--popup`): при открытом окне — закрыть его.
    pub toggle: bool,
    /// Режим оформления из `--theme`.
    pub theme: Option<ThemeMode>,
}

/// Открыть окно или передать просьбу уже открытому.
pub fn run(open: Open) -> eframe::Result {
    let _lock = match Instance::Popup.start() {
        Start::First(lock) => lock,
        // Окно уже открыто. Если оно ещё открывается и не слушает сокет,
        // просьба пропадает.
        Start::Second => {
            let line = if open.toggle {
                instance::CLOSE.to_owned()
            } else {
                instance::raise_line(open.page)
            };
            Instance::Popup.send(&line);
            return Ok(());
        }
    };
    // В KDE и waybar щелчок по значку сначала забирает у открытого окна
    // фокус — окно закрывается само, — и только потом служба запускает новый
    // `--popup`. Это тот же щелчок: открывать окно снова не надо.
    if open.toggle && same_click(since_closed(&Instance::Popup.path("closed"))) {
        return Ok(());
    }
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("shiftfix")
        .with_app_id(APP_ID)
        .with_inner_size([SIZE.0 as f32, SIZE.1 as f32])
        .with_resizable(false)
        .with_decorations(false)
        .with_window_level(egui::WindowLevel::AlwaysOnTop);
    // Где встать окну. В X11 оно ставит себя само у точки щелчка. В Wayland
    // окно себя не ставит, и до создания окна оно просит композитор открыть
    // его у курсора — щёлкнули по значку или пункту его меню, и курсор там:
    // в Hyprland — правилом окна (а когда окно появится, ещё и передвинет
    // его туда же), в KDE — KWin-скриптом. В остальных окружениях место
    // выбирает композитор.
    let (mut hyprland_spot, mut kwin_script) = (None, None);
    if !wayland() {
        if let Some(pos) = open.at.and_then(x11_position) {
            viewport = viewport.with_position(pos);
        }
    } else if hyprland::running() {
        match hyprland::cursor_and_area() {
            Ok((cursor, area)) => {
                let (x, y) = placement::popup_position(cursor, area, SIZE);
                if let Err(err) = hyprland::expect_popup(x, y) {
                    log::warn!("Hyprland откроет окно по-своему и потом передвинет: {err:#}");
                }
                hyprland_spot = Some((x, y));
            }
            Err(err) => log::warn!("Hyprland не поставит окно у курсора: {err:#}"),
        }
    } else if kwin::running() {
        match kwin::Script::load(&Instance::Popup.path("js")) {
            Ok(script) => kwin_script = Some(script),
            Err(err) => log::warn!("KWin не поставит окно у курсора: {err:#}"),
        }
    }
    let result = crate::launch(viewport, move |cc, system| {
        let raised = listen(cc.egui_ctx.clone());
        // Окно создано. Если правило окна не сработало, hyprctl передвинет
        // его к значку. Hyprland покажет окно в списке окон с первым кадром,
        // а пока поток окна ждёт, кадр не нарисуется — ждать в своём потоке.
        if let Some((x, y)) = hyprland_spot {
            thread::spawn(move || {
                if let Err(err) = hyprland::place_popup(x, y) {
                    log::warn!("Hyprland не поставил окно у курсора: {err:#}");
                }
            });
        }
        let page = open.page.unwrap_or(Page::Home);
        let app = App::new(cc, page, raised, open.theme, system);
        Box::new(Popup {
            app,
            had_focus: false,
        })
    });
    if let Some(script) = kwin_script
        && let Err(err) = script.unload()
    {
        log::warn!("KWin: скрипт окна не выгрузился: {err:#}");
    }
    result
}

/// Слушать просьбы следующих запусков: закрыться или показаться. Вкладки,
/// которые при этом просят открыть, приходят в возвращённый канал.
fn listen(ctx: egui::Context) -> Receiver<Page> {
    let (pages, raised) = mpsc::channel();
    let root = ViewportId::ROOT;
    Instance::Popup.listen(move |line| {
        if line == instance::CLOSE {
            ctx.send_viewport_cmd_to(root, ViewportCommand::Close);
            return;
        }
        let Some(page) = instance::parse_raise(line) else {
            return;
        };
        if let Some(page) = page {
            let _ = pages.send(page);
        }
        // В Wayland композитор может вместо этого подсветить окно на панели.
        ctx.send_viewport_cmd_to(root, ViewportCommand::Focus);
        ctx.request_repaint();
    });
    raised
}

/// Окно откроется в Wayland: winit выбирает его, когда задан WAYLAND_DISPLAY.
fn wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
}

/// Новый попап — тот же щелчок по значку, что закрыл прежний: тот потерял
/// фокус `since_closed` назад (`None` — так не закрывался).
fn same_click(since_closed: Option<Duration>) -> bool {
    since_closed.is_some_and(|ago| ago < SAME_CLICK)
}

/// Отметить, что попап закрылся, потеряв фокус: время — mtime файла `mark`.
fn mark_closed(mark: &Path) {
    let marked = File::create(mark).and_then(|file| file.set_modified(SystemTime::now()));
    if let Err(err) = marked {
        log::warn!("{}: {err}", mark.display());
    }
}

/// Сколько прошло с отметки `mark_closed`.
fn since_closed(mark: &Path) -> Option<Duration> {
    let closed = fs::metadata(mark).and_then(|m| m.modified()).ok()?;
    SystemTime::now().duration_since(closed).ok()
}

/// Где поставить окно в X11: у точки щелчка `at`, целиком на экране и, если
/// оконный менеджер сообщает рабочую область, не заходя на панели.
fn x11_position(at: (i32, i32)) -> Option<Pos2> {
    let (conn, screen) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots.get(screen)?;
    let screen = [
        0,
        0,
        i32::from(root.width_in_pixels),
        i32::from(root.height_in_pixels),
    ];
    let area = x11_work_area(&conn, root.root).unwrap_or(screen);
    // Точка и область — в пикселях X: при масштабе окон больше 1 окно встанет
    // дальше от значка, но на экране.
    let (x, y) = placement::popup_position(at, area, SIZE);
    Some(pos2(x as f32, y as f32))
}

/// Рабочая область текущего рабочего стола без панелей: x, y, ширина, высота
/// (`_NET_WORKAREA`, по четыре числа на рабочий стол). `None` — оконный
/// менеджер её не сообщает.
fn x11_work_area(conn: &impl Connection, root: Window) -> Option<[i32; 4]> {
    let cardinals = |name: &[u8], count: u32| -> Option<Vec<u32>> {
        let atom = conn.intern_atom(true, name).ok()?.reply().ok()?.atom;
        let property = conn
            .get_property(false, root, atom, AtomEnum::CARDINAL, 0, count)
            .ok()?
            .reply()
            .ok()?;
        Some(property.value32()?.collect())
    };
    let desktop = cardinals(b"_NET_CURRENT_DESKTOP", 1)
        .and_then(|v| v.first().copied())
        .unwrap_or(0);
    let areas = cardinals(b"_NET_WORKAREA", desktop.checked_add(1)?.checked_mul(4)?)?;
    let area = areas.as_chunks::<4>().0.get(desktop as usize)?;
    let [x, y, width, height] = area.map(|v| v as i32);
    (width > 0 && height > 0).then_some([x, y, width, height])
}

struct Popup {
    app: App,
    /// Окно уже было в фокусе. До этого «не в фокусе» не значит «щёлкнули
    /// мимо»: композитор может и не сделать активным окно, запущенное из трея.
    had_focus: bool,
}

impl Popup {
    /// Закрыть окно по Esc и когда оно теряет фокус: щёлкнули мимо.
    fn close_when_left(&mut self, ctx: &egui::Context) {
        let (focused, escape) = ctx.input(|i| (i.viewport().focused, i.key_pressed(Key::Escape)));
        if focused == Some(true) {
            self.had_focus = true;
        }
        let left = self.had_focus && focused == Some(false);
        if left {
            // Может быть, фокус забрал щелчок по значку — он сейчас запустит
            // новый `--popup`, который должен знать, что этот щелчок окно закрыл.
            mark_closed(&Instance::Popup.path("closed"));
        }
        if escape || left {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }
}

impl eframe::App for Popup {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.close_when_left(ui.ctx());
        self.app.show(ui);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.app.on_exit();
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        faster_scroll(&mut raw_input.events);
    }
}

/// Ускорить прокрутку в `SCROLL_SPEED` раз: и колесом мыши, и тачпадом.
fn faster_scroll(events: &mut [Event]) {
    for event in events {
        if let Event::MouseWheel { delta, .. } = event {
            *delta *= SCROLL_SPEED;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_is_faster_other_events_untouched() {
        let wheel = |delta| Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta,
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        };
        let mut events = vec![
            wheel(egui::vec2(0.0, -1.0)),
            Event::Text("ы".into()),
            wheel(egui::vec2(2.0, 4.0)),
        ];
        faster_scroll(&mut events);
        assert_eq!(
            events,
            [
                wheel(egui::vec2(0.0, -2.5)),
                Event::Text("ы".into()),
                wheel(egui::vec2(5.0, 10.0)),
            ]
        );
    }

    #[test]
    fn click_that_closed_popup_does_not_reopen_it() {
        assert!(!same_click(None), "попап не закрывался по потере фокуса");
        assert!(same_click(Some(Duration::ZERO)));
        assert!(same_click(Some(Duration::from_millis(200))));
        assert!(same_click(Some(Duration::from_millis(499))));
        assert!(!same_click(Some(Duration::from_millis(500))));
        assert!(!same_click(Some(Duration::from_secs(5))));

        let dir = std::env::temp_dir().join(format!("shiftfix-popup-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mark = dir.join("closed");
        assert_eq!(since_closed(&mark), None);
        mark_closed(&mark);
        assert!(same_click(since_closed(&mark)));
        // Отметка старше полсекунды — новый щелчок, попап открывается.
        let old = SystemTime::now() - Duration::from_secs(2);
        File::options()
            .write(true)
            .open(&mark)
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert!(!same_click(since_closed(&mark)));
        mark_closed(&mark);
        assert!(same_click(since_closed(&mark)), "отметка обновляется");
        fs::remove_dir_all(dir).unwrap();
    }
}

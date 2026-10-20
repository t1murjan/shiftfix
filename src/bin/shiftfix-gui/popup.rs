//! Попап у значка в трее (`shiftfix-gui --popup`): состояние службы и
//! выключатель, раскладки, устройства, выделение, последние исправления,
//! новая версия и кнопки, открывающие окно. Данные, тема и действия — те же,
//! что у окна (`App`): тот же опрос службы (`link.rs`) и та же палитра.
//!
//! Попап один (`Instance::Popup`): второй запуск закрывает открытый — так
//! щелчок по значку и открывает, и закрывает его. Сам попап закрывается по
//! Esc и когда теряет фокус. Где он появится, в Wayland решает композитор
//! (правила окна для KDE и Hyprland — в README, класс окна `shiftfix-popup`);
//! в X11 попап встаёт у точки щелчка (`--at`).

use std::sync::mpsc;

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{
    self, Align, CornerRadius, FontId, Frame, Key, Layout, Margin, Pos2, Response, RichText,
    ScrollArea, Stroke, StrokeKind, TextWrapMode, Ui, Vec2, ViewportCommand, ViewportId, pos2,
    vec2,
};
use egui_phosphor::regular as icons;
use shiftfix::config::ThemeMode;
use shiftfix::ipc::{Request, Status};
use shiftfix::sys::{paths, proc};
use x11rb::connection::Connection as _;

use crate::app::{App, Page, State, state_label};
use crate::format::clock;
use crate::instance::{self, Instance, Start};
use crate::link::ServiceAction;
use crate::pages::home;
use crate::theme::Palette;
use crate::toast::ToastKind;
use crate::update::Outcome;
use crate::widgets::{self, bold};

/// Размер попапа: без прокрутки помещаются и три раскладки (плашки в две
/// строки), и строка о новой версии.
const SIZE: Vec2 = vec2(380.0, 460.0);

/// Открыть попап; `at` — где щёлкнули по значку (`--at X,Y`).
pub fn run(at: Option<(i32, i32)>, forced_theme: Option<ThemeMode>) -> eframe::Result {
    let _lock = match Instance::Popup.start() {
        Start::First(lock) => lock,
        // Попап уже открыт: щелчок по значку его закрывает. Если попап ещё
        // открывается и не слушает сокет, щелчок пропадает.
        Start::Second => {
            Instance::Popup.send(instance::CLOSE);
            return Ok(());
        }
    };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("shiftfix")
        .with_app_id("shiftfix-popup")
        .with_inner_size(SIZE)
        .with_resizable(false)
        .with_decorations(false)
        .with_window_level(egui::WindowLevel::AlwaysOnTop);
    if let Some(pos) = at.and_then(x11_position) {
        viewport = viewport.with_position(pos);
    }
    crate::launch(viewport, move |cc, system| {
        let ctx = cc.egui_ctx.clone();
        Instance::Popup.listen(move |line| {
            if line == instance::CLOSE {
                ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
            }
        });
        // Вкладок у попапа нет, и просить открыть вкладку его некому.
        let (_, no_pages) = mpsc::channel();
        let app = App::new(cc, Page::Home, no_pages, forced_theme, system);
        Box::new(Popup {
            app,
            had_focus: false,
        })
    })
}

/// Где поставить попап в X11: у точки щелчка `at`, целиком на экране.
/// В Wayland положение окна выбирает композитор — `None`.
fn x11_position(at: (i32, i32)) -> Option<Pos2> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()) {
        return None;
    }
    let (conn, screen) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots.get(screen)?;
    let screen = vec2(
        f32::from(root.width_in_pixels),
        f32::from(root.height_in_pixels),
    );
    // Точка и экран — в пикселях X: при масштабе окон больше 1 попап встанет
    // дальше от значка, но на экране.
    Some(place(pos2(at.0 as f32, at.1 as f32), SIZE, screen))
}

/// Левый верхний угол попапа размера `size` у точки `at` на экране `screen`:
/// под точкой, а если там не помещается (панель внизу) — над ней; по
/// горизонтали — по центру точки. За край экрана попап не выходит.
fn place(at: Pos2, size: Vec2, screen: Vec2) -> Pos2 {
    const GAP: f32 = 8.0;
    let below = at.y + GAP;
    let y = if below + size.y <= screen.y {
        below
    } else {
        at.y - GAP - size.y
    };
    let x = at.x - size.x / 2.0;
    pos2(
        x.clamp(0.0, (screen.x - size.x).max(0.0)),
        y.clamp(0.0, (screen.y - size.y).max(0.0)),
    )
}

struct Popup {
    app: App,
    /// Попап уже был в фокусе. До этого «не в фокусе» не значит «щёлкнули
    /// мимо»: композитор может и не сделать активным окно, запущенное из трея.
    had_focus: bool,
}

impl Popup {
    /// Закрыть попап по Esc и когда он теряет фокус: щёлкнули мимо.
    fn close_when_left(&mut self, ctx: &egui::Context) {
        let (focused, escape) = ctx.input(|i| (i.viewport().focused, i.key_pressed(Key::Escape)));
        if focused == Some(true) {
            self.had_focus = true;
        }
        if escape || (self.had_focus && focused == Some(false)) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    /// Открыть окно (на вкладке `page`) и закрыть попап.
    fn open_window(&mut self, ctx: &egui::Context, page: Option<Page>) {
        let mut args = Vec::new();
        if let Some(page) = page {
            args.extend(["--page", page.name()]);
        }
        match proc::spawn_detached(paths::sibling_binary("shiftfix-gui"), &args) {
            Ok(()) => ctx.send_viewport_cmd(ViewportCommand::Close),
            Err(err) => self.app.toast(ToastKind::Error, format!("{err:#}")),
        }
    }

    /// Значок, название, состояние и выключатель.
    fn header(&mut self, ui: &mut Ui, p: &Palette, state: State) {
        ui.horizontal(|ui| {
            ui.add(self.app.logo(40.0));
            ui.vertical(|ui| {
                ui.label(bold("shiftfix", 17.0).color(p.text));
                ui.label(status_line(state, p));
            });
            if matches!(state, State::Working | State::Paused) {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let mut on = state == State::Working;
                    let toggle = widgets::toggle(ui, &mut on, 24.0, p)
                        .on_hover_text("Включить или поставить на паузу");
                    if toggle.changed() {
                        self.app
                            .link
                            .send(Request::SetEnabled { enabled: on }, None);
                    }
                });
            }
        });
    }

    /// Служба не отвечает: что с ней и кнопки, как на «Главной».
    fn stopped(&mut self, ui: &mut Ui, p: &Palette, state: State) {
        widgets::card(ui, p, |ui| {
            let (title, color) = state_label(state, p);
            ui.horizontal(|ui| {
                ui.label(widgets::icon(icons::WARNING_CIRCLE, 20.0).color(color));
                ui.label(bold(title, 16.0).color(p.text));
            });
            let text = home::hero_text(&self.app, state);
            ui.add(egui::Label::new(RichText::new(text).size(13.5).color(p.weak)).wrap());
            ui.add_space(6.0);
            ui.horizontal(|ui| match state {
                State::Stopped => {
                    if widgets::button(ui, p, icons::PLAY, "Запустить", true).clicked() {
                        self.app.service(ServiceAction::Start);
                    }
                    if widgets::button(ui, p, icons::SCROLL, "Журнал", false).clicked() {
                        self.open_window(ui.ctx(), Some(Page::Log));
                    }
                }
                State::NotInstalled => {
                    let install = widgets::button(
                        ui,
                        p,
                        icons::DOWNLOAD_SIMPLE,
                        "Установить и запустить",
                        true,
                    );
                    if install.clicked() {
                        self.app.service(ServiceAction::Install);
                    }
                }
                State::Working | State::Paused => {}
            });
        });
    }

    /// Проверка нашла новую версию: строка о ней и «Обновить», как на «Главной».
    fn new_version(&mut self, ui: &mut Ui, p: &Palette, version: &str) {
        Frame::NONE
            .fill(p.accent.gamma_multiply(0.12))
            .stroke(Stroke::new(1.0, p.accent.gamma_multiply(0.4)))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(widgets::icon(icons::ARROW_CIRCLE_UP, 18.0).color(p.accent));
                    ui.label(
                        RichText::new(format!("Доступна версия {version}"))
                            .size(13.5)
                            .color(p.text),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let button = egui::Button::new(
                            RichText::new("Обновить").size(13.0).color(p.on_accent),
                        )
                        .fill(p.accent)
                        .stroke(Stroke::NONE)
                        .corner_radius(CornerRadius::same(7))
                        .min_size(vec2(0.0, 26.0));
                        if ui.add(button).clicked() {
                            self.app.run_update();
                        }
                    });
                });
            });
    }

    /// Кнопки внизу: пауза, окно и инструкция.
    fn buttons(&mut self, ui: &mut Ui, p: &Palette, state: State) {
        let ctx = ui.ctx().clone();
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.columns(3, |cols| {
            let (icon, text) = if state == State::Paused {
                (icons::PLAY, "Продолжить")
            } else {
                (icons::PAUSE, "Пауза")
            };
            let running = matches!(state, State::Working | State::Paused);
            if footer_button(&mut cols[0], p, icon, text, running).clicked() {
                let enabled = state == State::Paused;
                self.app.link.send(Request::SetEnabled { enabled }, None);
            }
            if footer_button(&mut cols[1], p, icons::GEAR, "Настройки", true).clicked() {
                self.open_window(&ctx, None);
            }
            if footer_button(&mut cols[2], p, icons::BOOK_OPEN_TEXT, "Справка", true).clicked()
            {
                self.open_window(&ctx, Some(Page::Help));
            }
        });
    }
}

impl eframe::App for Popup {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.close_when_left(&ctx);
        let p = self.app.begin_frame(&ctx);
        let state = self.app.state();
        let status = self.app.snap.status.clone();
        let newer = match self.app.update.state().outcome {
            Some(Outcome::Newer(version)) => Some(version),
            _ => None,
        };
        // Низ — кнопки и новая версия над ними, остальное — сверху вниз.
        egui::Panel::bottom("buttons")
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(p.bg).inner_margin(Margin {
                left: 16,
                right: 16,
                top: 0,
                bottom: 16,
            }))
            .show(ui, |ui| {
                if let Some(version) = &newer {
                    self.new_version(ui, &p, version);
                    ui.add_space(4.0);
                }
                self.buttons(ui, &p, state);
            });
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(p.bg).inner_margin(Margin::same(16)))
            .show(ui, |ui| {
                // Обычно всё помещается; прокрутка — для длинной ошибки службы.
                ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        self.header(ui, &p, state);
                        ui.add_space(12.0);
                        match &status {
                            Some(status) => {
                                tiles(ui, &p, status);
                                ui.add_space(14.0);
                                recent(ui, &p, status);
                            }
                            None => self.stopped(ui, &p, state),
                        }
                    });
            });
        // Рамка: без неё попап сливается с окнами под ним.
        ui.painter().rect_stroke(
            ctx.content_rect(),
            CornerRadius::ZERO,
            Stroke::new(1.0, p.card_border),
            StrokeKind::Inside,
        );
        self.app.show_toasts(&ctx, &p);
    }
}

/// Строка под названием: состояние его цветом, у работающей службы — чем
/// исправлять.
fn status_line(state: State, p: &Palette) -> LayoutJob {
    let (text, color) = state_label(state, p);
    let font = FontId::proportional(12.5);
    let mut job = LayoutJob::default();
    job.append(text, 0.0, TextFormat::simple(font.clone(), color));
    if state == State::Working {
        job.append(" · ", 0.0, TextFormat::simple(font.clone(), p.weak));
        let shift = TextFormat {
            font_id: widgets::icon_font(12.5),
            color: p.weak,
            valign: Align::Center,
            ..Default::default()
        };
        job.append(&icons::ARROW_FAT_UP.repeat(2), 0.0, shift);
        job.append(" — исправить", 0.0, TextFormat::simple(font, p.weak));
    }
    job
}

/// Три плитки: раскладки, устройства, выделенный текст. Подробности — в
/// подсказке при наведении.
fn tiles(ui: &mut Ui, p: &Palette, s: &Status) {
    ui.columns(3, |cols| {
        layouts_tile(&mut cols[0], p, s);
        devices_tile(&mut cols[1], p, s);
        selection_tile(&mut cols[2], p, s);
    });
}

fn layouts_tile(ui: &mut Ui, p: &Palette, s: &Status) {
    let hint = match &s.layouts.error {
        Some(err) if s.layouts.codes.is_empty() => err.clone(),
        _ => format!("Раскладку переключает {}", s.layouts.backend),
    };
    tile(ui, p, icons::TRANSLATE, "раскладки", |ui| {
        if s.layouts.codes.is_empty() {
            ui.label(bold("?", 16.0).color(p.warning));
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            for (i, code) in s.layouts.codes.iter().enumerate() {
                widgets::chip(ui, p, code, s.layouts.current == Some(i));
            }
        });
    })
    .on_hover_text(hint);
}

fn devices_tile(ui: &mut Ui, p: &Palette, s: &Status) {
    let (keyboards, pointers) = home::device_counts(s);
    let (color, hint) = if keyboards == 0 {
        (
            p.negative,
            "Клавиатура не найдена — см. «Устройства» в окне",
        )
    } else {
        (
            p.text,
            "Клавиатуры и указатели: мыши, тачпады, экраны, перья",
        )
    };
    tile(ui, p, icons::KEYBOARD, "клав · указ", |ui| {
        ui.label(bold(format!("{keyboards} · {pointers}"), 16.0).color(color));
    })
    .on_hover_text(hint);
}

fn selection_tile(ui: &mut Ui, p: &Palette, s: &Status) {
    let (text, color, hint) = if s.selection {
        let hint = match &s.selection_via {
            Some(via) => format!("Выделенный текст исправляется через {via}"),
            None => "Выделенный текст исправляется".to_owned(),
        };
        ("вкл", p.positive, hint)
    } else {
        let hint = s.selection_note.clone();
        let hint = hint.unwrap_or_else(|| "Выделенный текст не исправляется".to_owned());
        ("выкл", p.warning, hint)
    };
    tile(ui, p, icons::SELECTION, "выделение", |ui| {
        ui.label(bold(text, 16.0).color(color));
    })
    .on_hover_text(hint);
}

/// Плитка: значок, значение (`add`) и подпись под ним.
fn tile(
    ui: &mut Ui,
    p: &Palette,
    icon: &str,
    caption: &str,
    add: impl FnOnce(&mut Ui),
) -> Response {
    Frame::NONE
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.card_border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.label(widgets::icon(icon, 16.0).color(p.accent));
            // Значение — в строке высотой с плашку раскладки: подписи у плиток
            // на одном уровне.
            let row = vec2(ui.available_width(), 24.0);
            ui.allocate_ui_with_layout(row, Layout::left_to_right(Align::Center), add);
            ui.label(RichText::new(caption).size(12.0).color(p.weak));
        })
        .response
}

/// Три последних исправления.
fn recent(ui: &mut Ui, p: &Palette, s: &Status) {
    ui.label(
        RichText::new("ПОСЛЕДНИЕ")
            .size(11.5)
            .extra_letter_spacing(0.8)
            .color(p.weak),
    );
    if s.actions.is_empty() {
        ui.label(
            RichText::new("Пока ничего не исправлено")
                .size(13.5)
                .color(p.weak),
        );
        return;
    }
    ui.spacing_mut().interact_size.y = 24.0;
    ui.spacing_mut().item_spacing.y = 6.0;
    for action in s.actions.iter().rev().take(3) {
        let (icon, what) = home::action_kind(action.kind);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(clock(action.time))
                    .monospace()
                    .size(12.0)
                    .color(p.weak),
            );
            ui.label(widgets::icon(icon, 15.0).color(p.accent));
            ui.label(
                RichText::new(format!("{what} · {} симв.", action.chars))
                    .size(13.5)
                    .color(p.text),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(bold(format!("{} → {}", action.from, action.to), 13.0).color(p.text));
            });
        });
    }
}

/// Кнопка внизу попапа: на всю ширину своей колонки. Отступы меньше
/// обычных, а подпись не переносится: «Продолжить» — в одну строку.
fn footer_button(ui: &mut Ui, p: &Palette, icon: &str, text: &str, enabled: bool) -> Response {
    ui.spacing_mut().button_padding = vec2(6.0, 4.0);
    let button = egui::Button::new(widgets::icon_job(icon, text, 13.0, p.text))
        .wrap_mode(TextWrapMode::Extend)
        .corner_radius(CornerRadius::same(9))
        .min_size(vec2(ui.available_width(), 32.0));
    ui.add_enabled(enabled, button)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_stays_on_screen_near_the_click() {
        let size = vec2(360.0, 420.0);
        let screen = vec2(1920.0, 1080.0);
        // Панель внизу, значок справа: над точкой, у правого края.
        assert_eq!(
            place(pos2(1850.0, 1060.0), size, screen),
            pos2(1560.0, 632.0)
        );
        // Панель сверху: под точкой, по центру её.
        assert_eq!(place(pos2(900.0, 12.0), size, screen), pos2(720.0, 20.0));
        // Значок слева: не левее края.
        assert_eq!(place(pos2(10.0, 12.0), size, screen), pos2(0.0, 20.0));
        // Экран меньше попапа: в левом верхнем углу.
        assert_eq!(
            place(pos2(100.0, 100.0), size, vec2(300.0, 300.0)),
            pos2(0.0, 0.0)
        );
    }
}

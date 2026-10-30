//! Главная: состояние службы, раскладки, устройства, выделенный текст, новая
//! версия, последние исправления и поле для пробы.

use eframe::egui::{
    self, Align, CornerRadius, FontId, Frame, Layout, Margin, Response, RichText, Stroke, TextEdit,
    Ui, vec2,
};
use egui_phosphor::regular as icons;
use shiftfix::devices::Role;
use shiftfix::engine::ActionKind;
use shiftfix::ipc::Status;

use crate::app::{App, Page, State, state_label};
use crate::format::clock;
use crate::link::ServiceAction;
use crate::theme::Palette;
use crate::update::{self, Outcome};
use crate::widgets::{self, bold, card, card_title, keycap};

pub fn show(app: &mut App, ui: &mut Ui, p: &Palette) {
    let state = app.state();
    let status = app.snap.status.clone();
    match &status {
        Some(status) => {
            intro(ui, p, state);
            ui.add_space(10.0);
            tiles(ui, p, status);
        }
        None => stopped(app, ui, p, state),
    }
    if let Some(Outcome::Newer(version)) = app.update.state().outcome {
        ui.add_space(10.0);
        new_version(app, ui, p, &version);
    }
    if status.as_ref().is_some_and(|s| s.tray_waiting) {
        ui.add_space(10.0);
        tray_waiting(ui, p);
    }
    if let Some(status) = &status {
        ui.add_space(14.0);
        recent(ui, p, status);
    }
    ui.add_space(14.0);
    try_it(app, ui, p);
}

/// Служба работает: как исправлять; на паузе — как включить.
fn intro(ui: &mut Ui, p: &Palette, state: State) {
    card(ui, p, |ui| {
        if state == State::Paused {
            let text = "Исправление выключено. Включите переключатель вверху или щёлкните колесом по значку в трее.";
            ui.add(egui::Label::new(RichText::new(text).size(13.5).color(p.weak)).wrap());
            return;
        }
        ui.label(
            RichText::new("Ошиблись раскладкой — дважды нажмите")
                .size(13.5)
                .color(p.weak),
        );
        ui.horizontal_wrapped(|ui| {
            keycap(ui, p, "Shift");
            keycap(ui, p, "Shift");
            ui.label(RichText::new("ещё раз — вернуть").size(13.5).color(p.weak));
        });
    });
}

/// Служба не отвечает: что с ней и что сделать.
fn stopped(app: &mut App, ui: &mut Ui, p: &Palette, state: State) {
    card(ui, p, |ui| {
        let (title, color) = state_label(state, p);
        ui.horizontal(|ui| {
            ui.label(widgets::icon(icons::WARNING_CIRCLE, 20.0).color(color));
            ui.label(bold(title, 16.0).color(p.text));
        });
        let text = match state {
            State::NotInstalled => {
                "Установите службу — она будет запускаться вместе с сеансом.".to_owned()
            }
            _ => app
                .snap
                .last_error
                .clone()
                .unwrap_or_else(|| "Запустите службу, чтобы двойной Shift заработал.".into()),
        };
        ui.add(egui::Label::new(RichText::new(text).size(13.5).color(p.weak)).wrap());
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| match state {
            State::Stopped => {
                if widgets::button(ui, p, icons::PLAY, "Запустить", true).clicked() {
                    app.service(ServiceAction::Start);
                }
                if widgets::button(ui, p, icons::SCROLL, "Журнал", false).clicked() {
                    app.page = Page::Log;
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
                    app.service(ServiceAction::Install);
                }
            }
            State::Working | State::Paused => {}
        });
    });
}

/// Проверка нашла новую версию: строка о ней и «Обновить».
fn new_version(app: &mut App, ui: &mut Ui, p: &Palette, version: &str) {
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
                    let button =
                        egui::Button::new(RichText::new("Обновить").size(13.0).color(p.on_accent))
                            .fill(p.accent)
                            .stroke(Stroke::NONE)
                            .corner_radius(CornerRadius::same(7))
                            .min_size(vec2(0.0, 26.0));
                    if ui.add(button).on_hover_text(update::BUTTON_HINT).clicked() {
                        app.run_update();
                    }
                });
            });
        });
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
    let (keyboards, pointers) = device_counts(s);
    let (color, hint) = if keyboards == 0 {
        (
            p.negative,
            "Клавиатура не найдена — см. вкладку «Устройства»",
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

/// Последние исправления, новые сверху.
fn recent(ui: &mut Ui, p: &Palette, s: &Status) {
    ui.label(
        RichText::new("ПОСЛЕДНИЕ ИСПРАВЛЕНИЯ")
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
    for action in s.actions.iter().rev().take(5) {
        let (icon, what) = action_kind(action.kind);
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

/// Поле, где можно попробовать исправление.
fn try_it(app: &mut App, ui: &mut Ui, p: &Palette) {
    ui.label(
        RichText::new("Попробуйте: наберите «ghbdtn» и дважды нажмите Shift")
            .size(13.0)
            .color(p.weak),
    );
    ui.add(
        TextEdit::singleline(&mut app.try_text)
            .hint_text("ghbdtn rfr ltkf")
            .desired_width(f32::INFINITY)
            .font(FontId::proportional(15.0))
            .margin(vec2(10.0, 8.0)),
    );
}

/// Сколько клавиатур и сколько указателей: мышей, тачпадов, экранов, перьев.
pub fn device_counts(s: &Status) -> (usize, usize) {
    let count = |roles: &[Role]| s.devices.iter().filter(|d| roles.contains(&d.role)).count();
    let keyboards = count(&[Role::Keyboard]);
    let pointers = count(&[Role::Mouse, Role::Touchpad, Role::Touchscreen, Role::Tablet]);
    (keyboards, pointers)
}

fn tray_waiting(ui: &mut Ui, p: &Palette) {
    card(ui, p, |ui| {
        card_title(ui, p, icons::HOURGLASS, "Значок в трее ждёт панель");
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Панель с системным треем (StatusNotifier) пока не появилась. Значок покажется \
                     сам, когда она запустится; исправление работает и без него. В Hyprland \
                     трей даёт, например, waybar с модулем tray; в GNOME нужно расширение \
                     AppIndicator.",
                )
                .color(p.weak),
            )
            .wrap(),
        );
    });
}

/// Значок и название исправления: «Слово», «Выделение», «Возврат».
pub fn action_kind(kind: ActionKind) -> (&'static str, &'static str) {
    match kind {
        ActionKind::Word => (icons::TEXT_AA, "Слово"),
        ActionKind::Selection => (icons::SELECTION, "Выделение"),
        ActionKind::Undo => (icons::ARROW_COUNTER_CLOCKWISE, "Возврат"),
    }
}

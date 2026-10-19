//! Главная: состояние, переключатель, раскладки, поле для пробы, последние исправления.

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Layout, Pos2, Rect, RichText, Sense,
    Stroke, StrokeKind, TextEdit, Ui, UiBuilder, vec2,
};
use egui_phosphor::regular as icons;
use shiftfix::devices::Role;
use shiftfix::engine::ActionKind;
use shiftfix::ipc::{Request, Status};

use crate::app::{App, Page, State, state_label};
use crate::format::clock;
use crate::link::ServiceAction;
use crate::theme::{self, Palette};
use crate::widgets::{self, bold, card, card_title, keycap};

pub fn show(app: &mut App, ui: &mut Ui, p: &Palette) {
    hero(app, ui, p);
    ui.add_space(16.0);
    stats(app, ui, p);
    ui.add_space(16.0);
    if app.status().is_some_and(|s| s.tray_waiting) {
        tray_waiting(ui, p);
        ui.add_space(16.0);
    }
    try_it(app, ui, p);
    ui.add_space(16.0);
    recent(app, ui, p);
}

fn hero(app: &mut App, ui: &mut Ui, p: &Palette) {
    let state = app.state();
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 168.0), Sense::hover());
    let (from, to) = match state {
        State::Working => (p.accent, p.accent2),
        State::Paused => (p.paused_from, p.paused_to),
        State::Stopped | State::NotInstalled => (p.stopped_from, p.stopped_to),
    };
    // «Чернила» заголовка: чёрные или белые — что лучше видно на этом градиенте.
    // Из них же — полупрозрачные круги, стрелки, кнопки и переключатель.
    let ink = theme::best_on(from, to);
    // Переключатель паузы — пока служба отвечает; без него текст шире.
    let show_toggle = matches!(state, State::Working | State::Paused);
    hero_backdrop(ui, rect, (from, to), ink, show_toggle);

    let inner = rect.shrink2(vec2(30.0, 24.0));
    let mut ui = ui.new_child(
        UiBuilder::new()
            .max_rect(inner)
            .layout(Layout::left_to_right(Align::Center)),
    );
    let (title, _) = state_label(state, p);
    let text = hero_text(app, state);
    ui.vertical(|ui| {
        ui.set_max_width(inner.width() - if show_toggle { 250.0 } else { 170.0 });
        ui.label(bold(title, 30.0).color(ink));
        ui.add_space(2.0);
        let soft = ink.gamma_multiply_u8(225);
        ui.add(egui::Label::new(RichText::new(text).size(15.0).color(soft)).wrap());
        ui.add_space(8.0);
        ui.horizontal(|ui| hero_actions(app, ui, state, ink));
    });
    if show_toggle {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let mut on = state == State::Working;
            if hero_toggle(ui, ink, p.accent, &mut on).changed() {
                app.link.send(Request::SetEnabled { enabled: on }, None);
            }
        });
    }
}

/// Фон заголовка: градиент, мягкие круги для объёма и две стрелки Shift.
fn hero_backdrop(
    ui: &Ui,
    rect: Rect,
    (from, to): (Color32, Color32),
    ink: Color32,
    show_toggle: bool,
) {
    widgets::gradient_rect(ui, rect, 20.0, from, to);
    let painter = ui.painter().with_clip_rect(rect);
    painter.circle_filled(
        Pos2::new(rect.right() - 170.0, rect.top() + 18.0),
        74.0,
        ink.gamma_multiply_u8(16),
    );
    painter.circle_filled(
        Pos2::new(rect.right() - 60.0, rect.bottom() - 6.0),
        52.0,
        ink.gamma_multiply_u8(12),
    );
    let decor_x = rect.right() - if show_toggle { 196.0 } else { 120.0 };
    for (dx, alpha) in [(-16.0f32, 80u8), (16.0, 52)] {
        painter.text(
            Pos2::new(decor_x + dx, rect.center().y),
            Align2::CENTER_CENTER,
            icons::ARROW_FAT_UP,
            widgets::icon_font(50.0),
            ink.gamma_multiply_u8(alpha),
        );
    }
}

/// Пояснение под заголовком: что сейчас происходит и что делать.
fn hero_text(app: &App, state: State) -> String {
    match state {
        State::Working => "Дважды нажмите Shift — последнее слово или выделенный текст перейдут в другую раскладку.".into(),
        State::Paused => "Исправление выключено. Включите переключатель справа или щёлкните колесом по значку в трее.".into(),
        State::Stopped => app
            .snap
            .last_error
            .clone()
            .unwrap_or_else(|| "Запустите службу, чтобы двойной Shift заработал.".into()),
        State::NotInstalled => {
            "Установите службу — она будет запускаться вместе с сеансом.".into()
        }
    }
}

/// Кнопки под пояснением, когда служба не работает.
fn hero_actions(app: &mut App, ui: &mut Ui, state: State, ink: Color32) {
    match state {
        State::Stopped => {
            if hero_button(ui, ink, icons::PLAY, "Запустить").clicked() {
                app.service(ServiceAction::Start);
            }
            if hero_button(ui, ink, icons::SCROLL, "Журнал").clicked() {
                app.page = Page::Log;
            }
        }
        State::NotInstalled => {
            if hero_button(ui, ink, icons::DOWNLOAD_SIMPLE, "Установить и запустить").clicked()
            {
                app.service(ServiceAction::Install);
            }
        }
        State::Working | State::Paused => {}
    }
}

fn hero_button(ui: &mut Ui, ink: Color32, icon: &str, text: &str) -> egui::Response {
    let label = widgets::icon_job(icon, text, 15.0, ink);
    ui.add(
        egui::Button::new(label)
            .fill(ink.gamma_multiply_u8(46))
            .stroke(Stroke::new(1.0, ink.gamma_multiply_u8(90)))
            .corner_radius(CornerRadius::same(10))
            .min_size(vec2(0.0, 36.0)),
    )
}

/// Крупный переключатель для цветного заголовка: дорожка цвета «чернил»,
/// ручка выключенного — противоположного цвета, включённого — акцентом.
fn hero_toggle(ui: &mut Ui, ink: Color32, accent: Color32, on: &mut bool) -> egui::Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(84.0, 44.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(response.id, *on);
    let radius = rect.height() / 2.0;
    let track = widgets::lerp_color(ink.gamma_multiply_u8(70), ink, t);
    ui.painter()
        .rect_filled(rect, CornerRadius::from(radius), track);
    if response.hovered() {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::from(radius),
            Stroke::new(2.0, ink.gamma_multiply_u8(160)),
            StrokeKind::Outside,
        );
    }
    let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
    let knob_off = if ink == Color32::BLACK {
        Color32::WHITE
    } else {
        Color32::BLACK
    };
    let knob_color = widgets::lerp_color(knob_off, accent, t);
    ui.painter().circle_filled(
        Pos2::new(x, rect.center().y + 1.0),
        radius - 5.0,
        Color32::from_black_alpha(45),
    );
    ui.painter()
        .circle_filled(Pos2::new(x, rect.center().y), radius - 5.5, knob_color);
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Включить или поставить на паузу")
}

/// Карточка-показатель; пока служба не отвечает — прочерк вместо `add`.
fn stat_card(
    ui: &mut Ui,
    p: &Palette,
    icon: &str,
    title: &str,
    status: Option<&Status>,
    add: fn(&mut Ui, &Palette, &Status),
) {
    card(ui, p, |ui| {
        ui.set_min_height(92.0);
        ui.horizontal(|ui| {
            ui.label(widgets::icon(icon, 16.0).color(p.accent));
            ui.label(RichText::new(title).size(13.0).color(p.weak));
        });
        ui.add_space(4.0);
        match status {
            Some(status) => add(ui, p, status),
            None => {
                ui.label(RichText::new("—").size(20.0).color(p.weak));
            }
        }
    });
}

fn chip(ui: &mut Ui, p: &Palette, text: &str, active: bool) {
    let font = FontId::proportional(14.0);
    let color = if active { p.on_accent } else { p.text };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let (rect, _) = ui.allocate_exact_size(galley.size() + vec2(18.0, 8.0), Sense::hover());
    let fill = if active { p.accent } else { p.hover };
    ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
}

fn stats(app: &App, ui: &mut Ui, p: &Palette) {
    let status = app.status();
    ui.columns(3, |cols| {
        stat_card(
            &mut cols[0],
            p,
            icons::TRANSLATE,
            "Раскладки",
            status,
            layouts_stat,
        );
        stat_card(
            &mut cols[1],
            p,
            icons::KEYBOARD,
            "Устройства",
            status,
            devices_stat,
        );
        stat_card(
            &mut cols[2],
            p,
            icons::SELECTION,
            "Выделенный текст",
            status,
            selection_stat,
        );
    });
}

fn layouts_stat(ui: &mut Ui, p: &Palette, s: &Status) {
    if s.layouts.codes.is_empty() {
        ui.label(RichText::new("не удалось узнать").color(p.warning));
        if let Some(err) = &s.layouts.error {
            ui.label(RichText::new(err).size(12.0).color(p.weak));
        }
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for (i, code) in s.layouts.codes.iter().enumerate() {
            chip(ui, p, code, s.layouts.current == Some(i));
        }
    });
    ui.label(
        RichText::new(format!("переключает {}", s.layouts.backend))
            .size(12.5)
            .color(p.weak),
    );
}

fn devices_stat(ui: &mut Ui, p: &Palette, s: &Status) {
    let count = |roles: &[Role]| s.devices.iter().filter(|d| roles.contains(&d.role)).count();
    let keyboards = count(&[Role::Keyboard]);
    let pointers = count(&[Role::Mouse, Role::Touchpad, Role::Touchscreen, Role::Tablet]);
    let color = if keyboards == 0 { p.negative } else { p.text };
    ui.label(bold(format!("{keyboards} клав. · {pointers} указ."), 18.0).color(color));
    let note = if keyboards == 0 {
        "клавиатура не найдена — см. «Устройства»"
    } else {
        "клавиатуры и указатели"
    };
    ui.label(RichText::new(note).size(12.5).color(p.weak));
}

fn selection_stat(ui: &mut Ui, p: &Palette, s: &Status) {
    if !s.selection {
        ui.label(bold("Недоступно", 18.0).color(p.warning));
        if let Some(note) = &s.selection_note {
            ui.add(egui::Label::new(RichText::new(note).size(12.0).color(p.weak)).wrap());
        }
        return;
    }
    ui.label(bold("Исправляется", 18.0).color(p.positive));
    let tables = if s.layouts.from_keymap {
        "таблицы из вашей keymap"
    } else {
        "встроенные таблицы us/ru"
    };
    let note = match &s.selection_via {
        Some(via) => format!("через {via} · {tables}"),
        None => tables.to_owned(),
    };
    ui.label(RichText::new(note).size(12.5).color(p.weak));
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

fn try_it(app: &mut App, ui: &mut Ui, p: &Palette) {
    card(ui, p, |ui| {
        card_title(ui, p, icons::PENCIL_SIMPLE, "Попробуйте здесь");
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Наберите «ghbdtn» и дважды нажмите").color(p.weak));
            keycap(ui, p, "Shift");
            keycap(ui, p, "Shift");
            ui.label(
                RichText::new("— получится «привет». Ещё раз — вернётся обратно.").color(p.weak),
            );
        });
        ui.add_space(6.0);
        ui.add(
            TextEdit::multiline(&mut app.try_text)
                .hint_text("ghbdtn rfr ltkf")
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .font(FontId::proportional(17.0))
                .margin(vec2(12.0, 10.0)),
        );
    });
}

fn recent(app: &mut App, ui: &mut Ui, p: &Palette) {
    let actions = app.status().map(|s| s.actions.clone()).unwrap_or_default();
    card(ui, p, |ui| {
        card_title(
            ui,
            p,
            icons::CLOCK_COUNTER_CLOCKWISE,
            "Последние исправления",
        );
        if actions.is_empty() {
            widgets::empty_state(ui, p, icons::SPARKLE, "Пока ничего не исправлено");
            return;
        }
        for action in actions.iter().rev().take(8) {
            let (icon, what) = match action.kind {
                ActionKind::Word => (icons::TEXT_AA, "Слово"),
                ActionKind::Selection => (icons::SELECTION, "Выделение"),
                ActionKind::Undo => (icons::ARROW_COUNTER_CLOCKWISE, "Возврат"),
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(clock(action.time)).monospace().color(p.weak));
                ui.label(widgets::icon(icon, 17.0).color(p.accent));
                ui.label(RichText::new(format!("{what}, {} симв.", action.chars)).color(p.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        bold(format!("{}  →  {}", action.from, action.to), 14.0).color(p.text),
                    );
                });
            });
        }
    });
}

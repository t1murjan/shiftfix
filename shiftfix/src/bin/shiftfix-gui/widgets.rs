//! Свои виджеты: карточки, переключатель, клавиши, бейджи, градиент.

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{
    self, Align, Color32, CornerRadius, FontFamily, FontId, Frame, Layout, Margin, Mesh, Pos2,
    Rect, Response, RichText, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, vec2,
};

use crate::theme::{self, BOLD, Palette};

/// Семейство шрифта с иконками Phosphor.
pub const ICONS: &str = "phosphor";

pub fn icon_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(ICONS.into()))
}

pub fn icon(glyph: &str, size: f32) -> RichText {
    RichText::new(glyph)
        .family(FontFamily::Name(ICONS.into()))
        .size(size)
}

/// Иконка и текст одной строкой — для кнопок и пунктов меню.
pub fn icon_job(glyph: &str, text: &str, size: f32, color: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let format = |font_id| TextFormat {
        font_id,
        color,
        valign: egui::Align::Center,
        ..Default::default()
    };
    job.append(glyph, 0.0, format(icon_font(size + 1.0)));
    job.append(
        text,
        if text.is_empty() { 0.0 } else { 8.0 },
        format(FontId::proportional(size)),
    );
    job
}

pub fn bold(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text)
        .family(FontFamily::Name(BOLD.into()))
        .size(size)
}

/// Заголовок страницы с подзаголовком.
pub fn page_header(ui: &mut Ui, p: &Palette, icon: &str, title: &str, subtitle: &str) {
    ui.horizontal(|ui| {
        ui.label(self::icon(icon, 26.0).color(p.accent));
        ui.label(bold(title, 26.0).color(p.text));
    });
    if !subtitle.is_empty() {
        ui.label(RichText::new(subtitle).color(p.weak));
    }
    ui.add_space(10.0);
}

/// Карточка: фон, рамка, скругление, внутренние отступы.
pub fn card<R>(ui: &mut Ui, p: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::NONE
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.card_border))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Заголовок внутри карточки.
pub fn card_title(ui: &mut Ui, p: &Palette, icon: &str, title: &str) {
    ui.horizontal(|ui| {
        ui.label(self::icon(icon, 18.0).color(p.accent));
        ui.label(bold(title, 17.0).color(p.text));
    });
    ui.add_space(4.0);
}

/// Переключатель в стиле iOS/Breeze.
pub fn toggle(ui: &mut Ui, on: &mut bool, height: f32, p: &Palette) -> Response {
    let size = vec2(height * 1.85, height);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(response.id, *on);
        let fill = lerp_color(p.toggle_off, p.accent, t);
        let radius = rect.height() / 2.0;
        ui.painter()
            .rect_filled(rect, CornerRadius::from(radius), fill);
        if response.hovered() {
            ui.painter().rect_stroke(
                rect,
                CornerRadius::from(radius),
                Stroke::new(1.0, p.accent),
                StrokeKind::Outside,
            );
        }
        let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
        let knob = Pos2::new(x, rect.center().y);
        ui.painter().circle_filled(
            knob + vec2(0.0, 1.0),
            radius - 3.0,
            Color32::from_black_alpha(40),
        );
        let knob_color = lerp_color(p.knob, theme::knob_on(p), t);
        ui.painter().circle_filled(knob, radius - 3.5, knob_color);
    }
    response
}

/// Сегмент «одно из нескольких»: иконка и подпись на каждой кнопке.
/// Возвращает нажатое, в том числе уже выбранное: повторный выбор — тоже
/// выбор (так режим из сегмента отменяет `--theme`, как и боковая панель).
pub fn segment<T: Copy + PartialEq>(
    ui: &mut Ui,
    p: &Palette,
    current: T,
    items: &[(T, &str, &str)],
) -> Option<T> {
    let mut picked = None;
    Frame::NONE
        .fill(p.field)
        .stroke(Stroke::new(1.0, p.card_border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                // Справа налево (в правой колонке строки) — с конца, чтобы
                // порядок на экране был тот же.
                let rtl = ui.layout().main_dir() == egui::Direction::RightToLeft;
                let mut order: Vec<_> = items.iter().collect();
                if rtl {
                    order.reverse();
                }
                for &(value, glyph, text) in order {
                    let selected = value == current;
                    let color = if selected { p.accent } else { p.text };
                    let button = egui::Button::new(icon_job(glyph, text, 14.0, color))
                        .fill(if selected {
                            p.accent.gamma_multiply(if p.dark { 0.22 } else { 0.14 })
                        } else {
                            Color32::TRANSPARENT
                        })
                        .stroke(Stroke::NONE)
                        .corner_radius(CornerRadius::same(8))
                        .min_size(vec2(0.0, 28.0));
                    if ui.add(button).clicked() {
                        picked = Some(value);
                    }
                }
            });
        });
    picked
}

/// Вкладка: подпись с чертой снизу у выбранной.
pub fn tab(ui: &mut Ui, p: &Palette, text: &str, selected: bool) -> Response {
    let font = FontId::proportional(15.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER);
    let size = galley.size() + vec2(20.0, 14.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let color = if selected {
        p.accent
    } else if response.hovered() {
        p.text
    } else {
        p.weak
    };
    if response.hovered() && !selected {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(8), p.hover);
    }
    let pos = rect.center() - galley.size() / 2.0 - vec2(0.0, 2.0);
    ui.painter().galley(pos, galley, color);
    if selected {
        let bar = Rect::from_min_max(
            Pos2::new(rect.left() + 8.0, rect.bottom() - 3.0),
            Pos2::new(rect.right() - 8.0, rect.bottom()),
        );
        ui.painter()
            .rect_filled(bar, CornerRadius::same(2), p.accent);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Плитка палитры: уменьшенное окно в её цветах (`preview`) и имя под ним.
/// Рамка у выбранной — акцентом окна (`p`).
pub fn palette_tile(
    ui: &mut Ui,
    p: &Palette,
    preview: &Palette,
    name: &str,
    selected: bool,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(132.0, 104.0), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();
    let shot = Rect::from_min_size(rect.min, vec2(rect.width(), 76.0));
    let r8 = CornerRadius::same(8);
    painter.rect_filled(shot, r8, preview.bg);
    // Боковая панель с выбранным пунктом.
    let side = Rect::from_min_size(shot.min, vec2(30.0, shot.height()));
    painter.rect_filled(
        side,
        CornerRadius {
            nw: 8,
            sw: 8,
            ne: 0,
            se: 0,
        },
        preview.sidebar,
    );
    let nav = Rect::from_min_size(side.min + vec2(5.0, 10.0), vec2(20.0, 7.0));
    painter.rect_filled(
        nav,
        CornerRadius::same(3),
        preview.accent.gamma_multiply(0.4),
    );
    for i in 0..2 {
        let item = Rect::from_min_size(
            side.min + vec2(5.0, 24.0 + 11.0 * i as f32),
            vec2(16.0, 4.0),
        );
        painter.rect_filled(item, CornerRadius::same(2), preview.weak);
    }
    // Карточка: заголовок, текст, кнопка и переключатель.
    let card = Rect::from_min_max(shot.min + vec2(38.0, 9.0), shot.max - vec2(8.0, 9.0));
    painter.rect(
        card,
        CornerRadius::same(6),
        preview.card,
        Stroke::new(1.0, preview.card_border),
        StrokeKind::Inside,
    );
    let line = |x: f32, y: f32, w: f32, color: Color32| {
        let r = Rect::from_min_size(card.min + vec2(x, y), vec2(w, 4.0));
        painter.rect_filled(r, CornerRadius::same(2), color);
    };
    painter.circle_filled(card.min + vec2(11.0, 11.0), 3.5, preview.accent);
    line(19.0, 9.0, 36.0, preview.text);
    line(8.0, 20.0, 62.0, preview.weak);
    line(8.0, 28.0, 48.0, preview.weak);
    let button = Rect::from_min_size(card.min + vec2(8.0, 39.0), vec2(30.0, 12.0));
    painter.rect_filled(button, CornerRadius::same(4), preview.accent);
    let label = Rect::from_center_size(button.center(), vec2(16.0, 3.0));
    painter.rect_filled(label, CornerRadius::same(1), preview.on_accent);
    let track = Rect::from_min_size(card.min + vec2(52.0, 40.0), vec2(20.0, 11.0));
    painter.rect_filled(track, CornerRadius::same(6), preview.accent);
    painter.circle_filled(
        track.right_center() - vec2(5.5, 0.0),
        4.0,
        theme::knob_on(preview),
    );

    let border = if selected {
        Stroke::new(2.5, p.accent)
    } else if response.hovered() {
        Stroke::new(1.5, p.accent.gamma_multiply(0.6))
    } else {
        Stroke::new(1.0, p.card_border)
    };
    painter.rect_stroke(shot, r8, border, StrokeKind::Outside);
    let galley = if selected {
        painter.layout_job(icon_job(
            egui_phosphor::regular::CHECK,
            name,
            14.0,
            p.accent,
        ))
    } else {
        painter.layout_no_wrap(name.to_owned(), FontId::proportional(14.0), p.text)
    };
    let pos = Pos2::new(rect.center().x - galley.size().x / 2.0, shot.bottom() + 6.0);
    painter.galley(pos, galley, p.text);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Строка «подпись слева — переключатель справа».
pub fn toggle_row(ui: &mut Ui, p: &Palette, on: &mut bool, title: &str, hint: &str) -> Response {
    let mut response = None;
    ui.horizontal(|ui| {
        let left = (ui.available_width() - 80.0).max(180.0);
        ui.allocate_ui_with_layout(
            vec2(left, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(left);
                ui.label(RichText::new(title).color(p.text));
                if !hint.is_empty() {
                    ui.add(egui::Label::new(RichText::new(hint).size(12.5).color(p.weak)).wrap());
                }
            },
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            response = Some(toggle(ui, on, 24.0, p));
        });
    });
    response.expect("toggle shown")
}

/// Изображение клавиши: «Shift», «Ctrl».
pub fn keycap(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    let font = FontId::new(13.0, FontFamily::Monospace);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, p.text);
    let size = galley.size() + vec2(16.0, 10.0);
    let (rect, response) = ui.allocate_exact_size(vec2(size.x.max(28.0), size.y), Sense::hover());
    let radius = CornerRadius::same(6);
    ui.painter()
        .rect_filled(rect.translate(vec2(0.0, 2.0)), radius, p.key_edge);
    ui.painter().rect(
        rect,
        radius,
        p.key_face,
        Stroke::new(1.0, p.key_edge),
        StrokeKind::Inside,
    );
    let pos = rect.center() - galley.size() / 2.0;
    ui.painter().galley(pos, galley, p.text);
    response
}

/// Бейдж с цветной точкой: «работает», «на паузе».
pub fn pill(ui: &mut Ui, text: &str, color: Color32, fill: Color32) -> Response {
    let font = FontId::new(13.0, FontFamily::Proportional);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let size = vec2(galley.size().x + 30.0, 24.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(12), fill);
    ui.painter()
        .circle_filled(Pos2::new(rect.left() + 12.0, rect.center().y), 4.0, color);
    ui.painter().galley(
        Pos2::new(rect.left() + 22.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    response
}

/// Точка активности устройства: вспыхивает при событии и плавно гаснет.
pub fn activity_dot(ui: &mut Ui, p: &Palette, idle_ms: Option<u64>) {
    let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
    let glow = idle_ms.map_or(0.0, |ms| (1.0 - ms as f32 / 1500.0).clamp(0.0, 1.0));
    let color = lerp_color(p.dot_idle, p.positive, glow);
    if glow > 0.0 {
        ui.painter()
            .circle_filled(rect.center(), 7.0, p.positive.gamma_multiply(0.25 * glow));
        ui.ctx().request_repaint();
    }
    ui.painter().circle_filled(rect.center(), 4.5, color);
}

/// Смесь цветов в гамма-пространстве. Компоненты `Color32` уже умножены на альфу,
/// поэтому и результат собирается как умноженный: полупрозрачный белый остаётся
/// белым, а не темнеет. Для непрозрачных цветов разницы нет.
pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        mix(a.r(), b.r()),
        mix(a.g(), b.g()),
        mix(a.b(), b.b()),
        mix(a.a(), b.a()),
    )
}

/// Скруглённый прямоугольник с диагональным градиентом и мягким краем.
pub fn gradient_rect(ui: &Ui, rect: Rect, radius: f32, from: Color32, to: Color32) {
    let r = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let corners = [
        (Pos2::new(rect.right() - r, rect.top() + r), -90.0_f32),
        (Pos2::new(rect.right() - r, rect.bottom() - r), 0.0),
        (Pos2::new(rect.left() + r, rect.bottom() - r), 90.0),
        (Pos2::new(rect.left() + r, rect.top() + r), 180.0),
    ];
    let mut outline: Vec<(Pos2, Vec2)> = Vec::new();
    for (center, start) in corners {
        for i in 0..=10 {
            let angle = (start + 90.0 * i as f32 / 10.0).to_radians();
            let normal = vec2(angle.cos(), angle.sin());
            outline.push((center + normal * r, normal));
        }
    }
    let color_at = |pos: Pos2| {
        let t = 0.75 * (pos.x - rect.left()) / rect.width()
            + 0.25 * (pos.y - rect.top()) / rect.height();
        lerp_color(from, to, t)
    };
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.center(), color_at(rect.center()));
    for (pos, _) in &outline {
        mesh.colored_vertex(*pos, color_at(*pos));
    }
    let n = outline.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    // Полупиксельная «бахрома» снаружи — сглаживание края.
    let feather = 1.0 / ui.ctx().pixels_per_point();
    for (pos, normal) in &outline {
        mesh.colored_vertex(*pos + *normal * feather, color_at(*pos).gamma_multiply(0.0));
    }
    for i in 0..n {
        let (a, b) = (1 + i, 1 + (i + 1) % n);
        let (c, d) = (1 + n + i, 1 + n + (i + 1) % n);
        mesh.add_triangle(a, b, d);
        mesh.add_triangle(a, d, c);
    }
    ui.painter().add(Shape::mesh(mesh));
}

/// Крупная кнопка с иконкой; `primary` — залитая акцентным цветом.
pub fn button(ui: &mut Ui, p: &Palette, icon: &str, text: &str, primary: bool) -> Response {
    let label = icon_job(icon, text, 15.0, if primary { p.on_accent } else { p.text });
    let mut b = egui::Button::new(label)
        .corner_radius(CornerRadius::same(9))
        .min_size(vec2(0.0, 34.0));
    if primary {
        b = b.fill(p.accent).stroke(Stroke::NONE);
    }
    ui.add(b)
}

/// Пустое состояние: иконка и пояснение по центру.
pub fn empty_state(ui: &mut Ui, p: &Palette, icon: &str, text: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(8.0);
        ui.label(self::icon(icon, 30.0).color(p.weak));
        ui.label(RichText::new(text).color(p.weak));
        ui.add_space(8.0);
    });
}

/// Строка настройки: название и пояснение слева, элемент управления справа.
pub fn setting_row(
    ui: &mut Ui,
    p: &Palette,
    title: &str,
    hint: &str,
    right: f32,
    add: impl FnOnce(&mut Ui),
) {
    ui.horizontal(|ui| {
        let left = (ui.available_width() - right - 16.0).max(180.0);
        ui.allocate_ui_with_layout(egui::vec2(left, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.set_width(left);
            ui.label(RichText::new(title).color(p.text));
            if !hint.is_empty() {
                ui.add(egui::Label::new(RichText::new(hint).size(12.5).color(p.weak)).wrap());
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), add);
    });
    ui.add_space(4.0);
}

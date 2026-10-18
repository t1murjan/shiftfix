//! Карточка «Оформление»: режим темы, палитры вариантов и свой акцент.

//! Настройки: правка config.toml с сохранением комментариев.

use eframe::egui::color_picker::{self, Alpha};
use eframe::egui::{
    self, Align, CornerRadius, Frame, Id, Layout, Margin, RichText, Stroke, StrokeKind, TextStyle,
    Ui,
};
use egui_phosphor::regular as icons;
use shiftfix::config::ThemeMode;

use crate::app::App;
use crate::portal::PortalState;
use crate::theme::{self, HexCommit, Palette, SystemNote, Variant};
use crate::widgets::{self, card, card_title, setting_row, toggle_row};

const MODES: [(ThemeMode, &str, &str); 3] = [
    (ThemeMode::Light, icons::SUN, "Светлая"),
    (ThemeMode::Dark, icons::MOON, "Тёмная"),
    (ThemeMode::System, icons::CIRCLE_HALF, "Системная"),
];

const VARIANTS: [(Variant, &str); 2] = [
    (Variant::Light, "Светлый вариант"),
    (Variant::Dark, "Тёмный вариант"),
];

/// Набранный в поле hex цвет, пока он не принят (Enter или уход из поля).
#[derive(Clone)]
struct HexDraft {
    text: String,
    /// Не цвет «#rrggbb» — поле подсвечено.
    bad: bool,
}

fn hex_field_id(v: Variant) -> Id {
    Id::new("theme_accent_hex").with(v)
}

fn hex_draft_id(v: Variant) -> Id {
    hex_field_id(v).with("draft")
}

/// Цвет сменился не из поля — поле снова показывает сохранённый.
fn forget_hex_drafts(ui: &Ui) {
    ui.data_mut(|d| {
        for (v, _) in VARIANTS {
            d.remove::<HexDraft>(hex_draft_id(v));
        }
    });
}

/// Строка о теме системы под выбором режима.
fn note_line(
    ui: &mut Ui,
    p: &Palette,
    glyph: &str,
    color: egui::Color32,
    text: &str,
) -> egui::Response {
    ui.horizontal_wrapped(|ui| {
        // Строка ниже обычной строки виджетов: это подпись, а не кнопка.
        ui.spacing_mut().interact_size.y = 18.0;
        ui.label(widgets::icon(glyph, 14.0).color(color));
        ui.label(RichText::new(text).size(12.5).color(p.weak))
    })
    .inner
}

/// Карточка «Оформление» на вкладке «Настройки»: всё меняется сразу и само
/// пишется в `[theme]` — без «Сохранить» и «Отменить».
pub fn show(app: &mut App, ui: &mut Ui, p: &Palette) {
    card(ui, p, |ui| {
        card_title(ui, p, icons::PALETTE, "Оформление");
        let mode = app.theme_mode();
        setting_row(
            ui,
            p,
            "Тема",
            "Применяется и сохраняется сразу.",
            330.0,
            |ui| {
                if let Some(mode) = widgets::segment(ui, p, mode, &MODES) {
                    app.set_mode(mode);
                }
            },
        );
        if app.theme.forced() {
            note_line(
                ui,
                p,
                icons::INFO,
                p.weak,
                "Сейчас тема задана параметром --theme — только до закрытия окна.",
            );
        } else if mode == ThemeMode::System {
            let note = SystemNote::of(&app.theme.portal);
            let (glyph, color) = match note {
                SystemNote::Waiting => (icons::HOURGLASS_MEDIUM, p.weak),
                note if note.fallback() => (icons::WARNING, p.warning),
                _ => (icons::INFO, p.weak),
            };
            let line = note_line(ui, p, glyph, color, note.text());
            if let PortalState::Unavailable(why) = &app.theme.portal {
                line.on_hover_text(why);
            }
        }
        ui.add_space(10.0);

        let variant = app.theme.tab.unwrap_or(Variant::of(p.dark));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (v, title) in VARIANTS {
                if widgets::tab(ui, p, title, v == variant).clicked() {
                    app.theme.tab = Some(v);
                }
            }
        });
        ui.add_space(10.0);

        // Плитки — по списку палитр: новая палитра появится здесь сама.
        let t = app.settings.current.theme.clone();
        let chosen = theme::find(variant.palette_id(&t)).id;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(14.0, 12.0);
            for def in theme::PALETTES {
                let preview = variant.of_def(def);
                if widgets::palette_tile(ui, p, preview, def.name, def.id == chosen).clicked() {
                    app.set_theme(|t| *variant.palette_id_mut(t) = def.id.to_owned());
                }
            }
        });
        ui.add_space(12.0);

        let system_accent = theme::system_accent_active(&t, &app.theme.system, mode);
        if mode == ThemeMode::System {
            let blocked = theme::system_accent_blocked(&app.theme.portal);
            let mut on = t.system_accent;
            let hint = blocked.unwrap_or("Акцент из настроек системы вместо своего.");
            let toggled = ui
                .add_enabled_ui(blocked.is_none(), |ui| {
                    toggle_row(ui, p, &mut on, "Акцент как в системе", hint)
                })
                .inner
                .on_disabled_hover_text(hint);
            if toggled.changed() {
                app.set_theme(|t| t.system_accent = on);
            }
        }

        let own = variant.accent(&t);
        let mut own_on = !own.trim().is_empty();
        let base = variant.of_def(theme::find(variant.palette_id(&t))).accent;
        let hint = if system_accent {
            "Сейчас действует акцент системы."
        } else {
            "Цвет кнопок, ссылок и значков вместо цвета палитры."
        };
        let toggled = ui
            .add_enabled_ui(!system_accent, |ui| {
                toggle_row(ui, p, &mut own_on, "Свой акцент", hint)
            })
            .inner;
        if toggled.changed() {
            app.set_theme(|t| theme::set_own_accent(t, variant, own_on));
            forget_hex_drafts(ui);
        }

        let shown = theme::variant_palette(&t, &app.theme.system, mode, variant);
        let mut message = None;
        ui.horizontal(|ui| {
            ui.add_enabled_ui(own_on && !system_accent, |ui| {
                let mut color = theme::parse_hex(own).unwrap_or(base);
                if color_picker::color_edit_button_srgba(ui, &mut color, Alpha::Opaque)
                    .on_hover_text("Выбрать цвет")
                    .changed()
                {
                    let hex = theme::format_hex(color);
                    app.set_theme(|t| *variant.accent_mut(t) = hex);
                    forget_hex_drafts(ui);
                }
                message = hex_field(app, ui, p, variant, own_on.then_some(own), base);
            });
            ui.add_space(8.0);
            // Образец кнопки — каким будет акцент этого варианта.
            Frame::NONE
                .fill(shown.accent)
                .corner_radius(CornerRadius::same(9))
                .inner_margin(Margin::symmetric(14, 7))
                .show(ui, |ui| {
                    ui.label(widgets::icon_job(
                        icons::CHECK,
                        "Образец",
                        14.0,
                        shown.on_accent,
                    ));
                });
        });
        if let Some(text) = message {
            note_line(ui, p, icons::WARNING_CIRCLE, p.negative, text);
        }
        if !theme::accent_readable(&shown) {
            let text = if system_accent {
                "Акцент системы плохо виден на фоне карточек."
            } else if shown.dark {
                "Акцент плохо виден на фоне карточек — возьмите цвет светлее."
            } else {
                "Акцент плохо виден на фоне карточек — возьмите цвет темнее."
            };
            note_line(ui, p, icons::WARNING, p.warning, text);
        }
        ui.add_space(6.0);

        let mut reset = t.clone();
        theme::reset_variant(&mut reset, variant);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let clicked = ui
                .add_enabled_ui(reset != t, |ui| {
                    widgets::button(ui, p, icons::ARROW_COUNTER_CLOCKWISE, "Сбросить", false)
                })
                .inner
                .on_hover_text("Палитра и акцент этого варианта — по умолчанию; режим не меняется.")
                .clicked();
            if clicked {
                app.set_theme(|t| theme::reset_variant(t, variant));
                forget_hex_drafts(ui);
            }
        });
    });
}

/// Поле «#rrggbb» своего акцента: цвет принимается по Enter или при уходе из
/// поля, Esc — отмена. `own` — свой акцент (`None` — выключен, в поле акцент
/// палитры `base`). Возвращает текст ошибки под строкой.
fn hex_field(
    app: &mut App,
    ui: &mut Ui,
    p: &Palette,
    variant: Variant,
    own: Option<&str>,
    base: egui::Color32,
) -> Option<&'static str> {
    let draft_id = hex_draft_id(variant);
    let draft: Option<HexDraft> = ui.data(|d| d.get_temp(draft_id));
    let stored_bad = own.is_some_and(|own| theme::parse_hex(own).is_none());
    let mut text = match (&draft, own) {
        (Some(draft), _) => draft.text.clone(),
        (None, Some(own)) => own.trim().to_owned(),
        (None, None) => theme::format_hex(base),
    };
    let bad = draft.as_ref().map_or(stored_bad, |d| d.bad);
    let mut edit = egui::TextEdit::singleline(&mut text)
        .id(hex_field_id(variant))
        .desired_width(88.0)
        .char_limit(9)
        .font(TextStyle::Monospace)
        .hint_text("#rrggbb");
    if bad {
        edit = edit.text_color(p.negative);
    }
    let response = ui.add(edit);
    if response.changed() {
        let draft = HexDraft {
            text: text.clone(),
            bad: false,
        };
        ui.data_mut(|d| d.insert_temp(draft_id, draft));
    }
    if response.lost_focus() {
        // Черновика нет — цвет сменили кнопкой («Сбросить», «Свой акцент»,
        // пипетка) в том же клике, которым ушли из поля: в поле уже
        // сохранённый цвет, записывать его обратно нельзя.
        let edited = draft.is_some() || response.changed();
        let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
        match theme::hex_commit(edited, ui.is_enabled(), escape, &text) {
            HexCommit::Keep => {}
            HexCommit::Discard => ui.data_mut(|d| d.remove::<HexDraft>(draft_id)),
            HexCommit::Apply(color) => {
                let hex = theme::format_hex(color);
                app.set_theme(|t| *variant.accent_mut(t) = hex);
                ui.data_mut(|d| d.remove::<HexDraft>(draft_id));
            }
            HexCommit::Bad => {
                let draft = HexDraft { text, bad: true };
                ui.data_mut(|d| d.insert_temp(draft_id, draft));
            }
        }
    }
    // Поле неактивно (действует акцент системы) — об ошибке не говорим.
    if !bad || !ui.is_enabled() {
        return None;
    }
    ui.painter().rect_stroke(
        response.rect,
        CornerRadius::same(4),
        Stroke::new(1.5, p.negative),
        StrokeKind::Outside,
    );
    Some(if draft.is_some() {
        "Нужен цвет вида #ff8800."
    } else {
        "В config.toml неверный цвет — используется акцент палитры."
    })
}

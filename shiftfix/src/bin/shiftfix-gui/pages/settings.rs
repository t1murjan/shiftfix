//! Настройки: правка config.toml с сохранением комментариев.

use crate::portal::PortalState;
use eframe::egui::color_picker::{self, Alpha};
use eframe::egui::{
    self, Align, CornerRadius, Frame, Id, Layout, Margin, RichText, Slider, Stroke, StrokeKind,
    TextStyle, Ui,
};
use egui_phosphor::regular as icons;
use shiftfix::config::{Config, Settings, ThemeMode};

use crate::app::{App, ToastKind, shown_path};
use crate::link::ServiceAction;
use crate::theme::{self, HexCommit, Palette, SystemNote, Variant};
use crate::widgets::{self, card, card_title, page_header, toggle_row};

const PASTE_KEYS: [(&str, &str); 3] = [
    ("KEY_LEFTCTRL+KEY_V", "Ctrl+V — почти везде"),
    ("KEY_LEFTSHIFT+KEY_INSERT", "Shift+Insert — и в терминалах"),
    (
        "KEY_LEFTCTRL+KEY_LEFTSHIFT+KEY_V",
        "Ctrl+Shift+V — терминалы",
    ),
];

const HOTKEYS: [(&str, &str); 5] = [
    ("KEY_LEFTMETA+KEY_SPACE", "Super+Space (GNOME)"),
    ("KEY_LEFTALT+KEY_LEFTSHIFT", "Alt+Shift"),
    ("KEY_LEFTCTRL+KEY_LEFTSHIFT", "Ctrl+Shift"),
    ("KEY_CAPSLOCK", "Caps Lock"),
    ("KEY_LEFTMETA+KEY_LEFTALT+KEY_K", "Meta+Alt+K (KDE)"),
];

const BACKENDS: [(&str, &str); 7] = [
    ("auto", "Автоматически"),
    ("kde", "KDE Plasma (KWin)"),
    ("gnome", "GNOME"),
    ("hyprland", "Hyprland"),
    ("sway", "Sway"),
    ("x11", "X11 (XKB)"),
    ("hotkey", "Нажатием сочетания клавиш"),
];

/// Строка настройки: название и пояснение слева, элемент управления справа.
fn row(ui: &mut Ui, p: &Palette, title: &str, hint: &str, right: f32, add: impl FnOnce(&mut Ui)) {
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

fn slider_row(
    ui: &mut Ui,
    p: &Palette,
    value: &mut u64,
    range: std::ops::RangeInclusive<u64>,
    title: &str,
    hint: &str,
) {
    row(ui, p, title, hint, 380.0, |ui| {
        ui.add(Slider::new(value, range).suffix(" мс").step_by(5.0));
    });
}

fn combo(ui: &mut Ui, id: &str, value: &mut String, options: &[(&str, &str)]) -> bool {
    let current = options
        .iter()
        .find(|(v, _)| v == value)
        .map_or(value.as_str(), |(_, label)| *label)
        .to_owned();
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .selected_text(current)
        .width(260.0)
        .show_ui(ui, |ui| {
            for (v, label) in options {
                if ui.selectable_label(value == v, *label).clicked() {
                    *value = (*v).to_owned();
                    changed = true;
                }
            }
        });
    changed
}

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

/// «Оформление»: всё меняется сразу и само пишется в `[theme]` — без
/// «Сохранить» и «Отменить».
fn appearance(app: &mut App, ui: &mut Ui, p: &Palette) {
    card(ui, p, |ui| {
        card_title(ui, p, icons::PALETTE, "Оформление");
        let mode = app.theme_mode();
        row(
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
        if app.theme_forced() {
            note_line(
                ui,
                p,
                icons::INFO,
                p.weak,
                "Сейчас тема задана параметром --theme — только до закрытия окна.",
            );
        } else if mode == ThemeMode::System {
            let note = SystemNote::of(&app.portal);
            let (glyph, color) = match note {
                SystemNote::Waiting => (icons::HOURGLASS_MEDIUM, p.weak),
                note if note.fallback() => (icons::WARNING, p.warning),
                _ => (icons::INFO, p.weak),
            };
            let line = note_line(ui, p, glyph, color, note.text());
            if let PortalState::Unavailable(why) = &app.portal {
                line.on_hover_text(why);
            }
        }
        ui.add_space(10.0);

        let variant = app.theme_tab.unwrap_or(Variant::of(p.dark));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (v, title) in VARIANTS {
                if widgets::tab(ui, p, title, v == variant).clicked() {
                    app.theme_tab = Some(v);
                }
            }
        });
        ui.add_space(10.0);

        // Плитки — по списку палитр: новая палитра появится здесь сама.
        let t = app.settings.theme.clone();
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

        let system_accent = theme::system_accent_active(&t, &app.system, mode);
        if mode == ThemeMode::System {
            let blocked = theme::system_accent_blocked(&app.portal);
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

        let shown = theme::variant_palette(&t, &app.system, mode, variant);
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

pub fn show(app: &mut App, ui: &mut Ui, p: &Palette) {
    let shown = shown_path(&app.config_path());
    page_header(
        ui,
        p,
        icons::SLIDERS_HORIZONTAL,
        "Настройки",
        &format!("Сохраняются в {shown}"),
    );
    if let Some(err) = &app.settings_error {
        ui.horizontal_wrapped(|ui| {
            ui.label(widgets::icon(icons::WARNING, 16.0).color(p.negative));
            ui.label(RichText::new(err).color(p.negative));
        });
        ui.add_space(8.0);
    }
    let s = &mut app.settings;

    card(ui, p, |ui| {
        card_title(ui, p, icons::ARROW_FAT_LINES_UP, "Двойной Shift");
        slider_row(
            ui,
            p,
            &mut s.double_tap_ms,
            150..=900,
            "Пауза между нажатиями",
            "Сколько можно ждать перед вторым нажатием Shift.",
        );
        slider_row(
            ui,
            p,
            &mut s.tap_max_hold_ms,
            100..=700,
            "Длительность нажатия",
            "Shift, зажатый дольше, считается модификатором, а не нажатием.",
        );
    });
    ui.add_space(12.0);

    card(ui, p, |ui| {
        card_title(ui, p, icons::KEYBOARD, "Набор");
        slider_row(
            ui,
            p,
            &mut s.key_delay_ms,
            0..=40,
            "Задержка между нажатиями",
            "Увеличьте, если при исправлении теряются буквы.",
        );
        slider_row(
            ui,
            p,
            &mut s.switch_delay_ms,
            0..=300,
            "Пауза после смены раскладки",
            "Время, за которое система успевает переключить раскладку.",
        );
        row(
            ui,
            p,
            "Самое длинное слово",
            "Длиннее не исправляется — защита в играх, где «слово» — это WASD.",
            380.0,
            |ui| {
                ui.add(Slider::new(&mut s.max_word_len, 10..=200).suffix(" симв."));
            },
        );
    });
    ui.add_space(12.0);

    card(ui, p, |ui| {
        card_title(ui, p, icons::SELECTION, "Выделенный текст");
        toggle_row(
            ui,
            p,
            &mut s.selection,
            "Исправлять выделение",
            "Если перед двойным Shift выделить текст, исправится он.",
        );
        ui.add_enabled_ui(s.selection, |ui| {
            ui.add_space(4.0);
            row(
                ui,
                p,
                "Как заменять",
                "Вставка надёжнее. Набор обходится без буфера обмена, но переводы строк и символы, которых нет на клавиатуре, всё равно вставляются через него.",
                220.0,
                |ui| {
                    ui.radio_value(&mut s.selection_method, "type".to_owned(), "Набором");
                    ui.radio_value(&mut s.selection_method, "paste".to_owned(), "Вставкой");
                },
            );
            // Нужны и при наборе: текст, который клавишами не набрать, вставляется.
            row(ui, p, "Сочетание вставки", "", 280.0, |ui| {
                combo(ui, "paste_keys", &mut s.paste_keys, &PASTE_KEYS);
            });
            toggle_row(
                ui,
                p,
                &mut s.restore_clipboard,
                "Возвращать буфер обмена",
                "После вставки прежнее содержимое буфера обмена возвращается.",
            );
        });
    });
    ui.add_space(12.0);

    card(ui, p, |ui| {
        card_title(ui, p, icons::TRANSLATE, "Переключение раскладки");
        row(
            ui,
            p,
            "Чем переключать",
            "«Автоматически» сам найдёт KDE, GNOME, Hyprland, Sway или X11.",
            280.0,
            |ui| {
                combo(ui, "backend", &mut s.backend, &BACKENDS);
            },
        );
        if s.backend == "hotkey" {
            row(
                ui,
                p,
                "Ваше сочетание переключения",
                "Должно совпадать с настройкой системы.",
                280.0,
                |ui| {
                    combo(ui, "hotkey", &mut s.switch_hotkey, &HOTKEYS);
                },
            );
            row(
                ui,
                p,
                "Раскладки по порядку",
                "Через запятую, например: us, ru",
                220.0,
                |ui| {
                    let mut text = s.hotkey_layouts.join(", ");
                    if ui
                        .add(egui::TextEdit::singleline(&mut text).desired_width(200.0))
                        .changed()
                    {
                        s.hotkey_layouts = text
                            .split(',')
                            .map(|l| l.trim().to_owned())
                            .filter(|l| !l.is_empty())
                            .collect();
                    }
                },
            );
        }
    });
    ui.add_space(12.0);

    appearance(app, ui, p);
    ui.add_space(12.0);

    let s = &mut app.settings;
    card(ui, p, |ui| {
        card_title(ui, p, icons::BELL, "Интерфейс");
        toggle_row(
            ui,
            p,
            &mut s.tray,
            "Значок в трее",
            "Пауза, настройки и выход — из системного трея.",
        );
        toggle_row(
            ui,
            p,
            &mut s.notify,
            "Уведомления",
            "Сообщать о паузе и возобновлении.",
        );
    });
    ui.add_space(12.0);

    let service = app.snap.service.clone();
    card(ui, p, |ui| {
        card_title(ui, p, icons::POWER, "Автозапуск");
        if service.installed || !service.systemd {
            let mut autostart = service.autostart;
            if toggle_row(
                ui,
                p,
                &mut autostart,
                "Запускать вместе с сеансом",
                "Служба стартует сама после входа в систему.",
            )
            .changed()
            {
                app.service(ServiceAction::Autostart(autostart));
            }
        } else {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Служба systemd ещё не установлена.").color(p.weak));
                if widgets::button(ui, p, icons::DOWNLOAD_SIMPLE, "Установить", false).clicked()
                {
                    app.service(ServiceAction::Install);
                }
            });
        }
    });
    ui.add_space(18.0);

    let dirty = app.settings != app.saved;
    ui.horizontal(|ui| {
        let valid = Config::from_settings(app.settings.clone());
        if let Err(err) = &valid {
            ui.label(widgets::icon(icons::WARNING, 16.0).color(p.negative));
            ui.label(RichText::new(format!("{err:#}")).color(p.negative));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let save = ui
                .add_enabled_ui(dirty && valid.is_ok(), |ui| {
                    widgets::button(ui, p, icons::FLOPPY_DISK, "Сохранить", true)
                })
                .inner;
            if save.clicked() {
                app.save_settings("Настройки сохранены");
            }
            if ui
                .add_enabled_ui(dirty, |ui| {
                    widgets::button(ui, p, icons::ARROW_COUNTER_CLOCKWISE, "Отменить", false)
                })
                .inner
                .clicked()
            {
                app.settings = app.saved.clone();
            }
            if widgets::button(ui, p, icons::BROOM, "По умолчанию", false).clicked() {
                // Роли устройств и оформление сбрасываются отдельно.
                let devices = std::mem::take(&mut app.settings.devices);
                let theme = app.settings.theme.clone();
                app.settings = Settings {
                    devices,
                    theme,
                    ..Settings::default()
                };
                app.toast(
                    ToastKind::Info,
                    "Значения по умолчанию — нажмите «Сохранить»",
                );
            }
            if dirty {
                ui.label(RichText::new("есть несохранённые изменения").color(p.warning));
            }
        });
    });
}

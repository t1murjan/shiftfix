//! Настройки: правка config.toml с сохранением комментариев.

use eframe::egui::{self, Align, Layout, RichText, Slider, Ui};
use egui_phosphor::regular as icons;
use shiftfix::config::{Config, Settings};

use crate::app::{App, ToastKind};
use crate::link::ServiceAction;
use crate::theme::Palette;
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

pub fn show(app: &mut App, ui: &mut Ui, p: &Palette) {
    let path = app.config_path().display().to_string();
    let home = std::env::var("HOME").unwrap_or_default();
    let shown = match path.strip_prefix(&home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => path,
    };
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
                let devices = std::mem::take(&mut app.settings.devices);
                app.settings = Settings {
                    devices,
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

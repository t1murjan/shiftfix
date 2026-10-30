//! Устройства: что слушает shiftfix, активность и ручная поправка роли.

use eframe::egui::{self, Color32, CornerRadius, Frame, Margin, RichText, Stroke, Ui};
use egui_phosphor::regular as icons;
use shiftfix::devices::{DeviceInfo, Role};

use crate::app::App;
use crate::theme::Palette;
use crate::widgets::{self, card, card_title, page_intro};

pub fn role_icon(role: Role) -> &'static str {
    match role {
        Role::Keyboard => icons::KEYBOARD,
        Role::Mouse => icons::MOUSE,
        Role::Touchpad => icons::HAND_POINTING,
        Role::Touchscreen => icons::HAND_TAP,
        Role::Tablet => icons::PEN_NIB,
        Role::Ignore => icons::EYE_SLASH,
        Role::Other => icons::PLUGS,
    }
}

pub fn show(app: &mut App, ui: &mut Ui, p: &Palette) {
    page_intro(
        ui,
        p,
        "Клавиатуры shiftfix слушает, а клики мыши, тачпада и касания экрана сбрасывают запомненное слово.",
    );
    tip(ui, p);
    ui.add_space(14.0);

    let locked = app.settings.error().is_some();
    if locked {
        widgets::notice(
            ui,
            icons::WARNING,
            p.negative,
            "В config.toml ошибка: пока её не исправят, роли не меняются. Подробности — на странице «Настройки».",
        );
        ui.add_space(8.0);
    } else if app.status().is_none() {
        widgets::notice(
            ui,
            icons::INFO,
            p.warning,
            "Служба не запущена: активность не видна, но роли можно поменять — они применятся при запуске.",
        );
        ui.add_space(8.0);
    }

    // Пока служба не отвечает, а окно ещё не нашло устройства, — пусто, без
    // предупреждения: поиск займёт мгновение.
    let Some(devices) = devices_now(app) else {
        return;
    };
    if devices.is_empty() {
        card(ui, p, |ui| {
            widgets::empty_state(
                ui,
                p,
                icons::PLUGS,
                "Устройства не видны. Нужна группа input: sudo usermod -aG input $USER, затем перезагрузка.",
            );
        });
        return;
    }

    if let Some((key, role)) = groups(ui, p, &devices, !locked) {
        app.set_device_role(&key, role);
    }
}

/// Подсказка в рамке акцентного цвета: как узнать устройство и поправить роль.
fn tip(ui: &mut Ui, p: &Palette) {
    Frame::NONE
        .fill(p.accent.gamma_multiply(if p.dark { 0.16 } else { 0.09 }))
        .stroke(Stroke::new(1.0, p.accent.gamma_multiply(0.35)))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(widgets::icon(icons::LIGHTBULB, 18.0).color(p.accent));
                ui.label(
                    RichText::new(
                        "Нажмите клавишу, щёлкните или коснитесь экрана — точка у этого устройства вспыхнет. \
                         Если устройство определилось неверно (например, сенсорный экран принят за тачпад), \
                         выберите роль вручную.",
                    )
                    .color(p.text),
                );
            });
        });
}

/// Устройства с простоем на этот кадр; `None` — их ещё не нашли.
fn devices_now(app: &App) -> Option<Vec<DeviceInfo>> {
    let mut devices = app.devices()?.to_vec();
    // Простой устройства — на момент опроса; досчитываем, чтобы точка гасла
    // плавно, а не ступеньками раз в 400 мс.
    let since_poll = app
        .snap
        .updated
        .map_or(0, |t| t.elapsed().as_millis() as u64);
    for dev in &mut devices {
        dev.idle_ms = dev.idle_ms.map(|ms| ms + since_poll);
    }
    Some(devices)
}

/// Карточки групп устройств; возвращает устройство, которому выбрали роль,
/// и роль (`None` — «авто»).
fn groups(
    ui: &mut Ui,
    p: &Palette,
    devices: &[DeviceInfo],
    editable: bool,
) -> Option<(String, Option<Role>)> {
    let groups: [(&str, &str, &[Role]); 4] = [
        (icons::KEYBOARD, "Клавиатуры", &[Role::Keyboard]),
        (
            icons::CURSOR_CLICK,
            "Мыши и тачпады",
            &[Role::Mouse, Role::Touchpad],
        ),
        (
            icons::HAND_TAP,
            "Сенсорные экраны и перья",
            &[Role::Touchscreen, Role::Tablet],
        ),
        (
            icons::EYE_SLASH,
            "Не слушаются",
            &[Role::Ignore, Role::Other],
        ),
    ];
    let mut change: Option<(String, Option<Role>)> = None;
    for (icon, title, roles) in groups {
        let in_group: Vec<&DeviceInfo> =
            devices.iter().filter(|d| roles.contains(&d.role)).collect();
        if in_group.is_empty() {
            continue;
        }
        card(ui, p, |ui| {
            card_title(ui, p, icon, &format!("{title} · {}", in_group.len()));
            for (i, dev) in in_group.iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                if let Some(choice) = device_row(ui, p, dev, editable) {
                    change = Some((dev.key.clone(), choice));
                }
            }
        });
        ui.add_space(12.0);
    }
    change
}

/// Строка устройства; возвращает выбранную роль (`Some(None)` — «авто»).
/// `editable == false` — выбор роли неактивен.
fn device_row(ui: &mut Ui, p: &Palette, dev: &DeviceInfo, editable: bool) -> Option<Option<Role>> {
    let mut picked = None;
    ui.horizontal(|ui| {
        widgets::activity_dot(ui, p, dev.idle_ms);
        let icon_color = if dev.role.listens() { p.accent } else { p.weak };
        ui.label(widgets::icon(role_icon(dev.role), 20.0).color(icon_color));
        ui.vertical(|ui| {
            let name = if dev.name.is_empty() { "(без имени)" } else { &dev.name };
            ui.add(egui::Label::new(RichText::new(name).color(p.text).strong()).wrap());
            let mut details = vec![dev.key.split(' ').next().unwrap_or_default().to_owned(), dev.path.clone()];
            if dev.virtual_device {
                details.push("виртуальное".into());
            }
            if dev.overridden {
                details.push("роль задана вручную".into());
            }
            ui.label(RichText::new(details.join(" · ")).size(12.0).color(p.weak));
            ui.add_space(2.0);
            // Роль — под именем: рядом с ним в узком окне не помещается.
            ui.horizontal(|ui| {
                let auto_label = format!("Авто: {}", dev.detected.title().to_lowercase());
                let selected = if dev.overridden { dev.role.title().to_owned() } else { auto_label.clone() };
                ui.add_enabled_ui(editable, |ui| {
                    egui::ComboBox::from_id_salt(("role", &dev.key, &dev.path))
                        .selected_text(selected)
                        .width(200.0)
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(!dev.overridden, auto_label).clicked() && dev.overridden {
                                picked = Some(None);
                            }
                            ui.separator();
                            for role in Role::CHOICES {
                                let label = widgets::icon_job(role_icon(role), role.title(), 14.5, p.text);
                                let current = dev.overridden && dev.role == role;
                                if ui.selectable_label(current, label).clicked() && !current {
                                    picked = Some(Some(role));
                                }
                            }
                        });
                });
                if dev.role == Role::Keyboard && !dev.overridden && dev.virtual_device {
                    ui.label(widgets::icon(icons::ROBOT, 17.0).color(Color32::GRAY)).on_hover_text(
                        "Виртуальная клавиатура (keyd, kanata, remapper…). Если она дублирует настоящую — выберите «Не слушать».",
                    );
                }
            });
        });
    });
    picked
}

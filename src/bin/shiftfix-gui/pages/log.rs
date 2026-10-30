//! Журнал службы и проверка системы.

use eframe::egui::{RichText, ScrollArea, Ui};
use egui_phosphor::regular as icons;

use crate::app::App;
use crate::format::clock;
use crate::link::ServiceAction;
use crate::theme::Palette;
use crate::toast::ToastKind;
use crate::widgets::{self, card, card_title, page_intro};

pub fn show(app: &mut App, ui: &mut Ui, p: &Palette) {
    page_intro(
        ui,
        p,
        "Сообщения службы и проверка того, всё ли готово к работе",
    );
    actions(app, ui, p);
    ui.add_space(12.0);
    if let Some(text) = app.check.output() {
        check_result(ui, p, &text);
        ui.add_space(12.0);
    }
    messages(app, ui, p);
}

/// Кнопки: проверка системы, служба, конфиг.
fn actions(app: &mut App, ui: &mut Ui, p: &Palette) {
    card(ui, p, |ui| {
        ui.horizontal_wrapped(|ui| {
            let running = app.check.running();
            let label = if running {
                "Проверяю…"
            } else {
                "Проверить систему"
            };
            if ui
                .add_enabled_ui(!running, |ui| {
                    widgets::button(ui, p, icons::STETHOSCOPE, label, true)
                })
                .inner
                .clicked()
            {
                app.check.start(ui.ctx());
            }
            if app.status().is_some() {
                if widgets::button(
                    ui,
                    p,
                    icons::ARROWS_CLOCKWISE,
                    "Перезапустить службу",
                    false,
                )
                .clicked()
                {
                    app.service(ServiceAction::Restart);
                }
                if widgets::button(ui, p, icons::STOP, "Остановить службу", false).clicked()
                {
                    app.service(ServiceAction::Stop);
                }
            } else if widgets::button(ui, p, icons::PLAY, "Запустить службу", false).clicked()
            {
                app.service(ServiceAction::Start);
            }
            if widgets::button(ui, p, icons::FOLDER_OPEN, "Открыть конфиг", false).clicked()
            {
                open_config(app);
            }
        });
    });
}

/// Открыть config.toml в редакторе по умолчанию; файла ещё нет — создать,
/// иначе открывать нечего.
fn open_config(app: &mut App) {
    let path = app.settings.path().to_owned();
    let created = if path.exists() {
        Ok(())
    } else {
        app.settings.saved().save(&path)
    };
    let opened = created
        .and_then(|()| shiftfix::sys::proc::spawn_detached("xdg-open", &[&path.to_string_lossy()]));
    if let Err(err) = opened {
        app.toast(ToastKind::Error, format!("{err:#}"));
    }
}

/// Вывод `shiftfix --check`: строки с ✗ — красным, с ✓ — зелёным.
fn check_result(ui: &mut Ui, p: &Palette, text: &str) {
    card(ui, p, |ui| {
        card_title(ui, p, icons::LIST_CHECKS, "Проверка системы");
        for line in text.lines() {
            let color = if line.contains('✗') {
                p.negative
            } else if line.contains('✓') {
                p.positive
            } else {
                p.text
            };
            ui.label(RichText::new(line).monospace().color(color));
        }
    });
}

/// Журнал службы, а пока она не отвечает — journalctl.
fn messages(app: &mut App, ui: &mut Ui, p: &Palette) {
    let (lines, source) = match app.status() {
        Some(s) => (
            s.log
                .iter()
                .map(|l| (clock(l.time), l.level.clone(), l.text.clone()))
                .collect::<Vec<_>>(),
            "служба",
        ),
        None => (
            app.snap
                .journal
                .iter()
                .map(|l| (String::new(), String::new(), l.clone()))
                .collect(),
            "journalctl",
        ),
    };
    card(ui, p, |ui| {
        ui.horizontal(|ui| {
            card_title(ui, p, icons::SCROLL, "Сообщения");
            ui.with_layout(
                eframe::egui::Layout::right_to_left(eframe::egui::Align::Center),
                |ui| {
                    let copy = widgets::button(ui, p, icons::COPY, "", false)
                        .on_hover_text(format!("Копировать (источник — {source})"));
                    if copy.clicked() {
                        let text: Vec<String> = lines
                            .iter()
                            .map(|(t, l, m)| format!("{t} {l} {m}").trim().to_owned())
                            .collect();
                        ui.ctx().copy_text(text.join("\n"));
                        app.toast(ToastKind::Success, "Журнал скопирован");
                    }
                },
            );
        });
        if lines.is_empty() {
            widgets::empty_state(ui, p, icons::SCROLL, "Сообщений пока нет");
            return;
        }
        ScrollArea::vertical()
            .max_height(420.0)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.spacing_mut().interact_size.y = 18.0;
                for (time, level, text) in &lines {
                    let color = match level.as_str() {
                        "ERROR" => p.negative,
                        "WARN" => p.warning,
                        "DEBUG" => p.weak,
                        _ => p.text,
                    };
                    ui.horizontal_wrapped(|ui| {
                        if !time.is_empty() {
                            ui.label(RichText::new(time).monospace().color(p.weak));
                        }
                        ui.label(RichText::new(text).monospace().color(color));
                    });
                }
            });
    });
}

//! Журнал службы и проверка системы.

use eframe::egui::{RichText, ScrollArea, Ui};
use egui_phosphor::regular as icons;

use crate::app::App;
use crate::format::clock;
use crate::link::{self, ServiceAction};
use crate::theme::Palette;
use crate::toast::ToastKind;
use crate::widgets::{self, card, card_title, page_header};

pub fn show(app: &mut App, ui: &mut Ui, p: &Palette) {
    page_header(
        ui,
        p,
        icons::SCROLL,
        "Журнал",
        "Сообщения службы и проверка того, всё ли готово к работе",
    );

    card(ui, p, |ui| {
        ui.horizontal_wrapped(|ui| {
            let label = if app.checking {
                "Проверяю…"
            } else {
                "Проверить систему"
            };
            if ui
                .add_enabled_ui(!app.checking, |ui| {
                    widgets::button(ui, p, icons::STETHOSCOPE, label, true)
                })
                .inner
                .clicked()
            {
                app.checking = true;
                if let Ok(mut slot) = app.check_output.lock() {
                    *slot = None;
                }
                link::run_check(app.check_output.clone(), ui.ctx().clone());
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
                let path = app.settings.path().to_owned();
                // Файла ещё нет — создать, иначе открывать нечего.
                let created = if path.exists() {
                    Ok(())
                } else {
                    app.settings.saved().save(&path)
                };
                let opened = created.and_then(|()| {
                    shiftfix::sys::proc::spawn_detached("xdg-open", &[&path.to_string_lossy()])
                });
                if let Err(err) = opened {
                    app.toast(ToastKind::Error, format!("{err:#}"));
                }
            }
        });
    });
    ui.add_space(12.0);

    let output = app.check_output.lock().ok().and_then(|o| o.clone());
    if let Some(text) = output {
        app.checking = false;
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
        ui.add_space(12.0);
    }

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
            card_title(ui, p, icons::SCROLL, &format!("Сообщения ({source})"));
            ui.with_layout(
                eframe::egui::Layout::right_to_left(eframe::egui::Align::Center),
                |ui| {
                    if widgets::button(ui, p, icons::COPY, "Копировать", false).clicked()
                    {
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

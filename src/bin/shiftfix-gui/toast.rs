//! Всплывающие сообщения в углу окна: «Выполняю…», «Сохранено», ошибки.

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Frame, Margin, Order, RichText, Stroke, vec2,
};
use egui_phosphor::regular as icons;

use crate::theme::Palette;
use crate::widgets;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

pub struct Toast {
    pub kind: ToastKind,
    pub text: String,
    born: Instant,
}

impl Toast {
    pub fn new(kind: ToastKind, text: impl Into<String>) -> Toast {
        Toast {
            kind,
            text: text.into(),
            born: Instant::now(),
        }
    }
}

/// Сообщения на экране и канал, по которому их присылают фоновые потоки.
pub struct Toasts {
    list: Vec<Toast>,
    rx: Receiver<Toast>,
}

impl Toasts {
    /// Сообщения и отправитель для фоновых потоков (связь со службой).
    pub fn new() -> (Toasts, Sender<Toast>) {
        let (tx, rx) = mpsc::channel();
        (
            Toasts {
                list: Vec::new(),
                rx,
            },
            tx,
        )
    }

    pub fn push(&mut self, toast: Toast) {
        self.list.push(toast);
    }

    /// Забрать то, что прислали фоновые потоки.
    pub fn receive(&mut self) {
        while let Ok(toast) = self.rx.try_recv() {
            self.list.push(toast);
        }
    }

    /// Нарисовать в правом нижнем углу; старые сообщения гаснут сами.
    pub fn show(&mut self, ctx: &egui::Context, p: &Palette) {
        const LIFE: f32 = 3.5;
        self.list.retain(|t| t.born.elapsed().as_secs_f32() < LIFE);
        if self.list.is_empty() {
            return;
        }
        // Пока висит «Выполняю…», а уже пришёл результат — оставляем только результат.
        if self.list.iter().any(|t| t.kind != ToastKind::Info) {
            self.list
                .retain(|t| t.kind != ToastKind::Info || t.born.elapsed().as_secs_f32() < 0.3);
        }
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(Align2::RIGHT_BOTTOM, vec2(-20.0, -20.0))
            .order(Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                for toast in self.list.iter().rev().take(4) {
                    let age = toast.born.elapsed().as_secs_f32();
                    let alpha = ((LIFE - age) / 0.4).clamp(0.0, 1.0) * (age / 0.15).clamp(0.0, 1.0);
                    let (icon, color) = match toast.kind {
                        ToastKind::Info => (icons::HOURGLASS_MEDIUM, p.accent),
                        ToastKind::Success => (icons::CHECK_CIRCLE, p.positive),
                        ToastKind::Error => (icons::WARNING_CIRCLE, p.negative),
                    };
                    Frame::NONE
                        .fill(p.card.gamma_multiply(alpha))
                        .stroke(Stroke::new(1.0, color.gamma_multiply(0.6 * alpha)))
                        .corner_radius(CornerRadius::same(12))
                        .inner_margin(Margin::symmetric(16, 12))
                        .shadow(egui::Shadow {
                            offset: [0, 6],
                            blur: 20,
                            spread: 0,
                            color: Color32::from_black_alpha((60.0 * alpha) as u8),
                        })
                        .show(ui, |ui| {
                            ui.set_max_width(380.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    widgets::icon(icon, 18.0).color(color.gamma_multiply(alpha)),
                                );
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&toast.text)
                                            .color(p.text.gamma_multiply(alpha)),
                                    )
                                    .wrap(),
                                );
                            });
                        });
                    ui.add_space(8.0);
                }
            });
        ctx.request_repaint_after(Duration::from_millis(50));
    }
}

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

/// Сколько живёт обычное сообщение, секунд.
const LIFE: f32 = 3.5;
/// «Выполняю…» висит до итога, но не дольше этого: вдруг итог не придёт.
const WAIT_MAX: f32 = 60.0;

pub struct Toast {
    pub kind: ToastKind,
    pub text: String,
    born: Instant,
    /// Номер действия в фоне: у «Выполняю…» — чей итог ждём, у итога —
    /// чьё «Выполняю…» он заменяет.
    action: Option<u64>,
}

impl Toast {
    pub fn new(kind: ToastKind, text: impl Into<String>) -> Toast {
        Toast {
            kind,
            text: text.into(),
            born: Instant::now(),
            action: None,
        }
    }

    /// Итог действия `action` (номер от `Toasts::start_action`).
    pub fn result_of(mut self, action: u64) -> Toast {
        self.action = Some(action);
        self
    }

    /// «Выполняю…», которое ждёт итога.
    fn waiting(&self) -> bool {
        self.kind == ToastKind::Info && self.action.is_some()
    }

    fn age(&self) -> f32 {
        self.born.elapsed().as_secs_f32()
    }
}

/// Сообщения на экране и канал, по которому их присылают фоновые потоки.
pub struct Toasts {
    list: Vec<Toast>,
    rx: Receiver<Toast>,
    last_action: u64,
}

impl Toasts {
    /// Сообщения и отправитель для фоновых потоков (связь со службой).
    pub fn new() -> (Toasts, Sender<Toast>) {
        let (tx, rx) = mpsc::channel();
        (
            Toasts {
                list: Vec::new(),
                rx,
                last_action: 0,
            },
            tx,
        )
    }

    pub fn push(&mut self, toast: Toast) {
        self.list.push(toast);
    }

    /// Показать «Выполняю…» для действия в фоне. Итог надо прислать с
    /// возвращённым номером (`Toast::result_of`): он и уберёт «Выполняю…».
    pub fn start_action(&mut self) -> u64 {
        self.last_action += 1;
        self.push(Toast::new(ToastKind::Info, "Выполняю…").result_of(self.last_action));
        self.last_action
    }

    /// Забрать то, что прислали фоновые потоки.
    pub fn receive(&mut self) {
        while let Ok(toast) = self.rx.try_recv() {
            if let Some(action) = toast.action {
                self.list
                    .retain(|t| !(t.waiting() && t.action == Some(action)));
            }
            self.list.push(toast);
        }
    }

    /// Убрать отжившие сообщения.
    fn expire(&mut self) {
        self.list
            .retain(|t| t.age() < if t.waiting() { WAIT_MAX } else { LIFE });
    }

    /// Нарисовать в правом нижнем углу; старые сообщения гаснут сами.
    pub fn show(&mut self, ctx: &egui::Context, p: &Palette) {
        self.expire();
        if self.list.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(Align2::RIGHT_BOTTOM, vec2(-20.0, -20.0))
            .order(Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                for toast in self.list.iter().rev().take(4) {
                    let age = toast.age();
                    let fade_out = if toast.waiting() {
                        1.0
                    } else {
                        ((LIFE - age) / 0.4).clamp(0.0, 1.0)
                    };
                    let alpha = fade_out * (age / 0.15).clamp(0.0, 1.0);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn aged(mut toast: Toast, secs: u64) -> Toast {
        toast.born = Instant::now() - Duration::from_secs(secs);
        toast
    }

    #[test]
    fn waiting_toast_lives_until_its_result() {
        let (mut toasts, tx) = Toasts::new();
        let first = toasts.start_action();
        let second = toasts.start_action();
        // «Выполняю…» второго действия висит дольше обычного сообщения…
        toasts.list[1] = aged(
            Toast::new(ToastKind::Info, "Выполняю…").result_of(second),
            10,
        );
        // …и его не гасит чужое сообщение или итог другого действия.
        tx.send(Toast::new(ToastKind::Success, "Журнал скопирован"))
            .unwrap();
        tx.send(Toast::new(ToastKind::Success, "Служба запущена").result_of(first))
            .unwrap();
        toasts.receive();
        toasts.expire();
        let texts: Vec<&str> = toasts.list.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, ["Выполняю…", "Журнал скопирован", "Служба запущена"]);

        tx.send(Toast::new(ToastKind::Error, "не запустилась").result_of(second))
            .unwrap();
        toasts.receive();
        assert!(!toasts.list.iter().any(Toast::waiting));
    }

    #[test]
    fn plain_info_toast_expires() {
        let (mut toasts, _tx) = Toasts::new();
        toasts.push(aged(Toast::new(ToastKind::Info, "По умолчанию"), 4));
        toasts.expire();
        assert!(toasts.list.is_empty());
    }
}

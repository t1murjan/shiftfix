//! Окно: боковая панель, страницы, всплывающие сообщения.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Frame, Image, Layout, Margin, Order, RichText,
    ScrollArea, Sense, Stroke, TextureHandle, TextureOptions, Ui, load::SizedTexture, vec2,
};
use egui_phosphor::regular as icons;
use shiftfix::config::{Config, Settings, ThemeMode, ThemeSettings};
use shiftfix::devices::{self, DeviceInfo, Role};
use shiftfix::ipc::{Request, Status};

use crate::link::{Link, ServiceAction, Snapshot};
use crate::pages;
use crate::theme::{self, Palette};
use crate::widgets::{self, bold};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Home,
    Devices,
    Settings,
    Help,
    Log,
}

impl Page {
    pub const ALL: [Page; 5] = [
        Page::Home,
        Page::Devices,
        Page::Settings,
        Page::Help,
        Page::Log,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Page::Home => "Главная",
            Page::Devices => "Устройства",
            Page::Settings => "Настройки",
            Page::Help => "Инструкция",
            Page::Log => "Журнал",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Page::Home => icons::HOUSE,
            Page::Devices => icons::KEYBOARD,
            Page::Settings => icons::SLIDERS_HORIZONTAL,
            Page::Help => icons::BOOK_OPEN_TEXT,
            Page::Log => icons::SCROLL,
        }
    }

    pub fn parse(name: &str) -> Option<Page> {
        match name {
            "home" => Some(Page::Home),
            "devices" => Some(Page::Devices),
            "settings" => Some(Page::Settings),
            "help" => Some(Page::Help),
            "log" => Some(Page::Log),
            _ => None,
        }
    }
}

/// Версия окна; у службы своя — в `Status.version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Служба другой версии, чем окно: после обновления её не перезапустили.
/// Пустая версия службы (не сообщила) — не расхождение.
pub fn versions_differ(window: &str, service: &str) -> bool {
    let norm = |v: &str| v.trim().trim_start_matches('v').to_owned();
    let service = norm(service);
    !service.is_empty() && service != norm(window)
}

/// Путь для показа: домашний каталог — «~».
pub fn short_path(path: &Path, home: Option<&Path>) -> String {
    match home.filter(|h| !h.as_os_str().is_empty()) {
        Some(home) => match path.strip_prefix(home) {
            Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        },
        None => path.display().to_string(),
    }
}

/// Путь для показа относительно `$HOME`.
pub fn shown_path(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    short_path(path, home.as_deref())
}

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

/// Что сейчас со службой — для заголовка и боковой панели.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum State {
    Working,
    Paused,
    Stopped,
    NotInstalled,
}

pub struct App {
    pub page: Page,
    pub link: Link,
    pub snap: Snapshot,
    toasts: Vec<Toast>,
    toast_rx: Receiver<Toast>,
    pub settings: Settings,
    pub saved: Settings,
    pub settings_error: Option<String>,
    pub try_text: String,
    pub check_output: Arc<Mutex<Option<String>>>,
    pub checking: bool,
    local_devices: Option<(Instant, Vec<DeviceInfo>)>,
    logo: TextureHandle,
    /// Тема окна: портал, применённая палитра, отложенная запись.
    pub theme: theme::ThemeState,
    /// Прокрутить «Инструкцию» к блоку «О программе» (клик по версии).
    pub about_focus: bool,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        page: Page,
        forced_mode: Option<ThemeMode>,
        watch: theme::SystemWatch,
    ) -> App {
        let ctx = &cc.egui_ctx;
        let (toast_tx, toast_rx) = mpsc::channel();
        let link = Link::start(ctx, toast_tx);
        let logo_image =
            egui::ColorImage::from_rgba_unmultiplied([96, 96], &shiftfix::icon::rgba(96, true));
        let logo = ctx.load_texture("logo", logo_image, TextureOptions::LINEAR);
        let theme_path = Config::default_path();
        let (mut settings, settings_error) = match Settings::load(&theme_path) {
            Ok(s) => (s, None),
            Err(err) => {
                // Ошибка в другом параметре — тему всё равно берём из файла.
                let theme = ThemeSettings::load(&theme_path).unwrap_or_default();
                let settings = Settings {
                    theme,
                    ..Settings::default()
                };
                (settings, Some(format!("{err:#}")))
            }
        };
        theme::migrate_legacy(&theme_path, &mut settings.theme);
        App {
            page,
            link,
            snap: Snapshot::default(),
            toasts: Vec::new(),
            toast_rx,
            saved: settings.clone(),
            settings,
            settings_error,
            try_text: String::new(),
            check_output: Arc::default(),
            checking: false,
            local_devices: None,
            logo,
            theme: theme::ThemeState::new(theme_path, forced_mode, watch),
            about_focus: false,
        }
    }

    /// Режим, в котором окно сейчас: `--theme` или из настроек.
    pub fn theme_mode(&self) -> ThemeMode {
        self.theme.mode(&self.settings.theme)
    }

    /// Выбор режима в окне (боковая панель и «Оформление»): отменяет `--theme`,
    /// вкладка «Оформления» снова показывает вариант на экране.
    pub fn set_mode(&mut self, mode: ThemeMode) {
        self.theme.mode_chosen();
        self.set_theme(|t| t.mode = mode);
    }

    /// Изменить тему: видно сразу, в файл — чуть позже и только секция `[theme]`.
    /// «Сохранить» на странице настроек тема не включает.
    pub fn set_theme(&mut self, change: impl FnOnce(&mut ThemeSettings)) {
        let mut theme = self.settings.theme.clone();
        change(&mut theme);
        if theme == self.settings.theme {
            return;
        }
        self.saved.theme = theme.clone();
        self.settings.theme = theme;
        self.theme.changed();
    }

    fn save_theme(&mut self) {
        if let Some(err) = self.theme.save(&self.settings.theme) {
            self.toast(ToastKind::Error, format!("Тема не сохранилась: {err}"));
        }
    }

    /// Цвета кадра: применить к egui, если тема изменилась, и вовремя записать её.
    fn palette(&mut self, ctx: &egui::Context) -> Palette {
        let p = self.theme.apply(ctx, &self.settings.theme);
        if self.theme.time_to_save(ctx) {
            self.save_theme();
        }
        p
    }

    pub fn config_path(&self) -> PathBuf {
        self.snap
            .status
            .as_ref()
            .map(|s| PathBuf::from(&s.config_path))
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(Config::default_path)
    }

    pub fn status(&self) -> Option<&Status> {
        self.snap.status.as_deref()
    }

    pub fn state(&self) -> State {
        match self.status() {
            Some(s) if s.enabled => State::Working,
            Some(_) => State::Paused,
            None if self.snap.service.installed || !self.snap.service.systemd => State::Stopped,
            None => State::NotInstalled,
        }
    }

    pub fn toast(&mut self, kind: ToastKind, text: impl Into<String>) {
        self.toasts.push(Toast::new(kind, text));
    }

    /// Устройства: от службы (с активностью) или собственный опрос, если служба не запущена.
    pub fn devices(&mut self) -> Vec<DeviceInfo> {
        if let Some(status) = self.status() {
            return status.devices.clone();
        }
        let stale = self
            .local_devices
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(3));
        if stale {
            let cfg = Config::from_settings(self.saved.clone()).unwrap_or_default();
            let list = devices::event_nodes()
                .iter()
                .filter_map(|p| devices::describe(p, &cfg))
                .collect();
            self.local_devices = Some((Instant::now(), list));
        }
        self.local_devices
            .as_ref()
            .map(|(_, l)| l.clone())
            .unwrap_or_default()
    }

    /// Сохранить настройки в файл и попросить службу перечитать их.
    pub fn save_settings(&mut self, success: &'static str) {
        match self.settings.save(&self.config_path()) {
            Ok(()) => {
                self.saved = self.settings.clone();
                self.settings_error = None;
                self.local_devices = None;
                if self.status().is_some() {
                    self.link.send(Request::Reload, Some(success));
                } else {
                    self.toast(
                        ToastKind::Success,
                        format!("{success}. Применится при запуске службы"),
                    );
                }
            }
            Err(err) => {
                self.settings_error = Some(format!("{err:#}"));
                self.toast(ToastKind::Error, format!("Не сохранилось: {err:#}"));
            }
        }
    }

    /// Роль устройства вручную (`None` — снова автоматически).
    pub fn set_device_role(&mut self, key: &str, role: Option<Role>) {
        // Остальные несохранённые правки настроек не трогаем.
        let mut on_disk = self.saved.clone();
        match role {
            Some(role) => on_disk
                .devices
                .insert(key.to_owned(), role.as_str().to_owned()),
            None => on_disk.devices.remove(key),
        };
        match role {
            Some(role) => self
                .settings
                .devices
                .insert(key.to_owned(), role.as_str().to_owned()),
            None => self.settings.devices.remove(key),
        };
        let edited = std::mem::replace(&mut self.settings, on_disk);
        self.save_settings("Роль устройства сохранена");
        self.settings = edited;
    }

    pub fn service(&mut self, action: ServiceAction) {
        self.toast(ToastKind::Info, "Выполняю…");
        self.link.service(action);
    }

    // ─── отрисовка ───

    fn sidebar(&mut self, ui: &mut Ui, p: &Palette) {
        ui.horizontal(|ui| {
            ui.add(Image::from_texture(SizedTexture::new(
                self.logo.id(),
                vec2(42.0, 42.0),
            )));
            ui.vertical(|ui| {
                ui.add_space(2.0);
                ui.label(bold("shiftfix", 21.0).color(p.text));
                ui.label(
                    RichText::new("исправление раскладки")
                        .size(12.5)
                        .color(p.weak),
                );
            });
        });
        ui.add_space(22.0);
        for page in Page::ALL {
            if nav_item(ui, p, page.icon(), page.title(), self.page == page).clicked() {
                self.page = page;
            }
            ui.add_space(2.0);
        }
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.horizontal(|ui| {
                let current = self.theme_mode();
                for (mode, icon, hint) in [
                    (ThemeMode::System, icons::CIRCLE_HALF, "Тема как в системе"),
                    (ThemeMode::Light, icons::SUN, "Светлая тема"),
                    (ThemeMode::Dark, icons::MOON, "Тёмная тема"),
                ] {
                    let selected = current == mode;
                    let text =
                        widgets::icon(icon, 16.0).color(if selected { p.accent } else { p.weak });
                    let button = egui::Button::new(text)
                        .fill(if selected {
                            p.accent.gamma_multiply(0.16)
                        } else {
                            Color32::TRANSPARENT
                        })
                        .stroke(Stroke::NONE)
                        .corner_radius(CornerRadius::same(8))
                        .min_size(vec2(34.0, 30.0));
                    if ui.add(button).on_hover_text(hint).clicked() {
                        self.set_mode(mode);
                    }
                }
            });
            ui.add_space(6.0);
            let version = self.status().map_or_else(
                || format!("версия {VERSION}"),
                |s| format!("служба {} · pid {}", s.version, s.pid),
            );
            let differ = self
                .status()
                .is_some_and(|s| versions_differ(VERSION, &s.version));
            let (color, hint) = if differ {
                (
                    p.warning,
                    "Версии окна и службы различаются — подробнее в «О программе»",
                )
            } else {
                (p.weak, "О программе")
            };
            let label = egui::Label::new(RichText::new(version).size(12.0).color(color))
                .sense(Sense::click());
            let clicked = ui
                .add(label)
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(hint)
                .clicked();
            if clicked {
                self.page = Page::Help;
                self.about_focus = true;
            }
            ui.add_space(4.0);
            let (text, color) = state_label(self.state(), p);
            widgets::pill(ui, text, color, color.gamma_multiply(0.14));
        });
    }

    fn draw_toasts(&mut self, ctx: &egui::Context, p: &Palette) {
        const LIFE: f32 = 3.5;
        self.toasts
            .retain(|t| t.born.elapsed().as_secs_f32() < LIFE);
        if self.toasts.is_empty() {
            return;
        }
        // Пока висит «Выполняю…», а уже пришёл результат — оставляем только результат.
        if self.toasts.iter().any(|t| t.kind != ToastKind::Info) {
            self.toasts
                .retain(|t| t.kind != ToastKind::Info || t.born.elapsed().as_secs_f32() < 0.3);
        }
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(Align2::RIGHT_BOTTOM, vec2(-20.0, -20.0))
            .order(Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                for toast in self.toasts.iter().rev().take(4) {
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

pub fn state_label(state: State, p: &Palette) -> (&'static str, Color32) {
    match state {
        State::Working => ("Работает", p.positive),
        State::Paused => ("На паузе", p.warning),
        State::Stopped => ("Служба остановлена", p.negative),
        State::NotInstalled => ("Служба не установлена", p.negative),
    }
}

fn nav_item(ui: &mut Ui, p: &Palette, icon: &str, title: &str, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
    let t = ui.ctx().animate_bool_responsive(response.id, selected);
    if selected || response.hovered() {
        let fill = if selected {
            p.accent.gamma_multiply(if p.dark { 0.22 } else { 0.14 })
        } else {
            p.hover
        };
        ui.painter().rect_filled(rect, CornerRadius::same(10), fill);
    }
    if t > 0.0 {
        let bar = egui::Rect::from_min_size(rect.left_top() + vec2(0.0, 10.0), vec2(3.5, 20.0 * t));
        ui.painter()
            .rect_filled(bar, CornerRadius::same(2), p.accent);
    }
    let color = if selected { p.accent } else { p.text };
    ui.painter().text(
        rect.left_center() + vec2(16.0, 0.0),
        Align2::LEFT_CENTER,
        icon,
        widgets::icon_font(19.0),
        color,
    );
    ui.painter().text(
        rect.left_center() + vec2(46.0, 0.0),
        Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(15.5),
        if selected { p.accent } else { p.text },
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.snap = self.link.snapshot();
        while let Ok(toast) = self.toast_rx.try_recv() {
            self.toasts.push(toast);
        }
        let p = self.palette(ui.ctx());
        egui::Panel::left("nav")
            .exact_size(236.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(p.sidebar).inner_margin(Margin {
                left: 14,
                right: 14,
                top: 20,
                bottom: 16,
            }))
            .show(ui, |ui| self.sidebar(ui, &p));
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(p.bg))
            .show(ui, |ui| {
                ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        Frame::NONE
                            .inner_margin(Margin {
                                left: 34,
                                right: 34,
                                top: 28,
                                bottom: 34,
                            })
                            .show(ui, |ui| {
                                ui.set_max_width(ui.available_width().min(940.0));
                                match self.page {
                                    Page::Home => pages::home::show(self, ui, &p),
                                    Page::Devices => pages::devices::show(self, ui, &p),
                                    Page::Settings => pages::settings::show(self, ui, &p),
                                    Page::Help => pages::help::show(self, ui, &p),
                                    Page::Log => pages::log::show(self, ui, &p),
                                }
                            });
                    });
            });
        let ctx = ui.ctx().clone();
        self.draw_toasts(&ctx, &p);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.theme.unsaved() {
            self.save_theme();
        }
    }
}

/// Unix-время → «ЧЧ:ММ:СС» по местному времени.
pub fn clock(ts: u64) -> String {
    let t = libc::time_t::try_from(ts).unwrap_or_default();
    // SAFETY: localtime_r пишет в локальную структуру.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return String::from("--:--:--");
    }
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_differ_only_on_real_mismatch() {
        assert!(!versions_differ("0.4.0", "0.4.0"));
        assert!(!versions_differ("0.4.0", " 0.4.0\n"));
        assert!(!versions_differ("0.4.0", "v0.4.0"));
        assert!(versions_differ("0.4.0", "0.3.0"));
        assert!(versions_differ("0.4.0", "0.4.1"));
        assert!(versions_differ("0.4.0", "0.4.0-dev"));
        // Служба версию не сообщила — не о чем предупреждать.
        assert!(!versions_differ("0.4.0", ""));
        assert!(!versions_differ("0.4.0", "  "));
    }

    #[test]
    fn short_path_replaces_home() {
        let home = Some(Path::new("/home/u"));
        let p = |s: &str| short_path(Path::new(s), home);
        assert_eq!(
            p("/home/u/.config/shiftfix/config.toml"),
            "~/.config/shiftfix/config.toml"
        );
        assert_eq!(p("/home/u"), "~");
        // Соседний каталог с тем же началом имени — не домашний.
        assert_eq!(p("/home/u2/config.toml"), "/home/u2/config.toml");
        assert_eq!(p("/etc/shiftfix.toml"), "/etc/shiftfix.toml");
        let path = Path::new("/home/u/x");
        assert_eq!(short_path(path, None), "/home/u/x");
        assert_eq!(short_path(path, Some(Path::new(""))), "/home/u/x");
    }
}

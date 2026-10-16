//! Окно: боковая панель, страницы, всплывающие сообщения.

use std::path::PathBuf;
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
use shiftfix::portal::PortalState;

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

/// Тема пишется в файл не сразу: пипетка меняет цвет каждый кадр.
const THEME_SAVE_DELAY: Duration = Duration::from_millis(400);

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
    /// Конфиг, в котором хранится тема (тот же, откуда прочитаны настройки).
    theme_path: PathBuf,
    /// Режим из `--theme`: только на этот запуск.
    forced_mode: Option<ThemeMode>,
    /// Поток портала: тема и акцент системы.
    watch: theme::SystemWatch,
    /// Ответ портала: «ещё не ответил» и «недоступен» — для подсказок в окне.
    pub portal: PortalState,
    /// Что сообщила система о теме и акценте (из `portal`).
    pub system: theme::System,
    /// Палитра, уже применённая к egui.
    applied: Option<Palette>,
    /// Когда записать изменённую тему.
    theme_save_at: Option<Instant>,
    /// О неудачной записи темы уже сказали.
    theme_save_failed: bool,
    /// Вкладка варианта в карточке «Оформление»; `None` — вариант на экране.
    pub theme_tab: Option<theme::Variant>,
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
        let portal = watch.state();
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
            theme_path,
            forced_mode,
            watch,
            system: theme::System::from(&portal),
            portal,
            applied: None,
            theme_save_at: None,
            theme_save_failed: false,
            theme_tab: None,
        }
    }

    /// Режим, в котором окно сейчас: `--theme` или из настроек.
    pub fn theme_mode(&self) -> ThemeMode {
        self.forced_mode.unwrap_or(self.settings.theme.mode)
    }

    /// Режим задан параметром `--theme` (до первого выбора в окне).
    pub fn theme_forced(&self) -> bool {
        self.forced_mode.is_some()
    }

    /// Выбор режима в окне (боковая панель и «Оформление»): отменяет `--theme`,
    /// вкладка «Оформления» снова показывает вариант на экране.
    pub fn set_mode(&mut self, mode: ThemeMode) {
        self.forced_mode = None;
        self.theme_tab = None;
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
        self.theme_save_at = Some(Instant::now() + THEME_SAVE_DELAY);
    }

    fn save_theme(&mut self) {
        self.theme_save_at = None;
        match self.settings.theme.save(&self.theme_path) {
            Ok(()) => self.theme_save_failed = false,
            Err(err) => {
                log::warn!("тема не сохранена: {err:#}");
                if !self.theme_save_failed {
                    self.theme_save_failed = true;
                    self.toast(ToastKind::Error, format!("Тема не сохранилась: {err:#}"));
                }
            }
        }
    }

    /// Цвета кадра: применить к egui, если тема изменилась, и вовремя записать её.
    fn palette(&mut self, ctx: &egui::Context) -> Palette {
        // Система сменила тему — поток портала уже попросил перерисовку.
        let portal = self.watch.state();
        if portal != self.portal {
            self.system = theme::System::from(&portal);
            self.portal = portal;
        }
        let p = theme::resolve(&self.settings.theme, &self.system, self.forced_mode);
        if self.applied != Some(p) {
            theme::apply_palette(ctx, &p);
            if self.applied.is_none_or(|old| old.dark != p.dark) {
                // Рамка окна в GNOME (CSD) — в тон.
                let theme = if p.dark {
                    egui::SystemTheme::Dark
                } else {
                    egui::SystemTheme::Light
                };
                ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(theme));
            }
            self.applied = Some(p);
        }
        let now = Instant::now();
        let pointer_down = ctx.input(|i| i.pointer.any_down());
        if theme::save_due(self.theme_save_at, now, pointer_down) {
            self.save_theme();
        } else if let Some(at) = self.theme_save_at {
            ctx.request_repaint_after(
                at.saturating_duration_since(now)
                    .max(Duration::from_millis(50)),
            );
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
                || format!("версия {}", env!("CARGO_PKG_VERSION")),
                |s| format!("служба {} · pid {}", s.version, s.pid),
            );
            ui.label(RichText::new(version).size(12.0).color(p.weak));
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
        if self.theme_save_at.is_some() {
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

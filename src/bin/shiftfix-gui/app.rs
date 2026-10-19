//! Окно: боковая панель, страницы, всплывающие сообщения.

use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Frame, Image, Layout, Margin, RichText, ScrollArea,
    Sense, Stroke, TextureHandle, TextureOptions, Ui, load::SizedTexture, vec2,
};
use egui_phosphor::regular as icons;
use shiftfix::config::{self, ThemeMode, ThemeSettings};
use shiftfix::devices::{DeviceInfo, Role};
use shiftfix::ipc::{Request, Status};

use crate::edit::SettingsEdit;
use crate::format::versions_differ;
use crate::link::{Check, Link, ServiceAction, Snapshot};
use crate::pages;
use crate::theme::{self, Palette};
use crate::toast::{Toast, ToastKind, Toasts};
use crate::update::{self, Update};
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

    /// Имя для `--page` (его же передаёт трей).
    pub fn name(self) -> &'static str {
        match self {
            Page::Home => "home",
            Page::Devices => "devices",
            Page::Settings => "settings",
            Page::Help => "help",
            Page::Log => "log",
        }
    }

    pub fn parse(name: &str) -> Option<Page> {
        Page::ALL.into_iter().find(|p| p.name() == name)
    }
}

/// Версия окна; у службы своя — в `Status.version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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
    toasts: Toasts,
    /// Правка config.toml окна: отсюда читаются настройки и тема, сюда же пишутся.
    pub settings: SettingsEdit,
    pub try_text: String,
    pub check: Check,
    /// Новая версия в папке с исходниками и запуск обновления.
    pub update: Update,
    logo: TextureHandle,
    /// Тема окна: портал, применённая палитра, отложенная запись.
    pub theme: theme::ThemeState,
    /// Прокрутить «Инструкцию» к блоку «О программе» (клик по версии).
    pub about_focus: bool,
    /// Вкладки из `--page` повторных запусков, пока окно открыто.
    raised: Receiver<Page>,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        page: Page,
        raised: Receiver<Page>,
        forced_mode: Option<ThemeMode>,
        watch: theme::SystemWatch,
    ) -> App {
        let ctx = &cc.egui_ctx;
        let (toasts, toast_tx) = Toasts::new();
        let logo_image =
            egui::ColorImage::from_rgba_unmultiplied([96, 96], &shiftfix::icon::rgba(96, true));
        let logo = ctx.load_texture("logo", logo_image, TextureOptions::LINEAR);
        let mut settings = SettingsEdit::load(config::default_path());
        let link = Link::start(ctx, toast_tx, settings.path().to_owned());
        let mut theme = settings.current.theme.clone();
        theme::migrate_legacy(settings.path(), &mut theme);
        for id in theme::unknown_palettes(&theme) {
            log::warn!("[theme]: неизвестная палитра «{id}» — вместо неё Breeze");
        }
        settings.set_theme(theme);
        let update = Update::default();
        update.check(ctx);
        App {
            page,
            link,
            snap: Snapshot::default(),
            toasts,
            theme: theme::ThemeState::new(settings.path().to_owned(), forced_mode, watch),
            settings,
            try_text: String::new(),
            check: Check::default(),
            update,
            logo,
            about_focus: false,
            raised,
        }
    }

    /// Режим, в котором окно сейчас: `--theme` или из настроек.
    pub fn theme_mode(&self) -> ThemeMode {
        self.theme.mode(&self.settings.current.theme)
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
        let mut theme = self.settings.current.theme.clone();
        change(&mut theme);
        if theme == self.settings.current.theme {
            return;
        }
        self.settings.set_theme(theme);
        self.theme.changed();
    }

    fn save_theme(&mut self) {
        if let Some(err) = self.theme.save(&self.settings.current.theme) {
            self.toast(ToastKind::Error, format!("Тема не сохранилась: {err}"));
        }
    }

    /// Цвета кадра: применить к egui, если тема изменилась, и вовремя записать её.
    fn palette(&mut self, ctx: &egui::Context) -> Palette {
        let p = self.theme.apply(ctx, &self.settings.current.theme);
        if self.theme.time_to_save(ctx) {
            self.save_theme();
        }
        p
    }

    /// Конфиг службы, если это не файл окна: службу запустили с `--config`.
    /// Окно в него не пишет — только говорит, что служба читает другой файл.
    pub fn service_config(&self) -> Option<PathBuf> {
        let theirs = PathBuf::from(&self.status()?.config_path);
        let ours = self.settings.path();
        let same = theirs.as_os_str().is_empty()
            || theirs == ours
            || fs::canonicalize(&theirs)
                .is_ok_and(|t| fs::canonicalize(ours).is_ok_and(|o| o == t));
        (!same).then_some(theirs)
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

    /// Устройства: от службы (с активностью) или найденные окном, пока служба
    /// не запущена. `None` — окно их ещё не нашло.
    pub fn devices(&self) -> Option<&[DeviceInfo]> {
        match self.status() {
            Some(status) => Some(&status.devices),
            None => self.snap.local_devices.as_deref(),
        }
    }

    /// Сохранить настройки в файл и попросить службу перечитать их.
    pub fn save_settings(&mut self, success: &'static str) {
        let result = self.settings.save();
        self.after_save(result, success);
    }

    /// Роль устройства вручную (`None` — снова автоматически).
    pub fn set_device_role(&mut self, key: &str, role: Option<Role>) {
        let result = self.settings.set_role(key, role);
        self.after_save(result, "Роль устройства сохранена");
    }

    /// Записалось — попросить службу перечитать конфиг, нет — сообщить.
    fn after_save(&mut self, result: anyhow::Result<()>, success: &'static str) {
        match result {
            Ok(()) => {
                if self.status().is_some() {
                    self.link.send(Request::Reload, Some(success));
                } else {
                    self.link.poll_now(); // роли устройств — по новому файлу
                    self.toast(
                        ToastKind::Success,
                        format!("{success}. Применится при запуске службы"),
                    );
                }
            }
            Err(err) => self.toast(ToastKind::Error, format!("Не сохранилось: {err:#}")),
        }
    }

    pub fn service(&mut self, action: ServiceAction) {
        let id = self.toasts.start_action();
        self.link.service(action, id);
    }

    /// «Обновить»: git pull и install.sh в терминале, в папке с исходниками.
    pub fn run_update(&mut self) {
        let Some(source) = self.update.state().source else {
            return;
        };
        match update::open_terminal(&source) {
            Ok(terminal) => self.toast(
                ToastKind::Success,
                format!("Обновление идёт в терминале ({terminal})"),
            ),
            Err(err) => self.toast(ToastKind::Error, err),
        }
    }

    // ─── отрисовка ───

    fn sidebar(&mut self, ui: &mut Ui, p: &Palette) {
        self.brand(ui, p);
        ui.add_space(22.0);
        for page in Page::ALL {
            if nav_item(ui, p, page.icon(), page.title(), self.page == page).clicked() {
                self.page = page;
            }
            ui.add_space(2.0);
        }
        // Низ панели раскладывается снизу вверх: режим темы, над ним версия
        // и состояние службы.
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            self.theme_buttons(ui, p);
            ui.add_space(6.0);
            self.version_link(ui, p);
            ui.add_space(4.0);
            let (text, color) = state_label(self.state(), p);
            widgets::pill(ui, text, color, color.gamma_multiply(0.14));
        });
    }

    /// Значок и название программы над вкладками.
    fn brand(&self, ui: &mut Ui, p: &Palette) {
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
    }

    /// Режим темы — тремя значками.
    fn theme_buttons(&mut self, ui: &mut Ui, p: &Palette) {
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
    }

    /// Версия службы (или окна, пока служба не отвечает); щелчок открывает
    /// «О программе». Версии различаются — подсвечена.
    fn version_link(&mut self, ui: &mut Ui, p: &Palette) {
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
        let label =
            egui::Label::new(RichText::new(version).size(12.0).color(color)).sense(Sense::click());
        let clicked = ui
            .add(label)
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(hint)
            .clicked();
        if clicked {
            self.page = Page::Help;
            self.about_focus = true;
        }
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
            widgets::selected_fill(p)
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
        if let Some(page) = self.raised.try_iter().last() {
            self.page = page;
        }
        self.snap = self.link.snapshot();
        self.link.show_devices(self.page == Page::Devices);
        self.toasts.receive();
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
        self.toasts.show(&ctx, &p);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.theme.unsaved() {
            self.save_theme();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_names_round_trip_and_are_in_usage() {
        for page in Page::ALL {
            assert_eq!(Page::parse(page.name()), Some(page));
            assert!(crate::USAGE.contains(page.name()), "{}", page.name());
        }
        // Так открывает «Инструкцию» пункт меню в трее.
        assert_eq!(Page::parse("help"), Some(Page::Help));
        assert_eq!(Page::parse("Home"), None);
    }
}

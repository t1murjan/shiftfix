//! Окно: полоса вкладок, вкладки, всплывающие сообщения.

use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Frame, Image, Layout, Margin, ScrollArea, Sense,
    Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, load::SizedTexture, vec2,
};
use egui_phosphor::regular as icons;
use shiftfix::config::{self, ThemeMode, ThemeSettings};
use shiftfix::devices::{DeviceInfo, Role};
use shiftfix::ipc::{Request, Status};

use crate::edit::SettingsEdit;
use crate::link::{Check, Link, ServiceAction, Snapshot};
use crate::pages;
use crate::theme::{self, Palette};
use crate::toast::{Toast, ToastKind, Toasts};
use crate::update::{self, Outcome, Update};
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

/// Что сейчас со службой — для строки над вкладкой.
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
            raised,
        }
    }

    /// Режим, в котором окно сейчас: `--theme` или из настроек.
    pub fn theme_mode(&self) -> ThemeMode {
        self.theme.mode(&self.settings.current.theme)
    }

    /// Выбор режима в окне (кнопка в полосе вкладок и «Оформление»): отменяет `--theme`,
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

    /// Начало кадра: снимок состояния службы, сообщения из фоновых потоков
    /// и цвета (тема применяется к egui, если изменилась, и вовремя пишется).
    pub fn begin_frame(&mut self, ctx: &egui::Context) -> Palette {
        self.snap = self.link.snapshot();
        self.toasts.receive();
        let p = self.theme.apply(ctx, &self.settings.current.theme);
        if self.theme.time_to_save(ctx) {
            self.save_theme();
        }
        p
    }

    /// Всплывающие сообщения поверх кадра.
    pub fn show_toasts(&mut self, ctx: &egui::Context, p: &Palette) {
        self.toasts.show(ctx, p);
    }

    /// Значок программы размером `size`.
    pub fn logo(&self, size: f32) -> Image<'static> {
        Image::from_texture(SizedTexture::new(self.logo.id(), vec2(size, size)))
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

    /// Кадр окна: полоса вкладок слева, над вкладкой — её название,
    /// состояние службы и выключатель, ниже — сама вкладка.
    pub fn show(&mut self, ui: &mut Ui) {
        if let Some(page) = self.raised.try_iter().last() {
            self.page = page;
        }
        let p = self.begin_frame(ui.ctx());
        self.link.show_devices(self.page == Page::Devices);
        egui::Panel::left("tabs")
            .exact_size(58.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                Frame::NONE
                    .fill(p.sidebar)
                    .inner_margin(Margin::symmetric(0, 12)),
            )
            .show(ui, |ui| self.tabs(ui, &p));
        egui::Panel::top("title")
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(p.bg).inner_margin(Margin {
                left: 16,
                right: 16,
                top: 14,
                bottom: 8,
            }))
            .show(ui, |ui| self.title_bar(ui, &p));
        // Несохранённые настройки пропадут, когда окно закроется, поэтому
        // «Сохранить» всегда на виду, а не в конце длинной вкладки.
        if self.page == Page::Settings {
            egui::Panel::bottom("save")
                .resizable(false)
                .show_separator_line(true)
                .frame(
                    Frame::NONE
                        .fill(p.bg)
                        .inner_margin(Margin::symmetric(16, 10)),
                )
                .show(ui, |ui| pages::settings::save_bar(self, ui, &p));
        }
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(p.bg))
            .show(ui, |ui| {
                ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        Frame::NONE
                            .inner_margin(Margin {
                                left: 16,
                                right: 16,
                                top: 4,
                                bottom: 16,
                            })
                            .show(ui, |ui| match self.page {
                                Page::Home => pages::home::show(self, ui, &p),
                                Page::Devices => pages::devices::show(self, ui, &p),
                                Page::Settings => pages::settings::show(self, ui, &p),
                                Page::Help => pages::help::show(self, ui, &p),
                                Page::Log => pages::log::show(self, ui, &p),
                            });
                    });
            });
        let ctx = ui.ctx().clone();
        // Рамка: без неё окно без заголовка сливается с окнами под ним.
        ui.painter().rect_stroke(
            ctx.content_rect(),
            CornerRadius::ZERO,
            Stroke::new(1.0, p.card_border),
            StrokeKind::Inside,
        );
        self.show_toasts(&ctx, &p);
    }

    /// Окно закрывается: недописанную тему — в файл.
    pub fn on_exit(&mut self) {
        if self.theme.unsaved() {
            self.save_theme();
        }
    }

    /// Полоса слева: значок, вкладки значками (название — в подсказке) и
    /// внизу режим темы. Точка на «Главной» — вышла новая версия.
    fn tabs(&mut self, ui: &mut Ui, p: &Palette) {
        let newer = matches!(self.update.state().outcome, Some(Outcome::Newer(_)));
        ui.vertical_centered(|ui| {
            ui.add(self.logo(34.0));
            ui.add_space(14.0);
            for page in Page::ALL {
                let dot = page == Page::Home && newer;
                let tab = tab_button(ui, p, page.icon(), self.page == page, dot);
                if tab.on_hover_text(page.title()).clicked() {
                    self.page = page;
                }
                ui.add_space(4.0);
            }
        });
        ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
            self.theme_button(ui, p);
        });
    }

    /// Режим темы одной кнопкой: щелчок — следующий режим.
    fn theme_button(&mut self, ui: &mut Ui, p: &Palette) {
        let (icon, hint, next) = match self.theme_mode() {
            ThemeMode::System => (icons::CIRCLE_HALF, "Тема как в системе", ThemeMode::Light),
            ThemeMode::Light => (icons::SUN, "Светлая тема", ThemeMode::Dark),
            ThemeMode::Dark => (icons::MOON, "Тёмная тема", ThemeMode::System),
        };
        let button = egui::Button::new(widgets::icon(icon, 18.0).color(p.weak))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(10))
            .min_size(vec2(40.0, 36.0));
        let hint = format!("{hint} — щёлкните, чтобы сменить");
        if ui.add(button).on_hover_text(hint).clicked() {
            self.set_mode(next);
        }
    }

    /// Над вкладкой: её название (у «Главной» — название программы),
    /// состояние службы и выключатель, пока служба отвечает.
    fn title_bar(&mut self, ui: &mut Ui, p: &Palette) {
        let title = match self.page {
            Page::Home => "shiftfix",
            page => page.title(),
        };
        let state = self.state();
        ui.horizontal(|ui| {
            ui.label(bold(title, 19.0).color(p.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if matches!(state, State::Working | State::Paused) {
                    let mut on = state == State::Working;
                    let toggle = widgets::toggle(ui, &mut on, 22.0, p)
                        .on_hover_text("Включить или поставить на паузу");
                    if toggle.changed() {
                        self.link.send(Request::SetEnabled { enabled: on }, None);
                    }
                    ui.add_space(4.0);
                }
                let (text, color) = state_label(state, p);
                widgets::pill(ui, text, color, color.gamma_multiply(0.14));
            });
        });
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

/// Вкладка в полосе слева: значок; `dot` — точка в углу.
fn tab_button(ui: &mut Ui, p: &Palette, icon: &str, selected: bool, dot: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::click());
    if selected || response.hovered() {
        let fill = if selected {
            widgets::selected_fill(p)
        } else {
            p.hover
        };
        ui.painter().rect_filled(rect, CornerRadius::same(10), fill);
    }
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        icon,
        widgets::icon_font(20.0),
        if selected { p.accent } else { p.weak },
    );
    if dot {
        let center = rect.right_top() + vec2(-9.0, 9.0);
        ui.painter().circle_filled(center, 4.0, p.accent);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
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

//! Тема в работающем окне: что сообщила система, какая палитра уже
//! применена к egui и когда записать изменённую тему в config.toml.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use shiftfix::config::{ThemeMode, ThemeSettings};

use super::{Palette, System, SystemWatch, Variant, apply_palette, resolve, save_due};
use crate::portal::PortalState;

/// Тема пишется в файл не сразу: пипетка меняет цвет каждый кадр.
const SAVE_DELAY: Duration = Duration::from_millis(400);

pub struct ThemeState {
    /// Конфиг, в котором хранится тема (тот же, откуда прочитаны настройки).
    path: PathBuf,
    /// Режим из `--theme`: только на этот запуск, до первого выбора в окне.
    forced: Option<ThemeMode>,
    /// Поток портала: тема и акцент системы.
    watch: SystemWatch,
    /// Ответ портала: «ещё не ответил» и «недоступен» — для подсказок в окне.
    pub portal: PortalState,
    /// Что сообщила система о теме и акценте (из `portal`).
    pub system: System,
    /// Палитра, уже применённая к egui.
    applied: Option<Palette>,
    /// Когда записать изменённую тему.
    save_at: Option<Instant>,
    /// О неудачной записи темы уже сказали.
    save_failed: bool,
    /// Вкладка варианта в карточке «Оформление»; `None` — вариант на экране.
    pub tab: Option<Variant>,
}

impl ThemeState {
    pub fn new(path: PathBuf, forced: Option<ThemeMode>, watch: SystemWatch) -> ThemeState {
        let portal = watch.state();
        ThemeState {
            path,
            forced,
            watch,
            system: System::from(&portal),
            portal,
            applied: None,
            save_at: None,
            save_failed: false,
            tab: None,
        }
    }

    /// Режим, в котором окно сейчас: `--theme` или из настроек.
    pub fn mode(&self, t: &ThemeSettings) -> ThemeMode {
        self.forced.unwrap_or(t.mode)
    }

    /// Режим задан параметром `--theme` (до первого выбора в окне).
    pub fn forced(&self) -> bool {
        self.forced.is_some()
    }

    /// Режим выбран в окне: `--theme` больше не действует, вкладка
    /// «Оформления» снова показывает вариант, который на экране.
    pub fn mode_chosen(&mut self) {
        self.forced = None;
        self.tab = None;
    }

    /// Тема изменилась: записать её чуть позже.
    pub fn changed(&mut self) {
        self.save_at = Some(Instant::now() + SAVE_DELAY);
    }

    /// Изменённая тема ещё не записана.
    pub fn unsaved(&self) -> bool {
        self.save_at.is_some()
    }

    /// Цвета кадра; к egui они применяются, только если тема изменилась.
    pub fn apply(&mut self, ctx: &egui::Context, t: &ThemeSettings) -> Palette {
        // Система сменила тему — поток портала уже попросил перерисовку.
        let portal = self.watch.state();
        if portal != self.portal {
            self.system = System::from(&portal);
            self.portal = portal;
        }
        let p = resolve(t, &self.system, self.forced);
        if self.applied != Some(p) {
            apply_palette(ctx, &p);
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
        p
    }

    /// Пора ли записать тему; если нет, но запись ждёт, — перерисовать к сроку.
    pub fn time_to_save(&self, ctx: &egui::Context) -> bool {
        let now = Instant::now();
        let pointer_down = ctx.input(|i| i.pointer.any_down());
        if save_due(self.save_at, now, pointer_down) {
            return true;
        }
        if let Some(at) = self.save_at {
            ctx.request_repaint_after(
                at.saturating_duration_since(now)
                    .max(Duration::from_millis(50)),
            );
        }
        false
    }

    /// Записать секцию `[theme]`. Текст ошибки возвращается только при первой
    /// неудаче подряд: окно скажет о ней один раз, а не после каждой правки.
    pub fn save(&mut self, t: &ThemeSettings) -> Option<String> {
        self.save_at = None;
        match t.save(&self.path) {
            Ok(()) => {
                self.save_failed = false;
                None
            }
            Err(err) => {
                log::warn!("тема не сохранена: {err:#}");
                let first = !self.save_failed;
                self.save_failed = true;
                first.then(|| format!("{err:#}"))
            }
        }
    }
}

//! Тема в работающем окне: что сообщила система, какая палитра уже
//! применена к egui и когда записать изменённую тему в config.toml.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use shiftfix::config::{ThemeMode, ThemeSettings};

use super::system::system_accent_blocked;
use super::{Palette, System, SystemNote, SystemWatch, Variant, apply_palette, resolve};
use crate::portal::PortalState;

/// Тема пишется в файл не сразу: пипетка меняет цвет каждый кадр.
const SAVE_DELAY: Duration = Duration::from_millis(400);

/// Пора ли записать тему: срок вышел и кнопка мыши отпущена. Пока тянут
/// ползунок пипетки, цвет меняется каждый кадр, а файл пишется один раз.
fn save_due(at: Option<Instant>, now: Instant, pointer_down: bool) -> bool {
    at.is_some_and(|at| now >= at) && !pointer_down
}

pub struct ThemeState {
    /// Конфиг, в котором хранится тема (тот же, откуда прочитаны настройки).
    path: PathBuf,
    /// Режим из `--theme`: только на этот запуск, до первого выбора в окне.
    forced: Option<ThemeMode>,
    /// Поток портала: тема и акцент системы.
    watch: SystemWatch,
    /// Ответ портала: «ещё не ответил» и «недоступен» — для подсказок в окне.
    portal: PortalState,
    /// Что сообщила система о теме и акценте (из `portal`).
    system: System,
    /// Палитра, уже применённая к egui.
    applied: Option<Palette>,
    /// Когда записать изменённую тему.
    save_at: Option<Instant>,
    /// О неудачной записи темы уже сказали.
    save_failed: bool,
    /// Вкладка варианта в карточке «Оформление»; `None` — вариант на экране.
    tab: Option<Variant>,
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

    /// Что сообщила система о теме и акценте.
    pub fn system(&self) -> &System {
        &self.system
    }

    /// Строка о теме системы в карточке «Оформление».
    pub fn note(&self) -> SystemNote {
        SystemNote::of(&self.portal)
    }

    /// Почему портал недоступен (подсказка к строке); `None` — доступен или
    /// ещё не ответил.
    pub fn unavailable_reason(&self) -> Option<&str> {
        match &self.portal {
            PortalState::Unavailable(why) => Some(why),
            _ => None,
        }
    }

    /// Почему «Акцент как в системе» нельзя включить; `None` — можно.
    pub fn system_accent_blocked(&self) -> Option<&'static str> {
        system_accent_blocked(&self.portal)
    }

    /// Вкладка варианта в карточке «Оформление»; `None` — вариант на экране.
    pub fn tab(&self) -> Option<Variant> {
        self.tab
    }

    pub fn set_tab(&mut self, v: Variant) {
        self.tab = Some(v);
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::portal::{Appearance, Scheme};

    fn ready(scheme: Scheme) -> PortalState {
        PortalState::Ready(Appearance {
            scheme: Some(scheme),
            accent: None,
        })
    }

    #[test]
    fn forced_mode_until_chosen_then_system() {
        let ctx = egui::Context::default();
        let t = ThemeSettings::default();
        assert_eq!(t.mode, ThemeMode::System);
        let watch = SystemWatch::fixed(ready(Scheme::Light));
        let mut theme = ThemeState::new(PathBuf::from("config.toml"), Some(ThemeMode::Dark), watch);
        // `--theme dark` перекрывает и настройки, и светлую тему системы.
        assert!(theme.forced());
        assert_eq!(theme.mode(&t), ThemeMode::Dark);
        assert!(theme.apply(&ctx, &t).dark);
        theme.set_tab(Variant::Light);
        assert_eq!(theme.tab(), Some(Variant::Light));
        // Режим выбрали в окне: `--theme` больше не действует, вкладка
        // «Оформления» — снова вариант на экране.
        theme.mode_chosen();
        assert!(!theme.forced());
        assert_eq!(theme.mode(&t), ThemeMode::System);
        assert_eq!(theme.tab(), None);
        assert!(!theme.apply(&ctx, &t).dark);
        // Система сменила тему — палитра следует за ней.
        theme.watch.set(ready(Scheme::Dark));
        assert!(theme.apply(&ctx, &t).dark);
        assert_eq!(theme.note(), SystemNote::Dark);
    }

    #[test]
    fn save_error_told_once_in_a_row() {
        let dir = std::env::temp_dir().join(format!("shiftfix-theme-save-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let broken = "double_tap_ms = [\n";
        fs::write(&path, broken).unwrap();
        let watch = SystemWatch::fixed(PortalState::Pending);
        let mut theme = ThemeState::new(path.clone(), None, watch);
        let t = ThemeSettings::default();

        assert!(!theme.unsaved());
        theme.changed();
        assert!(theme.unsaved());
        // Битый конфиг: об ошибке — при первой неудаче, а не после каждой правки.
        assert!(theme.save(&t).is_some());
        assert!(!theme.unsaved());
        theme.changed();
        assert_eq!(theme.save(&t), None);
        assert_eq!(fs::read_to_string(&path).unwrap(), broken);
        // Файл исправили — тема записана; новая ошибка — снова сказать.
        fs::write(&path, "double_tap_ms = 350\n").unwrap();
        assert_eq!(theme.save(&t), None);
        assert!(fs::read_to_string(&path).unwrap().contains("[theme]"));
        fs::write(&path, broken).unwrap();
        assert!(theme.save(&t).is_some());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn save_waits_for_deadline_and_release() {
        let now = Instant::now();
        let later = now + Duration::from_millis(400);
        assert!(!save_due(None, later, false), "сохранять нечего");
        assert!(!save_due(Some(later), now, false), "срок не вышел");
        assert!(!save_due(Some(now), later, true), "ползунок ещё тянут");
        assert!(save_due(Some(now), later, false));
        assert!(save_due(Some(now), now, false), "ровно в срок");
    }
}

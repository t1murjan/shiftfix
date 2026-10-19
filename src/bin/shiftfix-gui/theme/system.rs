//! Тема системы из XDG Desktop Portal: поток портала пишет, окно читает
//! каждый кадр; подписи о том, что сообщила система.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32};

use crate::portal::{self, PortalState, Scheme};

/// Что сообщила система (XDG Desktop Portal). По умолчанию — ничего:
/// портал не ответил, недоступен или не знает этих ключей.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct System {
    /// color-scheme: `Some(true)` — тёмная, `Some(false)` — светлая,
    /// `None` — предпочтения нет.
    pub dark: Option<bool>,
    /// accent-color, если система его сообщает.
    pub accent: Option<Color32>,
}

/// Пока портал не ответил (или недоступен), цвета — как без портала:
/// тёмная тема и свой акцент. Чем «ещё не ответил» отличается от
/// «недоступен», окно узнаёт из `SystemNote`.
impl From<&PortalState> for System {
    fn from(state: &PortalState) -> System {
        let PortalState::Ready(a) = state else {
            return System::default();
        };
        System {
            dark: a.scheme.and_then(|scheme| match scheme {
                Scheme::Dark => Some(true),
                Scheme::Light => Some(false),
                Scheme::NoPreference => None,
            }),
            accent: a.accent.map(|[r, g, b]| Color32::from_rgb(r, g, b)),
        }
    }
}

/// Сколько окно при запуске ждёт первый ответ портала, чтобы не мигнуть
/// тёмной темой. Дольше — открываемся тёмными, тема сменится на лету.
const FIRST_ANSWER: Duration = Duration::from_millis(250);

/// Тема системы из портала: поток портала пишет, кадр читает.
pub struct SystemWatch {
    shared: Arc<Shared>,
    started: Instant,
}

#[derive(Default)]
struct Shared {
    state: Mutex<PortalState>,
    answered: Condvar,
    /// Окно, которое надо перерисовать при смене темы системы.
    ctx: OnceLock<egui::Context>,
}

fn lock(state: &Mutex<PortalState>) -> MutexGuard<'_, PortalState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

impl SystemWatch {
    /// Начать слушать портал (сессионная шина) — ещё до создания окна.
    pub fn start() -> SystemWatch {
        let shared = Arc::new(Shared::default());
        let writer = Arc::clone(&shared);
        let spawned = portal::spawn(None, move |state| {
            *lock(&writer.state) = state;
            writer.answered.notify_all();
            if let Some(ctx) = writer.ctx.get() {
                ctx.request_repaint();
            }
        });
        if let Err(err) = spawned {
            log::warn!("портал настроек: поток не запущен: {err} — тёмная тема");
            *lock(&shared.state) = PortalState::Unavailable(err.to_string());
        }
        SystemWatch {
            shared,
            started: Instant::now(),
        }
    }

    /// Перерисовывать окно, когда сменится тема системы. Первый ответ
    /// подождать, но не дольше `FIRST_ANSWER` с запуска.
    pub fn attach(&self, ctx: &egui::Context) {
        let _ = self.shared.ctx.set(ctx.clone());
        let left = FIRST_ANSWER.saturating_sub(self.started.elapsed());
        let guard = lock(&self.shared.state);
        let _ = self
            .shared
            .answered
            .wait_timeout_while(guard, left, |s| *s == PortalState::Pending);
    }

    /// Что портал сообщил на сейчас.
    pub fn state(&self) -> PortalState {
        lock(&self.shared.state).clone()
    }

    /// Для тестов: портал не запускается, ответ задаёт тест.
    #[cfg(test)]
    pub(super) fn fixed(state: PortalState) -> SystemWatch {
        let watch = SystemWatch {
            shared: Arc::default(),
            started: Instant::now(),
        };
        watch.set(state);
        watch
    }

    /// Для тестов: портал ответил `state`.
    #[cfg(test)]
    pub(super) fn set(&self, state: PortalState) {
        *lock(&self.shared.state) = state;
    }
}

/// Что окно знает о теме системы — строка в карточке «Оформление».
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemNote {
    /// Портал ещё не ответил.
    Waiting,
    /// Портала нет или он не знает color-scheme: тёмная.
    NotReported,
    /// Портал есть, но тема в системе не выбрана: тёмная.
    NoPreference,
    Dark,
    Light,
}

impl SystemNote {
    pub fn of(portal: &PortalState) -> SystemNote {
        match portal {
            PortalState::Pending => SystemNote::Waiting,
            PortalState::Unavailable(_) => SystemNote::NotReported,
            PortalState::Ready(a) => match a.scheme {
                Some(Scheme::Dark) => SystemNote::Dark,
                Some(Scheme::Light) => SystemNote::Light,
                Some(Scheme::NoPreference) => SystemNote::NoPreference,
                None => SystemNote::NotReported,
            },
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            SystemNote::Waiting => "Ждём ответа системы…",
            SystemNote::NotReported => "Система не сообщает тему — используется тёмная.",
            SystemNote::NoPreference => "В системе тема не выбрана — используется тёмная.",
            SystemNote::Dark => "Сейчас в системе тёмная тема.",
            SystemNote::Light => "Сейчас в системе светлая тема.",
        }
    }

    /// Тема выбрана не системой, а запасная.
    pub fn fallback(self) -> bool {
        matches!(self, SystemNote::NotReported | SystemNote::NoPreference)
    }
}

/// Почему «Акцент как в системе» нельзя включить; `None` — можно. Пока портал
/// не ответил, переключатель не выключается.
pub fn system_accent_blocked(portal: &PortalState) -> Option<&'static str> {
    match portal {
        PortalState::Pending => None,
        PortalState::Unavailable(_) => Some("Система не сообщает тему и акцент."),
        PortalState::Ready(a) if a.accent.is_none() => {
            Some("Система не сообщает свой акцент (accent-color).")
        }
        PortalState::Ready(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_notes() {
        use crate::portal::Appearance;
        let ready = |scheme, accent| PortalState::Ready(Appearance { scheme, accent });
        let cases = [
            (PortalState::Pending, SystemNote::Waiting, None),
            (
                PortalState::Unavailable("нет шины".into()),
                SystemNote::NotReported,
                Some(()),
            ),
            // Портал есть, но color-scheme не знает: тему система не сообщает.
            (ready(None, None), SystemNote::NotReported, Some(())),
            (
                ready(Some(Scheme::NoPreference), Some([1, 2, 3])),
                SystemNote::NoPreference,
                None,
            ),
            (ready(Some(Scheme::Dark), None), SystemNote::Dark, Some(())),
            (
                ready(Some(Scheme::Light), Some([1, 2, 3])),
                SystemNote::Light,
                None,
            ),
        ];
        for (state, note, blocked) in cases {
            assert_eq!(SystemNote::of(&state), note, "{state:?}");
            assert_eq!(
                system_accent_blocked(&state).map(|_| ()),
                blocked,
                "{state:?}"
            );
        }
        // Запасная тёмная — только когда система тему не сообщила.
        assert!(SystemNote::NotReported.fallback());
        assert!(SystemNote::NoPreference.fallback());
        for note in [SystemNote::Waiting, SystemNote::Dark, SystemNote::Light] {
            assert!(!note.fallback(), "{note:?}");
        }
        assert!(SystemNote::NotReported.text().contains("тёмная"));
    }
}

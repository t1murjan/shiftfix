//! Значок в системном трее (StatusNotifierItem): пауза, настройки, выход.
//! Работает в Plasma, а также в Waybar и других панелях с поддержкой SNI.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{CheckmarkItem, StandardItem};
use ksni::{Icon, MenuItem, OfflineReason, ToolTip, Tray};
use log::{info, warn};

use super::Msg;
use crate::icon;
use crate::sys::{paths, proc};

struct ShiftTray {
    tx: Sender<Msg>,
    enabled: bool,
    layouts: String,
    /// Панель с треем сейчас есть (её может не быть при входе — служба стартует раньше).
    visible: Arc<AtomicBool>,
}

fn open_gui(page: Option<&str>) {
    let gui = paths::sibling_binary("shiftfix-gui");
    let page_args = match page {
        Some(page) => vec!["--page", page],
        None => Vec::new(),
    };
    // Под systemd окно запускается в отдельном scope: иначе перезапуск службы
    // (например, кнопкой из этого же окна) закрыл бы и его.
    let in_service = std::env::var_os("INVOCATION_ID").is_some() && paths::in_path("systemd-run");
    let result = if in_service {
        let gui_path = gui.to_string_lossy();
        let mut args = vec!["--user", "--scope", "--quiet", "--collect", &gui_path];
        args.extend(&page_args);
        proc::spawn_detached("systemd-run", &args)
    } else {
        proc::spawn_detached(&gui, &page_args)
    };
    if let Err(err) = result {
        warn!("не удалось открыть {}: {err:#}", gui.display());
    }
}

impl Tray for ShiftTray {
    fn id(&self) -> String {
        "shiftfix".into()
    }

    fn title(&self) -> String {
        "shiftfix".into()
    }

    fn icon_name(&self) -> String {
        if self.enabled {
            "shiftfix"
        } else {
            "shiftfix-paused"
        }
        .into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        // Если тема не знает «shiftfix», панель возьмёт эти картинки.
        [22, 32, 48, 64]
            .into_iter()
            .map(|size| Icon {
                width: size,
                height: size,
                data: icon::argb(size as u32, self.enabled),
            })
            .collect()
    }

    fn tool_tip(&self) -> ToolTip {
        let description = if self.enabled {
            format!("Двойной Shift исправляет раскладку\n{}", self.layouts)
        } else {
            "На паузе — щёлкните колесом, чтобы включить".into()
        };
        ToolTip {
            title: "shiftfix".into(),
            description,
            ..Default::default()
        }
    }

    fn watcher_online(&self) {
        info!("панель с треем появилась — значок показан");
        self.visible.store(true, Ordering::Relaxed);
    }

    fn watcher_offline(&self, reason: OfflineReason) -> bool {
        if self.visible.swap(false, Ordering::Relaxed) {
            info!("панели с треем нет ({reason:?}) — значок появится, когда она запустится");
        }
        true // ждать панель, а не завершаться
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        open_gui(None);
    }

    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(Msg::TogglePause);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            CheckmarkItem {
                label: "Исправлять раскладку".into(),
                checked: self.enabled,
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.send(Msg::TogglePause);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Открыть shiftfix…".into(),
                icon_name: "configure".into(),
                activate: Box::new(|_: &mut Self| open_gui(None)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Инструкция".into(),
                icon_name: "help-contents".into(),
                activate: Box::new(|_: &mut Self| open_gui(Some("help"))),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Выйти".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.send(Msg::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

pub(crate) struct TrayIcon {
    handle: Handle<ShiftTray>,
    visible: Arc<AtomicBool>,
}

impl TrayIcon {
    pub fn spawn(tx: Sender<Msg>, enabled: bool) -> Option<TrayIcon> {
        let visible = Arc::new(AtomicBool::new(true));
        let tray = ShiftTray {
            tx,
            enabled,
            layouts: String::new(),
            visible: visible.clone(),
        };
        // При входе служба часто стартует раньше панели: не сдаваться, а ждать её.
        match tray.assume_sni_available(true).spawn() {
            Ok(handle) => {
                if visible.load(Ordering::Relaxed) {
                    info!("значок в трее показан");
                }
                Some(TrayIcon { handle, visible })
            }
            Err(err) => {
                warn!("значок в трее недоступен: {err}");
                None
            }
        }
    }

    /// Значок действительно показан на панели.
    pub(crate) fn visible(&self) -> bool {
        self.visible.load(Ordering::Relaxed)
    }

    pub(crate) fn update(&self, enabled: bool, layouts: &str) {
        let layouts = layouts.to_owned();
        self.handle.update(move |tray| {
            tray.enabled = enabled;
            tray.layouts = layouts;
        });
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        let _ = self.handle.shutdown();
    }
}

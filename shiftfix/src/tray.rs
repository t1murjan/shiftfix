//! Значок в системном трее (StatusNotifierItem): пауза, настройки, выход.
//! Работает в Plasma, а также в Waybar и других панелях с поддержкой SNI.

use std::sync::mpsc::Sender;

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{CheckmarkItem, StandardItem};
use ksni::{Icon, MenuItem, ToolTip, Tray};
use log::{info, warn};

use crate::daemon::Msg;
use crate::env;
use crate::icon;

struct ShiftTray {
    tx: Sender<Msg>,
    enabled: bool,
    layouts: String,
}

fn open_gui(page: Option<&str>) {
    let gui = env::sibling_binary("shiftfix-gui");
    let gui_path = gui.to_string_lossy().into_owned();
    let mut args: Vec<&str> = vec![&gui_path];
    if let Some(page) = page {
        args.extend(["--page", page]);
    }
    // Под systemd окно запускается в отдельном scope: иначе перезапуск службы
    // (например, кнопкой из этого же окна) закрыл бы и его.
    let in_service = std::env::var_os("INVOCATION_ID").is_some() && env::in_path("systemd-run");
    let result = if in_service {
        let mut scoped = vec!["--user", "--scope", "--quiet", "--collect"];
        scoped.extend(args);
        crate::proc::spawn_detached("systemd-run", &scoped)
    } else {
        crate::proc::spawn_detached(&gui, &args[1..])
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

pub struct TrayIcon(Handle<ShiftTray>);

impl TrayIcon {
    pub fn spawn(tx: Sender<Msg>, enabled: bool) -> Option<TrayIcon> {
        match (ShiftTray {
            tx,
            enabled,
            layouts: String::new(),
        })
        .spawn()
        {
            Ok(handle) => {
                info!("значок в трее показан");
                Some(TrayIcon(handle))
            }
            Err(err) => {
                warn!("значок в трее недоступен: {err}");
                None
            }
        }
    }

    pub fn update(&self, enabled: bool, layouts: &str) {
        let layouts = layouts.to_owned();
        self.0.update(move |tray| {
            tray.enabled = enabled;
            tray.layouts = layouts;
        });
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        let _ = self.0.shutdown();
    }
}

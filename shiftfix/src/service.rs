//! Юнит systemd. KDE, GNOME и Hyprland через uwsm поднимают
//! `graphical-session.target` — служба стартует и останавливается вместе с
//! сеансом. Hyprland и Sway без uwsm, а также оконные менеджеры X11 этой цели
//! не знают: тогда служба запускается со входом пользователя (`default.target`)
//! и сама дожидается сеанса.

use std::path::Path;
use std::time::Duration;

use crate::proc;

pub const UNIT: &str = "shiftfix.service";

/// Сеанс поднимает `graphical-session.target`.
pub fn graphical_target_active() -> bool {
    proc::run(
        "systemctl",
        &["--user", "is-active", "--quiet", "graphical-session.target"],
        Duration::from_secs(5),
    )
    .is_ok_and(|out| out.ok)
}

pub fn unit_text(binary: &Path, graphical_target: bool) -> String {
    let binary = binary.to_string_lossy();
    let exec = if binary.contains(char::is_whitespace) {
        format!("\"{binary}\"")
    } else {
        binary.into_owned()
    };
    let wanted_by = if graphical_target {
        "graphical-session.target"
    } else {
        "graphical-session.target default.target"
    };
    format!(
        "[Unit]
Description=shiftfix: double Shift fixes the layout of the last word or selection
PartOf=graphical-session.target
After=graphical-session.target

[Service]
ExecStart={exec}
Restart=on-failure
RestartSec=3
# Код 2 — ошибка настройки (нет доступа к устройствам и т.п.), перезапуск не поможет.
RestartPreventExitStatus=2

[Install]
WantedBy={wanted_by}
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_depends_on_session_kind() {
        let unit = unit_text(Path::new("/home/u/.local/bin/shiftfix"), true);
        assert!(unit.contains("ExecStart=/home/u/.local/bin/shiftfix\n"));
        assert!(unit.contains("WantedBy=graphical-session.target\n"));
        let unit = unit_text(Path::new("/home/my user/bin/shiftfix"), false);
        assert!(unit.contains("ExecStart=\"/home/my user/bin/shiftfix\"\n"));
        assert!(unit.contains("WantedBy=graphical-session.target default.target\n"));
    }
}

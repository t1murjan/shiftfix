//! Юнит systemd. KDE, GNOME и Hyprland через uwsm поднимают
//! `graphical-session.target` — служба стартует и останавливается вместе с
//! сеансом. Hyprland и Sway без uwsm, а также оконные менеджеры X11 этой цели
//! не знают: тогда служба запускается со входом пользователя (`default.target`)
//! и сама дожидается сеанса.

use std::path::Path;
use std::time::Duration;

use crate::sys::proc;

pub const UNIT: &str = "shiftfix.service";

/// Код выхода службы «ошибка настройки»: нет доступа к устройствам, неверный
/// конфиг или параметры. Перезапуск тут не поможет, и юнит запрещает его
/// (`RestartPreventExitStatus`).
pub const EXIT_SETUP: u8 = 2;

/// Сеанс поднимает `graphical-session.target`.
pub fn graphical_target_active() -> bool {
    proc::run(
        "systemctl",
        &["--user", "is-active", "--quiet", "graphical-session.target"],
        Duration::from_secs(5),
    )
    .is_ok_and(|out| out.ok)
}

/// Файл автозапуска сеанса (`~/.config/autostart/shiftfix.desktop`) — там,
/// где нет пользовательского systemd. Один текст у install.sh и окна.
pub fn autostart_text(binary: &Path) -> String {
    format!(
        "[Desktop Entry]
Type=Application
Name=shiftfix
Exec={}
Icon=shiftfix
NoDisplay=true
X-GNOME-Autostart-enabled=true
",
        desktop_exec(&binary.to_string_lossy())
    )
}

/// Путь для `Exec=` по спецификации Desktop Entry: с пробелами и служебными
/// символами — в кавычках; `"`, `` ` ``, `$` и `\` внутри кавычек экранируются,
/// и обратная косая черта ещё раз удваивается (так значение читается из файла).
fn desktop_exec(path: &str) -> String {
    const RESERVED: &str = " \t\n\r\"'\\><~|&;$*?#()`";
    let path = path.replace('%', "%%");
    if !path.contains(|c| RESERVED.contains(c)) {
        return path;
    }
    let mut out = String::from("\"");
    for c in path.chars() {
        match c {
            '"' | '`' | '$' => {
                out.push_str("\\\\");
                out.push(c);
            }
            '\\' => out.push_str("\\\\\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Путь для `ExecStart=`: `%` удваивается (иначе systemd подставит
/// спецификатор), с пробелами, кавычками и `\` — в двойных кавычках, где `\`
/// и `"` экранируются. Кавычки и `\` в имени программы systemd всё равно не
/// примет, но скажет об этом прямо, а не запустит другой путь.
fn unit_exec(path: &str) -> String {
    let path = path.replace('%', "%%");
    if !path.contains(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '\\')) {
        return path;
    }
    let mut out = String::from("\"");
    for c in path.chars() {
        if matches!(c, '"' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

pub fn unit_text(binary: &Path, graphical_target: bool) -> String {
    let exec = unit_exec(&binary.to_string_lossy());
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
# Код {EXIT_SETUP} — ошибка настройки (нет доступа к устройствам и т.п.), перезапуск не поможет.
RestartPreventExitStatus={EXIT_SETUP}

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

    #[test]
    fn autostart_entry() {
        let text = autostart_text(Path::new("/home/u/.local/bin/shiftfix"));
        assert!(text.starts_with("[Desktop Entry]\n"));
        assert!(text.contains("\nType=Application\n"));
        assert!(text.contains("\nExec=/home/u/.local/bin/shiftfix\n"));
        assert!(text.contains("\nIcon=shiftfix\n"));
        assert!(text.ends_with("\nX-GNOME-Autostart-enabled=true\n"));
        let text = autostart_text(Path::new("/home/my user/bin/shiftfix"));
        assert!(text.contains("\nExec=\"/home/my user/bin/shiftfix\"\n"));
    }

    #[test]
    fn autostart_exec_escaping() {
        assert_eq!(desktop_exec("/opt/100%/shiftfix"), "/opt/100%%/shiftfix");
        assert_eq!(desktop_exec("/a b/$x"), r#""/a b/\\$x""#);
        assert_eq!(desktop_exec(r#"/a"b"#), r#""/a\\"b""#);
        assert_eq!(desktop_exec(r"/a\b"), r#""/a\\\\b""#);
    }

    #[test]
    fn unit_exec_escaping() {
        assert_eq!(unit_exec("/home/u/bin/shiftfix"), "/home/u/bin/shiftfix");
        assert_eq!(unit_exec("/opt/100%/shiftfix"), "/opt/100%%/shiftfix");
        assert_eq!(unit_exec("/opt/$x/shiftfix"), "/opt/$x/shiftfix");
        assert_eq!(unit_exec("/a b/shiftfix"), r#""/a b/shiftfix""#);
        assert_eq!(unit_exec(r#"/a"b"#), r#""/a\"b""#);
        assert_eq!(unit_exec(r"/a\b"), r#""/a\\b""#);
        assert_eq!(unit_exec("/a'b"), r#""/a'b""#);
    }
}

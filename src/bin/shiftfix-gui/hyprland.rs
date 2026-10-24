//! Попап у значка в трее в Hyprland: где курсор и рабочая область монитора
//! под ним и как поставить окно попапа в нужную точку. Всё — через `hyprctl`.
//!
//! В Wayland окно не выбирает себе место само, а правила окна в конфиге
//! Hyprland пишет пользователь (README). Поэтому попап, когда его окно
//! появилось, просит Hyprland сделать окно плавающим, передвинуть его и
//! закрепить.
//!
//! Команды `hyprctl dispatch` пишутся по-разному. С `hyprland.conf` (и во всех
//! версиях до 0.55) — `dispatch setfloating address:0x…`; с `hyprland.lua`
//! (0.55 и новее) `dispatch` — это `hl.dispatch(…)`, и команды пишутся на Lua:
//! `dispatch hl.dsp.window.float({ … })`. Какой конфиг у пользователя, отдельно
//! не спрашиваем: сначала команды для `hyprland.conf`, не вышло — для Lua.
//! Чужие команды Hyprland не выполняет: Lua не разберёт `setfloating …`, а
//! с `hyprland.conf` нет диспетчера `hl.dsp…`.

use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use shiftfix::sys::proc;

const HYPRCTL: &str = "hyprctl";

/// Класс окна попапа (`with_app_id` в popup.rs).
const POPUP_CLASS: &str = "shiftfix-popup";

/// Сколько ждать окно попапа в списке окон Hyprland и как часто спрашивать.
const WINDOW_WAIT: Duration = Duration::from_secs(2);
const WINDOW_POLL: Duration = Duration::from_millis(50);

/// Это Hyprland: задана `HYPRLAND_INSTANCE_SIGNATURE`.
#[allow(dead_code)] // подключает popup.rs, allow снимут вместе с подключением
pub fn running() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some_and(|sig| !sig.is_empty())
}

/// Курсор и рабочая область монитора под ним — `[x, y, ширина, высота]` без
/// зарезервированных краёв (панель вроде waybar). Всё — в глобальных
/// координатах Hyprland, в логических пикселях: в них же ждёт точку
/// `place_popup`.
#[allow(dead_code)] // подключает popup.rs, allow снимут вместе с подключением
pub fn cursor_and_area() -> Result<((i32, i32), [i32; 4])> {
    query_cursor_and_area(HYPRCTL)
}

/// Поставить окно попапа (класс `shiftfix-popup`) левым верхним углом в
/// `(x, y)`: сделать плавающим, передвинуть и закрепить (pin — попап виден на
/// любом рабочем столе).
///
/// Окно появляется у Hyprland, когда нарисован его первый кадр, и функция ждёт
/// его до 2 с. Поэтому звать её не из потока окна: пока он ждёт, кадр не
/// нарисуется.
#[allow(dead_code)] // подключает popup.rs, allow снимут вместе с подключением
pub fn place_popup(x: i32, y: i32) -> Result<()> {
    place_window(HYPRCTL, std::process::id(), (x, y), WINDOW_WAIT)
}

/// `cursor_and_area` с этим `hyprctl` (в тестах — поддельный).
fn query_cursor_and_area(hyprctl: &str) -> Result<((i32, i32), [i32; 4])> {
    let cursor = parse_cursor(&hyprctl_json(hyprctl, "cursorpos")?)
        .context("hyprctl cursorpos: нет x и y")?;
    let monitors = parse_monitors(&hyprctl_json(hyprctl, "monitors")?);
    let monitor =
        monitor_under(&monitors, cursor).context("hyprctl monitors: монитор не найден")?;
    Ok((cursor, monitor.work_area()))
}

/// `place_popup` с этим `hyprctl` для окна процесса `pid`; окно ждать до `wait`.
fn place_window(hyprctl: &str, pid: u32, at: (i32, i32), wait: Duration) -> Result<()> {
    let window = wait_for_window(hyprctl, pid, wait)?;
    let mut replies = Vec::new();
    for syntax in [Syntax::Conf, Syntax::Lua] {
        let commands = place_commands(&window, at, syntax);
        let reply = run_batch(hyprctl, &commands)?;
        if all_ok(&reply, commands.len()) {
            return Ok(());
        }
        replies.push(reply.split_whitespace().collect::<Vec<_>>().join(" "));
    }
    bail!("hyprctl не поставил попап: {}", replies.join(" | "))
}

/// Ответ `hyprctl -j <what>`.
fn hyprctl_json(hyprctl: &str, what: &str) -> Result<Value> {
    let out = proc::run(hyprctl, &["-j", what], proc::TIMEOUT)?;
    out.check(&format!("hyprctl {what}"))?;
    serde_json::from_str(&out.text()).with_context(|| format!("hyprctl {what}: неожиданный ответ"))
}

/// Команды одним вызовом `hyprctl --batch`: Hyprland выполняет их подряд в
/// одном запросе, без кадра между ними. Ответы на команды — через две пустые
/// строки.
fn run_batch(hyprctl: &str, commands: &[String]) -> Result<String> {
    let out = proc::run(hyprctl, &["--batch", &commands.join("; ")], proc::TIMEOUT)?;
    out.check("hyprctl --batch")?;
    Ok(out.text())
}

/// Удача — ответ «ok» на каждую из `count` команд. Искать «ok» внутри ответа
/// нельзя: оно бывает в тексте ошибки.
fn all_ok(reply: &str, count: usize) -> bool {
    let replies: Vec<&str> = reply.split("\n\n\n").map(str::trim).collect();
    replies.len() == count && replies.iter().all(|r| *r == "ok")
}

/// Целое число из ответа `hyprctl`.
fn int(value: &Value) -> Option<i32> {
    value.as_i64().and_then(|v| i32::try_from(v).ok())
}

/// Точка `{"x": …, "y": …}` из `hyprctl -j cursorpos`.
fn parse_cursor(json: &Value) -> Option<(i32, i32)> {
    Some((int(&json["x"])?, int(&json["y"])?))
}

/// Монитор из `hyprctl -j monitors`: место и размер в глобальных координатах
/// Hyprland, в логических пикселях.
#[derive(Debug, PartialEq)]
struct Monitor {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    /// Зарезервированные края (панели): слева, сверху, справа, снизу.
    reserved: [i32; 4],
    focused: bool,
}

/// Мониторы из ответа `hyprctl -j monitors`; монитор без нужных полей
/// пропускается.
fn parse_monitors(json: &Value) -> Vec<Monitor> {
    let monitors = json.as_array().into_iter().flatten();
    monitors.filter_map(parse_monitor).collect()
}

fn parse_monitor(json: &Value) -> Option<Monitor> {
    // width и height — пиксели режима монитора: до поворота (transform 1, 3,
    // 5, 7 — на 90° или 270°) и до масштаба. Логический размер Hyprland
    // считает так же: пиксели / scale с округлением. Старые версии пишут
    // scale с двумя знаками (1.33 вместо 1.333…) — размер выйдет на пару
    // пикселей больше.
    let (mut width, mut height) = (int(&json["width"])?, int(&json["height"])?);
    if json["transform"].as_i64().unwrap_or(0) % 2 == 1 {
        (width, height) = (height, width);
    }
    let scale = json["scale"].as_f64().filter(|s| *s > 0.0)?;
    let logical = |pixels: i32| (f64::from(pixels) / scale).round() as i32;
    let reserved = |side: usize| int(&json["reserved"][side]).unwrap_or(0);
    Some(Monitor {
        x: int(&json["x"])?,
        y: int(&json["y"])?,
        width: logical(width),
        height: logical(height),
        reserved: [reserved(0), reserved(1), reserved(2), reserved(3)],
        focused: json["focused"].as_bool() == Some(true),
    })
}

impl Monitor {
    /// Точка на мониторе. Правый и нижний края — уже соседний монитор.
    fn contains(&self, (x, y): (i32, i32)) -> bool {
        (self.x..self.x + self.width).contains(&x) && (self.y..self.y + self.height).contains(&y)
    }

    /// Монитор без зарезервированных краёв: `[x, y, ширина, высота]`.
    fn work_area(&self) -> [i32; 4] {
        let [left, top, right, bottom] = self.reserved;
        [
            self.x + left,
            self.y + top,
            self.width - left - right,
            self.height - top - bottom,
        ]
    }
}

/// Монитор под курсором, а если курсор ни на одном — тот, что в фокусе.
fn monitor_under(monitors: &[Monitor], cursor: (i32, i32)) -> Option<&Monitor> {
    let under = monitors.iter().find(|m| m.contains(cursor));
    under.or_else(|| monitors.iter().find(|m| m.focused))
}

/// Окно в `hyprctl -j clients`.
#[derive(Debug, PartialEq)]
struct Window {
    address: String,
    pinned: bool,
}

/// Окно попапа процесса `pid`. Hyprland показывает его в списке окон
/// (`hyprctl -j clients`), когда нарисован первый кадр, поэтому спрашиваем
/// раз в `WINDOW_POLL`, пока не пройдёт `wait`.
fn wait_for_window(hyprctl: &str, pid: u32, wait: Duration) -> Result<Window> {
    let deadline = Instant::now() + wait;
    loop {
        if let Some(window) = find_window(&hyprctl_json(hyprctl, "clients")?, pid) {
            return Ok(window);
        }
        if Instant::now() >= deadline {
            bail!(
                "hyprctl clients: окна {POPUP_CLASS} нет и через {} мс",
                wait.as_millis()
            );
        }
        thread::sleep(WINDOW_POLL);
    }
}

/// Окно попапа процесса `pid` в ответе `hyprctl -j clients`.
///
/// Появилось ли окно, видно только по этому списку: `setfloating` с окном,
/// которого нет, в 0.56 отвечает «ok». Кроме класса — pid: это окно этого
/// процесса, даже если окно прежнего попапа ещё не исчезло. Команды потом
/// выбирают окно по адресу (`address:0x…`) — ровно это окно, а `class:` —
/// регулярное выражение по всем окнам.
fn find_window(clients: &Value, pid: u32) -> Option<Window> {
    let ours = |c: &&Value| c["class"] == POPUP_CLASS && c["pid"].as_u64() == Some(pid.into());
    let client = clients.as_array()?.iter().find(ours)?;
    Some(Window {
        address: client["address"]
            .as_str()
            .filter(|a| !a.is_empty())?
            .to_owned(),
        pinned: client["pinned"].as_bool() == Some(true),
    })
}

/// Как пишутся команды `hyprctl dispatch` (см. начало файла).
#[derive(Clone, Copy)]
enum Syntax {
    /// `hyprland.conf` и все версии до 0.55.
    Conf,
    /// `hyprland.lua`, 0.55 и новее.
    Lua,
}

/// Команды `hyprctl --batch`: сделать окно плавающим, поставить левым верхним
/// углом в `(x, y)` и закрепить.
fn place_commands(window: &Window, (x, y): (i32, i32), syntax: Syntax) -> Vec<String> {
    let w = format!("address:{}", window.address);
    let (float, moving, pin) = match syntax {
        Syntax::Conf => (
            format!("setfloating {w}"),
            format!("movewindowpixel exact {x} {y},{w}"),
            format!("pin {w}"),
        ),
        Syntax::Lua => (
            format!(r#"hl.dsp.window.float({{ window = "{w}", action = "enable" }})"#),
            format!(r#"hl.dsp.window.move({{ window = "{w}", x = {x}, y = {y} }})"#),
            format!(r#"hl.dsp.window.pin({{ window = "{w}", action = "enable" }})"#),
        ),
    };
    let mut commands = vec![float, moving];
    // `pin` с hyprland.conf переключает: окно, уже закреплённое правилом окна
    // (`pin = yes` из README), открепилось бы.
    if !window.pinned {
        commands.push(pin);
    }
    commands
        .into_iter()
        .map(|c| format!("dispatch {c}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;
    use crate::testutil::fake_command;

    /// Слева направо: монитор под 90° в отрицательных координатах, ноутбук с
    /// масштабом 1.25 и waybar сверху, 4K с масштабом 2 и панелью снизу. И
    /// запись без scale — разбор её пропускает.
    fn three_monitors() -> Value {
        json!([
            {"name": "DP-2", "x": -1080, "y": 0, "width": 1920, "height": 1080,
             "scale": 1.0, "transform": 1, "reserved": [0, 0, 0, 0], "focused": false},
            {"name": "eDP-1", "x": 0, "y": 0, "width": 2560, "height": 1600,
             "scale": 1.25, "transform": 0, "reserved": [0, 32, 0, 0], "focused": true},
            {"name": "DP-1", "x": 2048, "y": 0, "width": 3840, "height": 2160,
             "scale": 2.0, "transform": 0, "reserved": [0, 0, 0, 30], "focused": false},
            {"name": "без масштаба", "x": 0, "y": 0, "width": 800, "height": 600},
        ])
    }

    #[test]
    fn monitors_in_logical_pixels() {
        let monitor = |x, y, width, height, reserved, focused| Monitor {
            x,
            y,
            width,
            height,
            reserved,
            focused,
        };
        assert_eq!(
            parse_monitors(&three_monitors()),
            [
                monitor(-1080, 0, 1080, 1920, [0, 0, 0, 0], false),
                monitor(0, 0, 2048, 1280, [0, 32, 0, 0], true),
                monitor(2048, 0, 1920, 1080, [0, 0, 0, 30], false),
            ]
        );
        assert_eq!(parse_monitors(&json!({"error": "?"})), []);
    }

    #[test]
    fn work_area_under_cursor() {
        let monitors = parse_monitors(&three_monitors());
        let area = |x, y| monitor_under(&monitors, (x, y)).map(Monitor::work_area);
        assert_eq!(area(-1, 100), Some([-1080, 0, 1080, 1920]));
        // Левый край монитора — его, правый — уже соседа.
        assert_eq!(area(0, 0), Some([0, 32, 2048, 1248]));
        assert_eq!(area(2047, 1279), Some([0, 32, 2048, 1248]));
        assert_eq!(area(2048, 500), Some([2048, 0, 1920, 1050]));
        // Ни на одном мониторе — монитор в фокусе.
        assert_eq!(area(2500, 1500), Some([0, 32, 2048, 1248]));
        assert_eq!(monitor_under(&[], (0, 0)), None);
    }

    #[test]
    fn popup_window_by_class_and_pid() {
        let clients = json!([
            {"address": "0x1", "class": "kitty", "pid": 4242, "pinned": false},
            {"address": "0x2", "class": "shiftfix-popup", "pid": 1000, "pinned": false},
            {"address": "0x5a", "class": "shiftfix-popup", "pid": 4242, "pinned": true},
        ]);
        let popup = Window {
            address: "0x5a".into(),
            pinned: true,
        };
        assert_eq!(find_window(&clients, 4242), Some(popup));
        assert_eq!(find_window(&clients, 7), None);
        assert_eq!(find_window(&json!([]), 4242), None);
    }

    #[test]
    fn commands_for_both_configs() {
        let window = Window {
            address: "0x5a".into(),
            pinned: false,
        };
        assert_eq!(
            place_commands(&window, (-1280, 20), Syntax::Conf),
            [
                "dispatch setfloating address:0x5a",
                "dispatch movewindowpixel exact -1280 20,address:0x5a",
                "dispatch pin address:0x5a",
            ]
        );
        assert_eq!(
            place_commands(&window, (-1280, 20), Syntax::Lua),
            [
                r#"dispatch hl.dsp.window.float({ window = "address:0x5a", action = "enable" })"#,
                r#"dispatch hl.dsp.window.move({ window = "address:0x5a", x = -1280, y = 20 })"#,
                r#"dispatch hl.dsp.window.pin({ window = "address:0x5a", action = "enable" })"#,
            ]
        );
        // Уже закреплённое окно — без pin: с hyprland.conf он бы его открепил.
        let pinned = Window {
            pinned: true,
            ..window
        };
        assert_eq!(
            place_commands(&pinned, (0, 0), Syntax::Conf),
            [
                "dispatch setfloating address:0x5a",
                "dispatch movewindowpixel exact 0 0,address:0x5a",
            ]
        );
    }

    #[test]
    fn batch_succeeds_only_with_ok_for_each_command() {
        assert!(all_ok("ok\n\n\nok\n\n\nok", 3));
        assert!(all_ok("ok\n\n\nok\n", 2));
        assert!(!all_ok("ok\n\n\nok", 3));
        assert!(!all_ok("ok\n\n\nInvalid dispatcher", 2));
        assert!(!all_ok("error: lookup ok failed", 1));
        assert!(!all_ok("", 1));
    }

    #[test]
    fn cursor_and_area_over_hyprctl() {
        let (dir, hyprctl) = fake_command(
            "hyprctl",
            r#"printf '%s\n' "$*" >> "$(dirname "$0")/calls"
case "$2" in
cursorpos) echo '{"x": -1, "y": 700}' ;;
monitors) echo '[
  {"x": -1080, "y": 0, "width": 1920, "height": 1080, "scale": 1, "transform": 1,
   "reserved": [0, 0, 0, 40], "focused": false},
  {"x": 0, "y": 0, "width": 2560, "height": 1600, "scale": 1.25, "transform": 0,
   "reserved": [0, 32, 0, 0], "focused": true}]' ;;
esac"#,
        );
        assert_eq!(
            query_cursor_and_area(&hyprctl).unwrap(),
            ((-1, 700), [-1080, 0, 1080, 1880])
        );
        assert_eq!(
            fs::read_to_string(dir.join("calls")).unwrap(),
            "-j cursorpos\n-j monitors\n"
        );
    }

    #[test]
    fn popup_placed_once_its_window_appears() {
        let (dir, hyprctl) = fake_command(
            "hyprctl",
            r#"calls="$(dirname "$0")/calls"
printf '%s\n' "$*" >> "$calls"
case "$1" in
-j)
  # Окно попапа появляется к третьему вопросу.
  if [ "$(grep -c '^-j clients$' "$calls")" -lt 3 ]; then
    echo '[{"address": "0x1", "class": "kitty", "pid": 7, "pinned": false}]'
  else
    echo '[{"address": "0x1", "class": "kitty", "pid": 7, "pinned": false},
      {"address": "0x5a", "class": "shiftfix-popup", "pid": 4242, "pinned": false}]'
  fi ;;
--batch) printf 'ok\n\n\nok\n\n\nok\n' ;;
esac"#,
        );
        place_window(&hyprctl, 4242, (-1280, 20), Duration::from_secs(5)).unwrap();
        let calls = fs::read_to_string(dir.join("calls")).unwrap();
        assert_eq!(
            calls.lines().collect::<Vec<_>>(),
            [
                "-j clients",
                "-j clients",
                "-j clients",
                "--batch dispatch setfloating address:0x5a; \
                 dispatch movewindowpixel exact -1280 20,address:0x5a; \
                 dispatch pin address:0x5a",
            ]
        );
    }

    #[test]
    fn popup_placed_with_lua_config() {
        // С hyprland.lua команды hyprland.conf — ошибка Lua с подсказкой.
        let (dir, hyprctl) = fake_command(
            "hyprctl",
            r#"printf '%s\n' "$*" >> "$(dirname "$0")/calls"
case "$1 $2" in
"-j clients")
  echo '[{"address": "0x5a", "class": "shiftfix-popup", "pid": 4242, "pinned": true}]' ;;
"--batch dispatch hl.dsp."*) printf 'ok\n\n\nok\n' ;;
--batch*)
  note=' → Note: dispatch in lua is a shorthand for hl.dispatch(...)'
  printf 'error: syntax error\n\n%s\n\n\nerror: syntax error\n\n%s\n' "$note" "$note" ;;
esac"#,
        );
        place_window(&hyprctl, 4242, (100, 50), Duration::from_secs(5)).unwrap();
        let calls = fs::read_to_string(dir.join("calls")).unwrap();
        // Окно уже закреплено (правилом окна) — pin нет ни в одной пачке.
        let lua = concat!(
            "--batch ",
            r#"dispatch hl.dsp.window.float({ window = "address:0x5a", action = "enable" }); "#,
            r#"dispatch hl.dsp.window.move({ window = "address:0x5a", x = 100, y = 50 })"#,
        );
        assert_eq!(
            calls.lines().collect::<Vec<_>>(),
            [
                "-j clients",
                "--batch dispatch setfloating address:0x5a; \
                 dispatch movewindowpixel exact 100 50,address:0x5a",
                lua,
            ]
        );
    }

    #[test]
    fn popup_errors() {
        // Окно так и не появилось: ждём недолго, команд окну нет. Сколько раз
        // успели спросить, зависит от загрузки машины, — не проверяем.
        let (dir, hyprctl) = fake_command(
            "hyprctl",
            r#"printf '%s\n' "$*" >> "$(dirname "$0")/calls"; echo '[]'"#,
        );
        let err = place_window(&hyprctl, 4242, (0, 0), Duration::from_millis(120)).unwrap_err();
        assert!(err.to_string().contains("shiftfix-popup"), "{err}");
        let calls = fs::read_to_string(dir.join("calls")).unwrap();
        assert!(calls.lines().all(|call| call == "-j clients"), "{calls}");

        // Ни одна пачка команд не прошла — в ошибке ответы на обе.
        let (_dir, hyprctl) = fake_command(
            "hyprctl",
            r#"case "$1" in
-j) echo '[{"address": "0x5a", "class": "shiftfix-popup", "pid": 4242}]' ;;
--batch) echo 'Invalid dispatcher' ;;
esac"#,
        );
        let err = place_window(&hyprctl, 4242, (0, 0), Duration::from_secs(5)).unwrap_err();
        assert_eq!(
            err.to_string(),
            "hyprctl не поставил попап: Invalid dispatcher | Invalid dispatcher"
        );
    }
}

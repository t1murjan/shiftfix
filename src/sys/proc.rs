//! Запуск внешних команд с таймаутом: зависший wl-paste или hyprctl не должен
//! остановить обработку клавиатуры. Переменные сессии подставляются всегда.

use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::sys::env;
use crate::sys::paths::{in_path, non_empty, runtime_dir};
use crate::sys::x11::x_authority;

pub const TIMEOUT: Duration = Duration::from_secs(2);

/// Команда с переменными сессии: wl-paste, xclip, hyprctl, swaymsg и GUI найдут
/// композитор, даже если сама служба запустилась раньше сеанса.
pub fn session_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        cmd.env("XDG_RUNTIME_DIR", runtime_dir());
    }
    let wayland = env::wayland_display();
    if let Some(display) = &wayland {
        cmd.env("WAYLAND_DISPLAY", display);
    }
    if let Some(sig) = env::hyprland_signature() {
        cmd.env("HYPRLAND_INSTANCE_SIGNATURE", sig);
    }
    if let Some(sock) = env::sway_socket() {
        cmd.env("SWAYSOCK", sock);
    }
    if let Some(display) = env::x11_display() {
        cmd.env("DISPLAY", display);
        if let Some(auth) = x_authority() {
            cmd.env("XAUTHORITY", auth);
        }
    }
    for var in ["XDG_CURRENT_DESKTOP", "XDG_SESSION_TYPE"] {
        if non_empty(var).is_none()
            && let Some(value) = env::session_var(var)
        {
            cmd.env(var, value);
        }
    }
    cmd
}

pub struct Output {
    pub ok: bool,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Output {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// Команда запускается своей группой процессов: по таймауту убивается вся
/// группа, вместе с тем, что команда успела запустить.
fn kill_group(child: &mut Child) {
    // SAFETY: kill без побочных эффектов в нашем процессе; группа — потомка.
    unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
    let _ = child.wait();
}

fn wait(child: &mut Child, cmd: &str, timeout: Duration) -> Result<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child
            .try_wait()
            .with_context(|| format!("{cmd}: ожидание"))?
        {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            kill_group(child);
            bail!("{cmd}: нет ответа за {} мс", timeout.as_millis());
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn drain(mut pipe: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        buf
    })
}

/// Запустить и собрать вывод.
pub fn run(cmd: &str, args: &[&str], timeout: Duration) -> Result<Output> {
    let started = Instant::now();
    let mut child = session_command(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .with_context(|| format!("{cmd}: не запускается"))?;
    let stdout = drain(child.stdout.take().expect("stdout"));
    let stderr = drain(child.stderr.take().expect("stderr"));
    let status = wait(&mut child, cmd, timeout)?;
    // Команда завершилась, а запущенный ею фоновый процесс ещё держит вывод.
    while !(stdout.is_finished() && stderr.is_finished()) {
        if started.elapsed() >= timeout {
            kill_group(&mut child);
            bail!("{cmd}: вывод не закрыт за {} мс", timeout.as_millis());
        }
        thread::sleep(Duration::from_millis(2));
    }
    Ok(Output {
        ok: status.success(),
        stdout: stdout.join().unwrap_or_default(),
        stderr: String::from_utf8_lossy(&stderr.join().unwrap_or_default())
            .trim()
            .to_owned(),
    })
}

/// Запустить, а при ненулевом коде вернуть ошибку с текстом stderr.
pub fn run_text(cmd: &str, args: &[&str]) -> Result<String> {
    let out = run(cmd, args, TIMEOUT)?;
    if !out.ok {
        let reason = if out.stderr.is_empty() {
            "ошибка".to_owned()
        } else {
            out.stderr
        };
        bail!("{cmd} {}: {reason}", args.join(" "));
    }
    Ok(out.text())
}

/// Передать данные на stdin. Вывод не перехватывается: wl-copy уходит в фон
/// и держал бы наши каналы открытыми.
pub fn feed(cmd: &str, args: &[&str], input: &[u8]) -> Result<()> {
    let mut child = session_command(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .with_context(|| format!("{cmd}: не запускается"))?;
    let mut stdin = child.stdin.take().expect("stdin");
    let data = input.to_vec();
    let writer = thread::spawn(move || stdin.write_all(&data));
    let status = wait(&mut child, cmd, TIMEOUT)?;
    let _ = writer.join();
    if !status.success() {
        bail!(
            "{cmd} {}: код {}",
            args.join(" "),
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

/// Запустить в фоне и забыть (процесс дожидается отдельный поток, без зомби).
pub fn spawn_detached(cmd: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> Result<()> {
    let program = cmd.as_ref().to_string_lossy().into_owned();
    let mut child = session_command(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("{program}: не запускается"))?;
    thread::spawn(move || child.wait());
    Ok(())
}

/// Уведомление на рабочем столе через notify-send, если он установлен.
/// `options` — дополнительные параметры notify-send (срок показа, важность).
pub fn notify(text: &str, options: &[&str]) {
    if !in_path("notify-send") {
        return;
    }
    let mut args = vec!["-a", "shiftfix", "-i", "shiftfix"];
    args.extend_from_slice(options);
    args.extend(["shiftfix", text]);
    let _ = spawn_detached("notify-send", &args);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_output_and_errors() {
        assert_eq!(run_text("sh", &["-c", "printf 'ok'"]).unwrap(), "ok");
        let err = run_text("sh", &["-c", "echo boom >&2; exit 3"]).unwrap_err();
        assert!(err.to_string().contains("boom"), "{err}");
        assert!(run_text("/nonexistent/tool", &[]).is_err());
    }

    #[test]
    fn times_out() {
        let started = Instant::now();
        assert!(run("sleep", &["5"], Duration::from_millis(100)).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn background_child_does_not_hang() {
        // Фоновый процесс унаследовал вывод и держит его после выхода команды.
        let started = Instant::now();
        assert!(
            run(
                "sh",
                &["-c", "sleep 5 & echo hi"],
                Duration::from_millis(300)
            )
            .is_err()
        );
        assert!(run("sh", &["-c", "sleep 5"], Duration::from_millis(300)).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn feeds_stdin() {
        let dir = crate::testutil::TempDir::new("feed");
        let file = dir.join("out");
        let script = format!("cat > {}", file.display());
        feed("sh", &["-c", &script], "привет".as_bytes()).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "привет");
    }
}

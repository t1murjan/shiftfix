//! Запуск внешних команд с таймаутом: зависший wl-paste или hyprctl не должен
//! остановить обработку клавиатуры. Переменные сессии подставляются всегда.

use std::io::{Read, Write};
use std::process::{Child, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::sys::env::session_command;

pub const TIMEOUT: Duration = Duration::from_secs(2);

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
            let _ = child.kill();
            let _ = child.wait();
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
    let mut child = session_command(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("{cmd}: не запускается"))?;
    let stdout = drain(child.stdout.take().expect("stdout"));
    let stderr = drain(child.stderr.take().expect("stderr"));
    let status = wait(&mut child, cmd, timeout)?;
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
    fn feeds_stdin() {
        let dir = std::env::temp_dir().join(format!("shiftfix-feed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("out");
        let script = format!("cat > {}", file.display());
        feed("sh", &["-c", &script], "привет".as_bytes()).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "привет");
        std::fs::remove_dir_all(dir).unwrap();
    }
}

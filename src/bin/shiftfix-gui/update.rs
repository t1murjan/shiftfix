//! Обновление из папки с исходниками, откуда ставил install.sh: проверка новой
//! версии (`git fetch` и версия в `Cargo.toml` ветки upstream) и запуск
//! `git pull --ff-only && ./install.sh` в окне терминала.

use std::env;
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use eframe::egui;
use shiftfix::sys::{paths, proc};

use crate::app::VERSION;
use crate::format::shown_path;

/// Сколько ждать `git fetch`: без сети он может висеть долго.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
/// Сколько ждать остальные команды git: они не ходят в сеть.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(5);

/// Подсказка у кнопки «Обновить».
pub const BUTTON_HINT: &str = "Откроет терминал: git pull и ./install.sh в папке с исходниками. \
     Окно закроется и откроется снова.";

/// Итог проверки новой версии.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// В upstream версия новее окна.
    Newer(String),
    /// Новее нет.
    Latest,
    /// Не удалось проверить; причина — для показа.
    Failed(String),
}

#[derive(Clone, Default)]
pub struct State {
    /// Проверка идёт.
    pub running: bool,
    /// Папка с исходниками; `None` — неизвестна: нет файла от install.sh или
    /// в папке больше нет install.sh.
    pub source: Option<PathBuf>,
    /// Итог последней проверки; `None` — ещё не проверяли.
    pub outcome: Option<Outcome>,
}

/// Проверка новой версии в фоне: при запуске окна и по кнопке «Проверить».
#[derive(Clone, Default)]
pub struct Update(Arc<Mutex<State>>);

impl Update {
    /// Перечитать папку с исходниками и проверить в фоне, нет ли новой версии.
    pub fn check(&self, ctx: &egui::Context) {
        let source = read_source(&source_file());
        self.set(State {
            running: source.is_some(),
            source: source.clone(),
            outcome: None,
        });
        let Some(dir) = source else { return };
        let (update, ctx) = (self.clone(), ctx.clone());
        thread::spawn(move || {
            let outcome = check_version(&dir, VERSION);
            update.set(State {
                running: false,
                source: Some(dir),
                outcome: Some(outcome),
            });
            ctx.request_repaint();
        });
    }

    pub fn state(&self) -> State {
        self.0.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn set(&self, state: State) {
        if let Ok(mut s) = self.0.lock() {
            *s = state;
        }
    }
}

/// Файл, в который install.sh пишет путь к папке, откуда его запускали
/// (`SOURCE_FILE` в scripts/common.sh).
fn source_file() -> PathBuf {
    env::var_os("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| paths::home().map(|home| home.join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("shiftfix-source")
}

/// Папка с исходниками из файла `file`, если в ней по-прежнему лежит install.sh.
fn read_source(file: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(file).ok()?;
    let dir = PathBuf::from(text.strip_suffix('\n').unwrap_or(&text));
    (dir.is_absolute() && dir.join("install.sh").is_file()).then_some(dir)
}

/// Есть ли в upstream версия новее `current`.
fn check_version(dir: &Path, current: &str) -> Outcome {
    match upstream_version(dir) {
        Ok(version) if newer(&version, current) => Outcome::Newer(version),
        Ok(_) => Outcome::Latest,
        Err(reason) => Outcome::Failed(reason),
    }
}

/// Версия в Cargo.toml ветки upstream после `git fetch`; ошибка — причина для показа.
fn upstream_version(dir: &Path) -> Result<String, String> {
    if !dir.join(".git").exists() {
        return Err("папка с исходниками — не git-клон".into());
    }
    if !paths::in_path("git") {
        return Err("не установлен git".into());
    }
    git(dir, &["rev-parse", "@{u}"], LOCAL_TIMEOUT)
        .map_err(|_| "у текущей ветки в папке с исходниками нет upstream".to_owned())?;
    git(dir, &["fetch", "--quiet"], FETCH_TIMEOUT)
        .map_err(|err| format!("нет связи с репозиторием ({err})"))?;
    let cargo_toml = git(dir, &["show", "@{u}:Cargo.toml"], LOCAL_TIMEOUT)
        .map_err(|err| format!("Cargo.toml из upstream не читается ({err})"))?;
    package_version(&cargo_toml).ok_or_else(|| "в Cargo.toml из upstream нет версии".to_owned())
}

/// `git -C dir args`: вывод или первая строка ошибки. Пароль git не спрашивает:
/// без этого `fetch` из закрытого репозитория открывал бы окно ввода пароля
/// (ksshaskpass в KDE) при каждом запуске окна — пусть лучше честно не удастся.
fn git(dir: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let dir = dir.to_string_lossy();
    let mut full = vec![
        "GIT_TERMINAL_PROMPT=0",
        "GIT_ASKPASS=true",
        "SSH_ASKPASS=true",
        "GIT_SSH_COMMAND=ssh -o BatchMode=yes",
        "git",
        "-C",
        &dir,
    ];
    full.extend_from_slice(args);
    let out = proc::run("env", &full, timeout).map_err(|err| format!("{err:#}"))?;
    if !out.ok {
        let first = out.stderr.lines().next().unwrap_or("ошибка");
        return Err(first.to_owned());
    }
    Ok(out.text())
}

/// `version` из `[package]` в тексте Cargo.toml.
fn package_version(cargo_toml: &str) -> Option<String> {
    let doc: toml::Table = cargo_toml.parse().ok()?;
    let version = doc.get("package")?.get("version")?.as_str()?;
    Some(version.to_owned())
}

/// Версия `a` новее `b`. Числа через точку сравниваются по порядку (0.10.0
/// новее 0.9.1, 0.5 — то же, что 0.5.0); при равных числах выпуск новее
/// предварительной версии (0.5.0 новее 0.5.0-dev).
fn newer(a: &str, b: &str) -> bool {
    fn parse(version: &str) -> (Vec<u64>, bool) {
        let version = version.trim().trim_start_matches('v');
        let version = version.split('+').next().unwrap_or_default(); // без сведений о сборке
        let (numbers, release) = match version.split_once('-') {
            Some((numbers, _)) => (numbers, false),
            None => (version, true),
        };
        let numbers = numbers.split('.').map(|n| n.parse().unwrap_or(0)).collect();
        (numbers, release)
    }
    let ((mut a, a_release), (mut b, b_release)) = (parse(a), parse(b));
    let len = a.len().max(b.len());
    a.resize(len, 0);
    b.resize(len, 0);
    (a, a_release) > (b, b_release)
}

/// Терминалы, которые ищет «Обновить», по порядку (до них — `$TERMINAL`), и
/// аргументы, после которых идут программа и её аргументы.
const TERMINALS: [(&str, &[&str]); 11] = [
    // Запускает терминал, выбранный в системе, и сам подставляет его ключ.
    ("xdg-terminal-exec", &[]),
    // -e забирает все аргументы после себя.
    ("konsole", &["-e"]),
    // -e и -x устарели: команда — после --.
    ("gnome-terminal", &["--"]),
    // GNOME Console.
    ("kgx", &["--"]),
    ("kitty", &[]),
    ("alacritty", &["-e"]),
    ("ghostty", &["-e"]),
    ("foot", &[]),
    ("wezterm", &["start", "--"]),
    // -x — остаток аргументов, а -e — одна строка, которую терминал сам разбирает.
    ("xfce4-terminal", &["-x"]),
    ("xterm", &["-e"]),
];

/// Аргументы терминала перед командой. Незнакомый терминал из `$TERMINAL` —
/// `-e`: так команду принимает большинство терминалов.
fn terminal_args(terminal: &str) -> &'static [&'static str] {
    let name = Path::new(terminal)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(terminal);
    TERMINALS
        .iter()
        .find(|(known, _)| *known == name)
        .map_or(&["-e"], |(_, args)| args)
}

/// Терминал: из `$TERMINAL` (`env_terminal`), иначе первый из `TERMINALS`,
/// который есть (`exists`).
fn find_terminal(env_terminal: Option<String>, exists: impl Fn(&str) -> bool) -> Option<String> {
    env_terminal
        .filter(|t| !t.is_empty() && exists(t))
        .or_else(|| {
            TERMINALS
                .iter()
                .map(|(name, _)| name.to_string())
                .find(|name| exists(name))
        })
}

/// Обновление в папке с исходниками: без git-клона — только install.sh.
fn update_command(git_clone: bool) -> &'static str {
    if git_clone {
        "git pull --ff-only && ./install.sh"
    } else {
        "./install.sh"
    }
}

/// Полная команда запуска: терминал, его ключи и `sh -c` со сценарием.
/// Папка передаётся отдельным аргументом (`$1`): в сценарий её не надо вставлять
/// с кавычками. В конце терминал ждёт Enter, чтобы итог можно было прочитать.
fn terminal_command(terminal: &str, source: &Path, git_clone: bool) -> Vec<String> {
    let script = format!(
        "cd -- \"$1\" && {} || printf '\\nНе получилось — что не так, написано выше.\\n'\n\
         printf '\\nНажмите Enter, чтобы закрыть. '\n\
         read -r _",
        update_command(git_clone)
    );
    let mut command = vec![terminal.to_owned()];
    command.extend(terminal_args(terminal).iter().map(|a| a.to_string()));
    command.extend(["sh".into(), "-c".into(), script, "shiftfix-update".into()]);
    command.push(source.to_string_lossy().into_owned());
    command
}

/// Папка для команды, которую человек наберёт сам: домашний каталог — «~», а путь
/// с пробелами и другими особыми для shell символами — целиком в кавычках.
fn typed_path(dir: &Path) -> String {
    let shown = shown_path(dir);
    if shown
        .chars()
        .all(|c| c.is_alphanumeric() || "/._-+~".contains(c))
    {
        return shown;
    }
    format!("'{}'", dir.to_string_lossy().replace('\'', r"'\''"))
}

/// Открыть терминал с обновлением из папки `source`. Итог — имя терминала (без
/// каталога), ошибка — текст для показа с командой, которую выполнить самому.
pub fn open_terminal(source: &Path) -> Result<String, String> {
    let git_clone = source.join(".git").exists();
    let by_hand = format!("cd {} && {}", typed_path(source), update_command(git_clone));
    let exists = |t: &str| {
        if t.contains('/') {
            Path::new(t).is_file()
        } else {
            paths::in_path(t)
        }
    };
    let Some(terminal) = find_terminal(env::var("TERMINAL").ok(), exists) else {
        return Err(format!("Не найден терминал. Выполните сами: {by_hand}"));
    };
    let command = terminal_command(&terminal, source, git_clone);
    let spawned = Command::new(&command[0])
        .args(&command[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Своя группа процессов: терминал не зависит от окна, которое install.sh
        // закроет, и от терминала, из которого окно запустили.
        .process_group(0)
        .spawn();
    match spawned {
        Ok(mut child) => {
            thread::spawn(move || child.wait()); // без зомби
            let name = Path::new(&terminal).file_name().unwrap_or_default();
            Ok(name.to_string_lossy().into_owned())
        }
        Err(err) => Err(format!(
            "{terminal} не запускается: {err}. Выполните сами: {by_hand}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// Свой временный каталог на тест; удаляется в Drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> TempDir {
            static N: AtomicUsize = AtomicUsize::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let dir = env::temp_dir().join(format!("shiftfix-update-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Исполняемый скрипт `dir/name`. Копия `testutil::write_script` библиотеки
    /// (её тестовый код окну недоступен): файл пишет отдельный `sh`, иначе
    /// соседний тест, запуская процесс, унаследовал бы открытый на запись файл,
    /// и запуск скрипта упал бы с ETXTBSY.
    fn write_script(dir: &Path, name: &str, body: &str) {
        let written = Command::new("sh")
            .args(["-c", r#"printf '%s' "$1" > "$0" && chmod 755 "$0""#])
            .arg(dir.join(name))
            .arg(format!("#!/bin/sh\n{body}"))
            .status()
            .unwrap();
        assert!(written.success(), "не записать {name}");
    }

    /// git в каталоге теста; без своих и системных настроек git пользователя.
    fn git_ok(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// Коммит Cargo.toml с версией `version` в репозиторий `dir`.
    fn commit_version(dir: &Path, version: &str) {
        let text = format!("[package]\nname = \"shiftfix\"\nversion = \"{version}\"\n");
        fs::write(dir.join("Cargo.toml"), text).unwrap();
        git_ok(dir, &["add", "Cargo.toml"]);
        git_ok(dir, &["commit", "-q", "-m", version]);
    }

    #[test]
    fn versions_compare_by_numbers() {
        assert!(newer("0.5.0", "0.4.0"));
        assert!(newer("0.10.0", "0.9.1"));
        assert!(newer("1.0.0", "0.99.99"));
        assert!(newer("0.4.1", "0.4.0"));
        assert!(!newer("0.4.0", "0.4.0"));
        assert!(!newer("0.4.0", "0.5.0"));
        assert!(!newer("0.5", "0.5.0"));
        assert!(!newer("v0.5.0", "0.5.0"));
        // Предварительная версия старше прежнего выпуска, но младше своего.
        assert!(newer("0.5.0", "0.5.0-dev"));
        assert!(!newer("0.5.0-dev", "0.5.0"));
        assert!(newer("0.5.0-dev", "0.4.0"));
        assert!(!newer("0.5.0+build.2", "0.5.0"));
    }

    #[test]
    fn version_from_cargo_toml() {
        let text = r#"[package]
name = "shiftfix"
version = "0.5.0"
edition = "2024"

[dependencies]
anyhow = { version = "1.0.104" }
"#;
        assert_eq!(package_version(text).as_deref(), Some("0.5.0"));
        // Версия зависимости — не версия пакета.
        assert_eq!(package_version("[dependencies]\nversion = \"1.0\"\n"), None);
        assert_eq!(
            package_version("[package]\nversion.workspace = true\n"),
            None
        );
        assert_eq!(package_version("не toml"), None);
    }

    #[test]
    fn source_dir_needs_install_sh() {
        let tmp = TempDir::new();
        let file = tmp.0.join("shiftfix-source");
        let src = tmp.0.join("my shiftfix");
        fs::create_dir_all(&src).unwrap();
        assert_eq!(read_source(&file), None); // файла нет

        fs::write(&file, format!("{}\n", src.display())).unwrap();
        assert_eq!(read_source(&file), None); // в папке нет install.sh

        fs::write(src.join("install.sh"), "").unwrap();
        assert_eq!(read_source(&file), Some(src.clone()));
        fs::write(&file, src.display().to_string()).unwrap(); // без перевода строки
        assert_eq!(read_source(&file), Some(src));

        fs::write(&file, "relative/dir\n").unwrap();
        assert_eq!(read_source(&file), None);
    }

    #[test]
    fn terminal_command_for_each_terminal() {
        let src = Path::new("/home/u/my shiftfix");
        let expected: [(&str, &[&str]); 12] = [
            ("xdg-terminal-exec", &[]),
            ("konsole", &["-e"]),
            ("gnome-terminal", &["--"]),
            ("kgx", &["--"]),
            ("kitty", &[]),
            ("alacritty", &["-e"]),
            ("ghostty", &["-e"]),
            ("foot", &[]),
            ("wezterm", &["start", "--"]),
            ("xfce4-terminal", &["-x"]),
            ("xterm", &["-e"]),
            // Незнакомый терминал из $TERMINAL.
            ("/opt/st/bin/st", &["-e"]),
        ];
        for (terminal, args) in expected {
            let command = terminal_command(terminal, src, true);
            assert_eq!(command[0], terminal);
            assert_eq!(&command[1..=args.len()], args, "{terminal}");
            let rest = &command[args.len() + 1..];
            assert_eq!(rest[..2], ["sh", "-c"], "{terminal}");
            assert_eq!(
                rest[3..],
                ["shiftfix-update", "/home/u/my shiftfix"],
                "{terminal}"
            );
        }
        // Знакомый терминал по полному пути из $TERMINAL — со своими ключами.
        assert_eq!(terminal_args("/usr/bin/wezterm"), ["start", "--"]);
    }

    #[test]
    fn terminal_script_updates_in_source_dir() {
        let tmp = TempDir::new();
        let src = tmp.0.join("src dir");
        fs::create_dir_all(&src).unwrap();
        // Вместо install.sh — запись папки, в которой его запустили.
        write_script(&src, "install.sh", "pwd > ran\n");

        // Без git-клона — только install.sh; Enter закрывает.
        let command = terminal_command("xterm", &src, false);
        assert!(command[4].contains("&& ./install.sh") && !command[4].contains("git"));
        let mut sh = Command::new(&command[2])
            .args(&command[3..])
            .current_dir(&tmp.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        sh.stdin.take().unwrap().write_all(b"\n").unwrap();
        let out = sh.wait_with_output().unwrap();
        assert!(out.status.success());
        let ran = fs::read_to_string(src.join("ran")).unwrap();
        assert_eq!(ran.trim_end(), src.display().to_string());
        let shown = String::from_utf8_lossy(&out.stdout);
        assert!(shown.contains("Нажмите Enter"), "{shown}");
        assert!(!shown.contains("Не получилось"), "{shown}");

        let command = terminal_command("xterm", &src, true);
        assert!(command[4].contains("cd -- \"$1\" && git pull --ff-only && ./install.sh"));
    }

    #[test]
    fn typed_path_is_quoted_when_needed() {
        assert_eq!(
            typed_path(Path::new("/opt/shiftfix-0.4")),
            "/opt/shiftfix-0.4"
        );
        assert_eq!(typed_path(Path::new("/opt/my src")), "'/opt/my src'");
        assert_eq!(typed_path(Path::new("/opt/it's")), r"'/opt/it'\''s'");
    }

    #[test]
    fn terminal_from_env_then_list() {
        let have = |names: &'static [&'static str]| move |t: &str| names.contains(&t);
        let pick = |env: Option<&str>, names| find_terminal(env.map(String::from), have(names));
        assert_eq!(
            pick(Some("foot"), &["foot", "konsole"]).as_deref(),
            Some("foot")
        );
        // $TERMINAL не найден или пуст — по списку.
        assert_eq!(
            pick(Some("nope"), &["kitty", "xterm"]).as_deref(),
            Some("kitty")
        );
        assert_eq!(
            pick(Some(""), &["xterm", "konsole"]).as_deref(),
            Some("konsole")
        );
        assert_eq!(pick(None, &["xterm"]).as_deref(), Some("xterm"));
        assert_eq!(pick(None, &[]), None);
    }

    #[test]
    fn check_with_real_git() {
        let tmp = TempDir::new();
        let upstream = tmp.0.join("upstream");
        let clone = tmp.0.join("clone");
        fs::create_dir_all(&upstream).unwrap();
        git_ok(&upstream, &["init", "-q"]);
        commit_version(&upstream, "0.5.0");
        git_ok(&tmp.0, &["clone", "-q", "upstream", "clone"]);

        assert_eq!(
            check_version(&clone, "0.4.0"),
            Outcome::Newer("0.5.0".into())
        );
        assert_eq!(check_version(&clone, "0.5.0"), Outcome::Latest);

        // Новая версия в upstream видна после fetch, без pull.
        commit_version(&upstream, "0.6.0");
        assert_eq!(
            check_version(&clone, "0.5.0"),
            Outcome::Newer("0.6.0".into())
        );

        // Репозиторий недоступен.
        fs::rename(&upstream, tmp.0.join("gone")).unwrap();
        let Outcome::Failed(reason) = check_version(&clone, "0.5.0") else {
            panic!("ожидалась ошибка");
        };
        assert!(reason.starts_with("нет связи с репозиторием ("), "{reason}");

        // Ветка без upstream.
        git_ok(&clone, &["checkout", "-q", "-b", "local"]);
        assert_eq!(
            check_version(&clone, "0.5.0"),
            Outcome::Failed("у текущей ветки в папке с исходниками нет upstream".into())
        );

        // Не git-клон.
        assert_eq!(
            check_version(&tmp.0, "0.5.0"),
            Outcome::Failed("папка с исходниками — не git-клон".into())
        );
    }
}

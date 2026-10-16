//! X11: сеансы X11 (Mint/Cinnamon, XFCE, MATE…) и XWayland — через него
//! выделение отдаёт GNOME, у которого нет протокола data-control.
//!
//! Подключение своё (x11rb, без libX11): служба часто стартует раньше сеанса,
//! и XAUTHORITY приходится искать самим.

use std::fs;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use anyhow::{Context, Result, anyhow, bail};
use log::{debug, info};
use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
use x11rb::protocol::xkb::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask,
    WindowClass,
};
use x11rb::rust_connection::{DefaultStream, RustConnection};

use crate::daemon::Msg;
use crate::engine::{Backend, Keys};
use crate::env;
use crate::keys::norm_layout;
use crate::xkb::{Keymap, keymap_from_names};

pub struct XConn {
    pub conn: RustConnection,
    pub root: u32,
}

// ─── Подключение ────────────────────────────────────────────────────────────

/// Куки MIT-MAGIC-COOKIE-1 для дисплея из файла Xauthority.
fn parse_xauth(data: &[u8], number: u32) -> Vec<Vec<u8>> {
    fn take<'a>(data: &'a [u8], pos: &mut usize, n: usize) -> Option<&'a [u8]> {
        let slice = data.get(*pos..pos.checked_add(n)?)?;
        *pos += n;
        Some(slice)
    }
    fn field<'a>(data: &'a [u8], pos: &mut usize) -> Option<&'a [u8]> {
        let len = take(data, pos, 2)?;
        let len = usize::from(u16::from_be_bytes([len[0], len[1]]));
        take(data, pos, len)
    }
    let wanted = number.to_string();
    let mut pos = 0;
    let mut cookies = Vec::new();
    while pos < data.len() {
        // семейство адреса, адрес, номер дисплея, метод, данные
        let Some(_family) = take(data, &mut pos, 2) else {
            break;
        };
        let (Some(_address), Some(display), Some(name), Some(cookie)) = (
            field(data, &mut pos),
            field(data, &mut pos),
            field(data, &mut pos),
            field(data, &mut pos),
        ) else {
            break;
        };
        let matches = display.is_empty() || display == wanted.as_bytes();
        if matches && name == b"MIT-MAGIC-COOKIE-1" && !cookies.iter().any(|c| c == cookie) {
            cookies.push(cookie.to_vec());
        }
    }
    cookies
}

/// Где может лежать Xauthority: KDE и SDDM, GNOME (XWayland и GDM), домашний.
fn auth_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = env::session_var("XAUTHORITY")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    if let Some(home) = std::env::var_os("HOME") {
        files.push(PathBuf::from(home).join(".Xauthority"));
    }
    let run = env::runtime_dir();
    let mut found: Vec<PathBuf> = fs::read_dir(&run)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with("xauth_") || name.starts_with(".mutter-Xwaylandauth.")
        })
        .map(|e| e.path())
        .collect();
    found.sort();
    files.extend(found);
    files.push(run.join("gdm").join("Xauthority"));
    let mut unique = Vec::new();
    for file in files {
        if file.is_file() && !unique.contains(&file) {
            unique.push(file);
        }
    }
    unique
}

fn stream(number: u32) -> std::io::Result<UnixStream> {
    let path = format!("/tmp/.X11-unix/X{number}");
    // Как libxcb: сначала абстрактный сокет, потом файл.
    SocketAddr::from_abstract_name(path.as_bytes())
        .and_then(|addr| UnixStream::connect_addr(&addr))
        .or_else(|_| UnixStream::connect(&path))
}

fn open(number: u32, cookie: Option<Vec<u8>>) -> Result<RustConnection> {
    let (stream, _) = DefaultStream::from_unix_stream(stream(number)?)?;
    let (name, data) = match cookie {
        Some(cookie) => (b"MIT-MAGIC-COOKIE-1".to_vec(), cookie),
        None => (Vec::new(), Vec::new()),
    };
    Ok(RustConnection::connect_to_stream_with_auth_info(
        stream, 0, name, data,
    )?)
}

pub fn connect() -> Result<XConn> {
    let display = env::x11_display().context("X-сервер не найден")?;
    let number = env::x11_number(&display).context("номер дисплея")?;
    let mut attempts: Vec<(Option<PathBuf>, Option<Vec<u8>>)> = Vec::new();
    for file in auth_files() {
        for cookie in parse_xauth(&fs::read(&file).unwrap_or_default(), number) {
            if !attempts.iter().any(|(_, c)| c.as_ref() == Some(&cookie)) {
                attempts.push((Some(file.clone()), Some(cookie)));
            }
        }
    }
    attempts.push((None, None)); // XWayland без авторизации (wlroots и др.)
    let mut last = anyhow!("нет подходящего Xauthority");
    for (file, cookie) in attempts.into_iter().take(12) {
        match open(number, cookie) {
            Ok(conn) => {
                if file.is_some() {
                    env::set_x_authority(file);
                }
                let root = conn
                    .setup()
                    .roots
                    .first()
                    .context("у X-сервера нет экранов")?
                    .root;
                return Ok(XConn { conn, root });
            }
            Err(err) => last = err,
        }
    }
    Err(last.context(format!("X-сервер {display}")))
}

/// `_XKB_RULES_NAMES` корневого окна: rules, model, layout, variant, options.
pub fn rules_names(x: &XConn) -> Result<Vec<String>> {
    let atom = x
        .conn
        .intern_atom(false, b"_XKB_RULES_NAMES")?
        .reply()?
        .atom;
    let reply = x
        .conn
        .get_property(false, x.root, atom, AtomEnum::STRING, 0, 4096)?
        .reply()?;
    if reply.value.is_empty() {
        bail!("X-сервер не сообщает раскладки (_XKB_RULES_NAMES)");
    }
    Ok(reply
        .value
        .split(|b| *b == 0)
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect())
}

// ─── Раскладка через XKB ────────────────────────────────────────────────────

/// Переключение группы XKB — работает в любом окружении X11.
#[derive(Default)]
pub struct XkbLayout {
    x: Option<XConn>,
}

impl XkbLayout {
    fn with<R>(&mut self, f: impl FnOnce(&XConn) -> Result<R>) -> Result<R> {
        if self.x.is_none() {
            let x = connect()?;
            let reply = x.conn.xkb_use_extension(1, 0)?.reply()?;
            if !reply.supported {
                bail!("у X-сервера нет расширения XKB");
            }
            self.x = Some(x);
        }
        let result = f(self.x.as_ref().expect("connection"));
        if result.is_err() {
            self.x = None; // X-сервер мог перезапуститься
        }
        result
    }
}

impl Backend for XkbLayout {
    fn name(&self) -> &'static str {
        "X11"
    }

    fn layouts(&mut self) -> Result<Vec<String>> {
        self.with(|x| {
            let names = rules_names(x)?;
            let layouts = names.get(2).map(String::as_str).unwrap_or_default();
            if layouts.is_empty() {
                bail!("раскладки X11 неизвестны");
            }
            Ok(layouts.split(',').map(norm_layout).collect())
        })
    }

    fn current(&mut self) -> Result<Option<usize>> {
        self.with(|x| {
            let state = x
                .conn
                .xkb_get_state(xkb::ID::USE_CORE_KBD.into())?
                .reply()?;
            Ok(Some(usize::from(u8::from(state.locked_group))))
        })
    }

    fn set(&mut self, index: usize, _keys: &mut dyn Keys) -> Result<()> {
        let group = u8::try_from(index)
            .ok()
            .filter(|g| *g < 4)
            .context("в X11 не больше четырёх раскладок")?;
        self.with(|x| {
            x.conn
                .xkb_latch_lock_state(
                    xkb::ID::USE_CORE_KBD.into(),
                    0u8.into(),
                    0u8.into(),
                    true,
                    xkb::Group::from(group),
                    0u8.into(),
                    false,
                    0,
                )?
                .check()?;
            Ok(())
        })
    }

    fn toggle(&mut self, keys: &mut dyn Keys) -> Result<()> {
        let count = self.layouts()?.len().max(1);
        let current = self.current()?.unwrap_or(0);
        self.set((current + 1) % count, keys)
    }
}

// ─── Keymap ─────────────────────────────────────────────────────────────────

/// Раскладки сеанса X11 из `_XKB_RULES_NAMES` (`None` — нет libxkbcommon).
pub fn keymap(x: &XConn) -> Option<Keymap> {
    let names = rules_names(x).ok()?;
    let part = |i: usize| names.get(i).map(String::as_str).unwrap_or_default();
    let keymap = keymap_from_names(part(2), part(3), part(4));
    if keymap.is_none() {
        debug!("keymap X11 не разобрана (нет libxkbcommon?)");
    }
    keymap
}

/// Присылать keymap сеанса X11 сейчас и при каждой смене раскладок, пока жив
/// X-сервер. В Wayland keymap присылает композитор (`xkb::spawn_watcher`).
pub fn watch_keymap(on_change: &impl Fn(Keymap)) -> Result<()> {
    let x = connect()?;
    let conn = &x.conn;
    let rules = conn.intern_atom(false, b"_XKB_RULES_NAMES")?.reply()?.atom;
    conn.change_window_attributes(
        x.root,
        &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )?;
    conn.flush()?;
    let send = || {
        if let Some(keymap) = keymap(&x) {
            let list: Vec<&str> = keymap.layouts.iter().map(|l| l.name.as_str()).collect();
            info!("раскладки из X11: {}", list.join(", "));
            on_change(keymap);
        }
    };
    send();
    loop {
        if let Event::PropertyNotify(ev) = conn.wait_for_event()?
            && ev.atom == rules
        {
            send();
        }
    }
}

// ─── Слежение за PRIMARY ────────────────────────────────────────────────────

/// Следить за PRIMARY, пока жив X-сервер. `Ok(())` — служба завершается.
pub fn watch(tx: &Sender<Msg>) -> Result<()> {
    let x = connect()?;
    let conn = &x.conn;
    conn.xfixes_query_version(5, 0)?.reply()?;
    let window = conn.generate_id()?;
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        x.root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new().override_redirect(1),
    )?;
    let primary: u32 = AtomEnum::PRIMARY.into();
    conn.xfixes_select_selection_input(
        window,
        primary,
        SelectionEventMask::SET_SELECTION_OWNER
            | SelectionEventMask::SELECTION_WINDOW_DESTROY
            | SelectionEventMask::SELECTION_CLIENT_CLOSE,
    )?;
    conn.flush()?;
    if tx.send(Msg::Selection(Ok("X11"))).is_err() {
        return Ok(());
    }
    loop {
        if let Event::XfixesSelectionNotify(ev) = conn.wait_for_event()?
            && ev.selection == primary
            && tx
                .send(Msg::Primary(usize::from(ev.owner != x11rb::NONE)))
                .is_err()
        {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(display: &str, name: &[u8], cookie: &[u8]) -> Vec<u8> {
        let mut out = vec![1, 0]; // FamilyLocal
        for field in [b"host".as_slice(), display.as_bytes(), name, cookie] {
            out.extend(u16::try_from(field.len()).unwrap().to_be_bytes());
            out.extend(field);
        }
        out
    }

    #[test]
    fn parses_xauthority() {
        let mut file = record("0", b"MIT-MAGIC-COOKIE-1", &[1, 2, 3]);
        file.extend(record("1", b"MIT-MAGIC-COOKIE-1", &[4, 5, 6]));
        file.extend(record("", b"MIT-MAGIC-COOKIE-1", &[7]));
        file.extend(record("1", b"XDM-AUTHORIZATION-1", &[8]));
        assert_eq!(parse_xauth(&file, 1), [vec![4, 5, 6], vec![7]]);
        assert_eq!(parse_xauth(&file, 0), [vec![1, 2, 3], vec![7]]);
        // Обрезанный файл не роняет разбор.
        assert_eq!(parse_xauth(&file[..5], 0), Vec::<Vec<u8>>::new());
    }

    /// Раскладки XKB и PRIMARY на настоящем X-сервере (Xvfb). Пропускается, если
    /// Xvfb, setxkbmap или xclip нет, а также при живом X-сеансе: тест не должен
    /// переключать раскладку на машине разработчика.
    #[test]
    fn xkb_and_primary_on_xvfb() {
        use std::process::{Command, Stdio};
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        const DISPLAY: &str = ":97";
        if !["Xvfb", "setxkbmap", "xclip"]
            .iter()
            .all(|t| env::in_path(t))
        {
            eprintln!("нет Xvfb, setxkbmap или xclip — пропуск");
            return;
        }
        if env::x11_display().is_some() {
            eprintln!("есть живой X-сервер — пропуск");
            return;
        }
        let Ok(mut server) = Command::new("Xvfb")
            .args([DISPLAY, "-nolisten", "tcp", "-noreset"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            eprintln!("Xvfb не запустился — пропуск");
            return;
        };
        struct Kill(std::process::Child);
        impl Drop for Kill {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let started = Instant::now();
        while env::x11_display().as_deref() != Some(DISPLAY) {
            if started.elapsed() > Duration::from_secs(5) {
                let _ = server.kill();
                eprintln!("Xvfb не поднялся — пропуск");
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _server = Kill(server);
        let x = |program: &str, args: &[&str]| {
            Command::new(program)
                .args(args)
                .env("DISPLAY", DISPLAY)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .status()
                .unwrap()
        };
        assert!(x("setxkbmap", &["-layout", "us,ru"]).success());

        struct NoKeys;
        impl Keys for NoKeys {
            fn backspace(&mut self, _: usize) -> Result<()> {
                Ok(())
            }
            fn type_strokes(&mut self, _: &[crate::engine::Stroke]) -> Result<()> {
                Ok(())
            }
            fn combo(&mut self, _: &[evdev::KeyCode]) -> Result<()> {
                Ok(())
            }
        }
        let mut xkb = XkbLayout::default();
        let mut keys = NoKeys;
        assert_eq!(xkb.layouts().unwrap(), ["us", "ru"]);
        assert_eq!(xkb.current().unwrap(), Some(0));
        xkb.set(1, &mut keys).unwrap();
        assert_eq!(xkb.current().unwrap(), Some(1));
        xkb.toggle(&mut keys).unwrap();
        assert_eq!(xkb.current().unwrap(), Some(0));
        assert!(xkb.set(4, &mut keys).is_err());

        // Keymap приходит и без слежения за выделением, и после смены раскладок.
        if crate::xkb::keymap_from_names("us", "", "").is_some() {
            let (ktx, krx) = mpsc::channel();
            std::thread::spawn(move || {
                watch_keymap(&|keymap| {
                    let _ = ktx.send(keymap.layouts.len());
                })
            });
            let layouts = || {
                krx.recv_timeout(Duration::from_secs(5))
                    .expect("keymap от X11")
            };
            assert_eq!(layouts(), 2);
            assert!(x("setxkbmap", &["-layout", "us,ru,de"]).success());
            assert_eq!(layouts(), 3);
        } else {
            eprintln!("нет libxkbcommon — keymap X11 не проверяется");
        }

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || watch(&tx));
        let next = || {
            rx.recv_timeout(Duration::from_secs(5))
                .expect("событие от X11")
        };
        assert!(
            matches!(next(), Msg::Selection(Ok("X11"))),
            "ждали Selection(Ok(\"X11\")), keymap выделение больше не присылает"
        );
        // Новое выделение — событие Primary.
        let mut xclip = Command::new("xclip")
            .args(["-selection", "primary", "-i"])
            .env("DISPLAY", DISPLAY)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        drop(xclip.stdin.take()); // пустой ввод тоже делает xclip владельцем PRIMARY
        let primary = match next() {
            Msg::Primary(size) => size,
            _ => panic!("ждали Primary"),
        };
        assert_eq!(primary, 1);
        let _ = xclip.kill();
        let _ = xclip.wait();
    }
}

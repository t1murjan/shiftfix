# shiftfix

Аналог Caramba Switcher для Linux на Rust: двойной Shift исправляет раскладку последнего слова или выделенного текста. Автопереключения нет. Пользователь работает в CachyOS (KDE Plasma, Wayland) и Omarchy (Hyprland). Отвечать по-русски.

## Устройство

- `src/main.rs` — служба `shiftfix` и её CLI: `--status`, `--toggle`, `--check`, `--print-config`, `--print-unit`.
- `src/bin/shiftfix-gui/` — окно на eframe/egui 0.36 с вкладками «Главная», «Устройства», «Настройки», «Инструкция», «Журнал».
- `engine.rs` — логика без ввода-вывода. Буфер хранит скан-коды. Исправить слово = переключить раскладку, нажать Backspace×N и повторить те же клавиши через uinput.
- `devices.rs` — evdev и роли устройств: клавиатура, мышь, тачпад, экран, перо; роль можно задать вручную.
- `backend.rs` — переключение раскладки: KDE (DBus), GNOME (gsettings + Super+Space), Hyprland, Sway, X11 (XKB), хоткей.
- `xkb.rs` — keymap от композитора через libxkbcommon, загружаемую dlopen.
- `selection.rs`, `clipboard.rs`, `x11.rs` — выделение. Wayland с data-control идёт через wl-clipboard, X11 и GNOME через XWayland — через xclip и XFixes.
- `env.rs` — поиск сеанса: своё окружение, окружение systemd, живые сокеты.
- `session.rs` — проверка logind `session/auto` Active, чтобы не печатать в чужой сеанс.
- `service.rs` — юнит systemd.
- `ipc.rs` — сокет `$XDG_RUNTIME_DIR/shiftfix.sock` (JSON-строки).
- `install.sh` — сборка и установка для пользователя.
- `uninstall.sh` — полное удаление: откатывает всё, что ставят install.sh и GUI (параметры `--yes`, `--keep-config`, `--keep-group`, `--keep-build`).
- `BUGS.md` — найденные при ревью ошибки: место в коде, сценарий и как исправлено.

## Состояние (версия 0.3.0)

Собирается; `cargo test`, `cargo clippy --all-targets` и `shellcheck install.sh` проходят без замечаний.

Что вошло в 0.3.0:
- трей ждёт панель (`assume_sni_available`), в `Status` — `tray` (значок виден) и `tray_waiting`;
- охранник движка: `session::blocked` (logind `session/auto` Active, есть графический сеанс);
- живые сокеты и окружение systemd user (`env.rs`);
- Bluetooth-клавиатуры после входа, служба не падает без клавиатуры;
- keyd (vendor 0x0FAC), мёртвые клавиши;
- пароли и файлы в буфере обмена (`restore_plan`);
- GNOME (рамка окна, раскладка через источники ввода, выделение через XWayland) и X11 (XKB, XFixes);
- выделение выбирается в `selection.rs`: `Msg::Selection(Ok(чем) | Err(почему))`, в `Status` — `selection_via`;
- `--print-unit` (`service.rs`) и `--check` с сеансом и источником выделения;
- GUI: бэкенды gnome и x11, один экземпляр окна, ошибки (в том числе паника winit) — через notify-send;
- install.sh: без root, rustup на Arch, ostree и `/usr/lib/group`, NixOS, проверка `cc`, xclip, юнит через `--print-unit`;
- `uninstall.sh` и исправления 12 ошибок из ревью — что было и как исправлено, в `BUGS.md`.

Проверяется только на машине автора: реальный ввод и uinput в композиторе, трей на панели, PRIMARY в живых KDE, Hyprland и GNOME, logind.

## Команды

```sh
cargo test                 # X11-тест сам поднимает Xvfb :97 (нужны Xvfb, setxkbmap, xclip)
cargo clippy --all-targets
shellcheck install.sh uninstall.sh
cargo run --bin shiftfix -- --check
cargo run --bin shiftfix-gui -- --page devices
```

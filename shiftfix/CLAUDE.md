# shiftfix

Аналог Caramba Switcher для Linux на Rust: двойной Shift исправляет раскладку последнего слова или выделенного текста. Автопереключения нет. Пользователь работает в CachyOS (KDE Plasma, Wayland) и Omarchy (Hyprland). Отвечать по-русски.

## Устройство

- `src/bin/shiftfix/` — служба `shiftfix` и её CLI: `--status`, `--toggle`, `--print-config`, `--print-unit`, `--print-autostart`; `--check` — в `check.rs`.
- `src/bin/shiftfix-gui/` — окно на eframe/egui 0.36 с вкладками «Главная», «Устройства», «Настройки», «Инструкция», «Журнал».
- `engine.rs` — логика без ввода-вывода. Буфер хранит скан-коды. Исправить слово = переключить раскладку, нажать Backspace×N и повторить те же клавиши через uinput.
- `devices.rs` — evdev и роли устройств: клавиатура, мышь, тачпад, экран, перо; роль можно задать вручную.
- `backend/` — переключение раскладки (по файлу на окружение, выбор — `auto.rs`): KDE (DBus), GNOME (gsettings + Super+Space), Hyprland, Sway, X11 (XKB), хоткей.
- `keymap/` — таблицы раскладок: keymap от композитора (`wayland.rs`) или X11 (`x11.rs`), разбор через libxkbcommon, загружаемую dlopen (`xkb.rs`), встроенные us/ru (`layout.rs`), перекодировка выделения (`convert.rs`); `keys.rs` — классы клавиш.
- `selection/` — выделение: выбор источника (`mod.rs`), Wayland с data-control — wl-clipboard (`wayland.rs`), X11 и GNOME через XWayland — xclip и XFixes (`x11.rs`), возврат буфера обмена (`clipboard.rs`).
- `sys/` — связь с системой: окружение сеанса, logind, DBus, подключение к Wayland и X11, запуск команд.
- `sys/env.rs` — поиск сеанса: своё окружение, окружение systemd, живые сокеты.
- `sys/session.rs` — проверка logind `session/auto` Active, чтобы не печатать в чужой сеанс.
- `service.rs` — тексты юнита systemd и файла автозапуска (`.desktop`): их печатают `--print-unit` и `--print-autostart` и пишет окно.
- `ipc.rs` — сокет `$XDG_RUNTIME_DIR/shiftfix.sock` (JSON-строки).
- `portal.rs` — XDG Desktop Portal (`org.freedesktop.appearance`: `color-scheme`, `accent-color`) на zbus blocking, со сменой на лету; служба его не вызывает, только окно.
- `src/bin/shiftfix-gui/theme.rs` — палитры (данными в `PALETTES`), режим, акцент и контраст текста на нём (WCAG) → `egui::Visuals`; тема хранится в `[theme]` конфига (`config::ThemeSettings`).
- `install.sh` — установка и обновление одним идемпотентным скриптом: версии (установка / обновление / уже установлена / более старая — с вопросом), сборка до остановки, атомарная замена, sudo только для недостающего, служба запускается снова, только если работала; конфиг и выбор автозапуска не трогает.
- `uninstall.sh` — удаление: откатывает то, что ставят install.sh и GUI, в том числе правило udev и группу input; настройки и `target/` — только с `--purge` (ещё `--yes`, `--keep-group`, `--keep-build`).
- `BUGS.md` — найденные при ревью ошибки: место в коде, сценарий и как исправлено.

## Состояние (версия 0.4.0)

Собирается; `cargo test`, `cargo clippy --all-targets` и `shellcheck install.sh uninstall.sh` проходят без замечаний.

Что вошло в 0.4.0 (логика службы и исправления раскладки не менялись):
- темы окна: режим (системная / светлая / тёмная), палитры Breeze, Nord, Catppuccin, Gruvbox для каждого варианта, свой акцент, «акцент как в системе»; текст на акценте — чёрный или белый по контрасту WCAG; всё применяется сразу и хранится в `[theme]` конфига (старый конфиг без неё читается, `gui-theme` переносится);
- «Системная» — по XDG Desktop Portal (`portal.rs`), меняется на лету; без портала — тёмная и предупреждение в лог; `color-scheme` 0 («нет предпочтения») — тоже тёмная;
- одна версия из Cargo.toml: `shiftfix --version`, `shiftfix-gui --version`, «О программе» во вкладке «Инструкция»; `shiftfix --print-autostart`;
- install.sh — установка и обновление одним скриптом (см. «Устройство»); uninstall.sh — настройки и `target/` только с `--purge`.

Проверено в контейнере от временного пользователя (поддельные systemctl, cargo и sudo с журналами): первая установка, затем обновление поверх (конфиг байт в байт тот же, служба и окно перезапущены, sudo не нужен), повтор без изменений, падение сборки, более старая версия, выключенный автозапуск, путь без systemd, удаление.

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

# Как вносить правки

Как устроена программа — в [docs/architecture.md](docs/architecture.md). Там же список решений, которые не меняются без обсуждения, и известные грабли.

## Сборка и проверки

Нужен Rust 1.95 или новее (edition 2024).

```sh
cargo fmt
cargo build --all-targets
cargo clippy --all-targets        # без предупреждений
cargo test
shellcheck -x install.sh uninstall.sh scripts/common.sh tests/install/run.sh tests/install/fakes/*
```

То же самое запускает CI на GitHub при каждом push (`.github/workflows/ci.yml`).

Для части тестов нужны внешние программы; без них такие тесты пропускаются и пишут об этом:

- `tests/x11.rs` — Xvfb, setxkbmap и xclip: сам поднимает Xvfb на `:97`;
- тесты KDE и портала — `dbus-daemon`: каждый поднимает свою шину с поддельным сервисом;
- тесты таблиц раскладок — libxkbcommon.

Release-сборка с LTO идёт 3–4 минуты. Для быстрой проверки:

```sh
CARGO_TARGET_DIR=/tmp/sf-fast CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 \
    cargo build --release --locked
```

## Окно без настоящей службы

Окно можно открыть под Xvfb с поддельной службой — она отвечает на `status` так же, как настоящая:

```sh
export XDG_RUNTIME_DIR=/tmp/sfg DISPLAY=:96 WAYLAND_DISPLAY= XDG_CONFIG_HOME=/tmp/sfgcfg
mkdir -m 700 -p $XDG_RUNTIME_DIR $XDG_CONFIG_HOME/shiftfix
./target/debug/shiftfix --print-config > $XDG_CONFIG_HOME/shiftfix/config.toml
Xvfb :96 -screen 0 1100x800x24 -nolisten tcp -noreset &
python3 -I tests/install/fake_daemon.py $XDG_RUNTIME_DIR 0.4.0 &
./target/debug/shiftfix-gui --page settings --theme light &
sleep 7 && import -window root shot.png    # окно поднимается 6–8 секунд
```

Без сессионной шины DBus окно пишет «портал настроек недоступен — тёмная тема» — это нормально.

## Стенд для install.sh и uninstall.sh

`tests/install/run.sh` прогоняет 20 сценариев — установка, повтор, обновление, ошибка сборки, более старая версия, выключенный автозапуск, Ctrl+C во время остановки, без systemd, удаление, удаление во время установки — от временного пользователя с поддельными `systemctl`, `cargo` и `sudo`. На каждый сценарий он пишет журнал: код выхода, вывод скрипта, вызовы `sudo`, `systemctl` и `cargo`, файлы в домашнем каталоге и в `/etc`.

**Только в одноразовом контейнере:** стенд создаёт пользователя, пишет в `/etc` и может создать группу `input` (в конце всё убирает).

```sh
cargo build --bins
git worktree add /tmp/sf-before HEAD          # скрипты до правки
sudo tests/install/run.sh --container /tmp/sf-before target/debug /tmp/journal-before
sudo tests/install/run.sh --container . target/debug /tmp/journal-after
diff -r /tmp/journal-before /tmp/journal-after
```

Если правка не должна менять поведение, журналы совпадают байт в байт.

## Что проверять на настоящей машине

В контейнере не проверить: ввод через uinput в живом композиторе, значок в трее, PRIMARY в KDE, Hyprland и GNOME, logind, настоящие systemd и udev, тему из XDG Desktop Portal. Если что-то из этого сломалось, первым делом нужны:

```sh
shiftfix --check
shiftfix --status
journalctl --user -u shiftfix -n 100
```

## Коммиты

- Небольшими шагами: каждый коммит собирается, проходит clippy и тесты.
- Сообщения по-русски. Тема — «область: что сделано», например `install.sh: …`, `shiftfix-gui: …`, `config: …`. В теле — что изменилось и как проверено.

## Новая версия

Версия одна — в `Cargo.toml`. Поднимая её, выполните `cargo update -w --offline`: иначе `cargo build --locked` в `install.sh` упадёт. Что вошло — в [CHANGELOG.md](CHANGELOG.md).

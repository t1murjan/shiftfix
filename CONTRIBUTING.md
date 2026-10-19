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

То же самое запускает CI на GitHub при каждом push (`.github/workflows/ci.yml`), и ещё две задачи: `cargo check --all-targets` на Rust 1.95 (`rust-version` из `Cargo.toml`) и стенд для `install.sh` и `uninstall.sh` (ниже).

Для части тестов нужны внешние программы; без них такие тесты пропускаются и пишут «— пропуск: …» (видно с `cargo test -- --nocapture`):

- `tests/x11.rs` — Xvfb, setxkbmap и xclip: сам поднимает Xvfb на свободном дисплее;
- тесты KDE, logind и портала — `dbus-daemon`: каждый поднимает свою шину с поддельным сервисом;
- тесты таблиц раскладок — libxkbcommon.

`SHIFTFIX_TEST_STRICT=1 cargo test` превращает такой пропуск в падение — чтобы в CI тест не «прошёл», не запустившись. Пропуск из-за окружения, а не нехватки программ, остаётся пропуском: живой X-сервер (тест не трогает раскладку сеанса), рабочий стол со своим переключателем раскладки (тест запасного хоткея).

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
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)   # версия из Cargo.toml
python3 -I tests/install/fake_daemon.py $XDG_RUNTIME_DIR "$version" &
./target/debug/shiftfix-gui --page settings --theme light &   # или --page help, --theme dark
sleep 7 && import -window root shot.png    # окно поднимается 6–8 секунд
```

`import` — из ImageMagick. Вкладки: `home`, `devices`, `settings`, `help`, `log`.

Автоматической проверки страниц окна нет: правку окна проверяют глазами по снимку экрана, до и после, в обеих темах.

Советы и оговорки вкладки «Инструкция» (`src/bin/shiftfix-gui/pages/help.rs`) есть и в README: «Если не работает» — в «Если не работает»; «Стоит знать» — в «Ограничениях», а про GNOME и Sway — в «Где работает»; про выделение в терминале (карточка «Исправить выделенный текст») и про keyd и kanata («Устройства») — тоже в «Ограничениях». Тексты совпадают по смыслу, не дословно; только в README — запуск с `--debug` и fcitx5/IBus. Новую проблему или оговорку добавляйте в оба места.

Без сессионной шины DBus окно пишет «портал настроек недоступен — тёмная тема» — это нормально.

Ответ поддельной службы на `status` лежит в `tests/install/fake_status.json` в формате `ipc::Status`; версию и путь к конфигу (`$XDG_CONFIG_HOME` или `~/.config`) она подставляет сама. Поменялся `ipc::Status` — поправьте и этот файл: тест `ipc::tests::install_stand_status_matches` проверяет, что файл разбирается в `Status` без потерь.

## Стенд для install.sh и uninstall.sh

`tests/install/run.sh` прогоняет сценарии — установка, повтор, доставка недостающего, обновление, ошибка сборки, более старая версия, выключенный автозапуск, Ctrl+C во время остановки, без systemd, выключенный юнит при автозапуске сеанса и при первой установке, удаление, удаление и установка во время другой установки, оборванная первая установка и её доделка, пробел в пути домашнего каталога, обновление вне графического сеанса, служба-процесс без systemd и переход с автозапуска сеанса на юнит, упавшая служба, нет sudo, `install.sh --uninstall` с `--keep-build` — от временного пользователя с поддельными `systemctl`, `cargo` и `sudo`. На каждый сценарий он пишет журнал: код выхода, вывод скрипта, вызовы `sudo`, `systemctl` и `cargo`, файлы в домашнем каталоге и в `/etc`.

**Только в одноразовом контейнере:** стенд создаёт пользователя, пишет в `/etc` и может создать группу `input` (в конце всё убирает).

```sh
cargo build --bins
git worktree add /tmp/sf-before HEAD          # скрипты до правки
sudo tests/install/run.sh --container /tmp/sf-before target/debug /tmp/journal-before
sudo tests/install/run.sh --container . target/debug /tmp/journal-after
diff -r /tmp/journal-before /tmp/journal-after
```

Если правка не должна менять поведение, журналы совпадают байт в байт. Если меняет намеренно — по `diff -r` убедитесь, что изменилось только ожидаемое.

У каждого сценария в `run.sh` записан ожидаемый код выхода. Если код какого-то сценария другой, `run.sh` в конце перечисляет такие сценарии и выходит с кодом 1. Правка меняет код намеренно — поправьте его и в `run.sh`.

Сценарии идут подряд, и каждый начинает с того, что оставил предыдущий. Новый сценарий добавляйте в конец, с коротким комментарием, что он проверяет: вставленный в середину может поменять журналы всех следующих.

В CI стенд — задача `install-scripts`: собирает `target/debug` и прогоняет `run.sh`; неожиданный код выхода валит задачу. Журналы сценариев — в артефакте `install-journal` прогона, и при падении тоже.

## Что проверять на настоящей машине

В контейнере не проверить: ввод через uinput в живом композиторе, значок в трее, PRIMARY в KDE, Hyprland и GNOME, logind, настоящие systemd и udev, тему из XDG Desktop Portal. Если что-то из этого сломалось, первым делом нужны:

```sh
shiftfix --check
shiftfix --status
journalctl --user -u shiftfix -n 100
```

Подробный журнал — служба в терминале с `--debug` (Ctrl+C — остановить, потом `systemctl --user start shiftfix`):

```sh
systemctl --user stop shiftfix; shiftfix --debug
```

## Коммиты

- Небольшими шагами: каждый коммит собирается, проходит clippy и тесты.
- Сообщения по-русски. Тема — «область: что сделано», например `install.sh: …`, `shiftfix-gui: …`, `config: …`. В теле — что изменилось и как проверено.

## Новая версия

Версия одна — в `Cargo.toml`. Поднимая её, выполните `cargo update -w --offline`: иначе `cargo build --locked` в `install.sh` упадёт. Что вошло — в [CHANGELOG.md](CHANGELOG.md).

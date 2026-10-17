# shiftfix

Аналог Caramba Switcher для Linux на Rust: двойной Shift исправляет раскладку последнего слова или выделенного текста. Автопереключения нет. Автор работает в CachyOS (KDE Plasma, Wayland) и Omarchy (Hyprland).

## Где что

- [README.md](README.md) — для пользователя: установка, обновление, окно, команды.
- [docs/architecture.md](docs/architecture.md) — как устроено, решения, которые не меняются без спроса, и грабли. Прочитать перед правками.
- [CONTRIBUTING.md](CONTRIBUTING.md) — сборка, проверки, окно под Xvfb, стенд `tests/install`, коммиты, новая версия.
- [CHANGELOG.md](CHANGELOG.md) — что вошло в каждую версию.
- Карта модулей — в начале `src/lib.rs`.

## Как работать

- Отвечать по-русски, просто и по делу.
- Сначала изучить и предложить план, дождаться подтверждения. Спорные решения — вопросом с вариантами и рекомендацией.
- Небольшими шагами: после каждого — `cargo fmt`, `cargo clippy --all-targets` без предупреждений, `cargo test`, для скриптов `shellcheck`, и отдельный коммит (по-русски, «область: что сделано», в теле — как проверено).
- Проверять по-настоящему, а не «должно работать»: тесты, вывод команд, окно под Xvfb, стенд `tests/install` для скриптов. В конце — короткий итог: что сделано, как проверено, что проверить автору на своей машине.
- Логику службы и исправления раскладки (`engine/`, `daemon/`, `devices/`, `backend/`, `selection/`, `keymap/`) без прямой просьбы не менять.
- Просят не коммитить — не коммитить. Pull request — только по просьбе.

## Команды

```sh
cargo test                       # тест на Xvfb сам поднимает :97 (нужны Xvfb, setxkbmap, xclip)
cargo clippy --all-targets
shellcheck install.sh uninstall.sh tests/install/run.sh tests/install/fakes/*
cargo run --bin shiftfix -- --check
cargo run --bin shiftfix-gui -- --page settings --theme light
```

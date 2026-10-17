#!/usr/bin/env bash
# Удаление shiftfix: всё, что поставили install.sh и окно shiftfix-gui.
# Настройки и сборка остаются — их стирает --purge. Параметры — ./uninstall.sh --help.
# Этапы по порядку — в main в конце файла.
set -euo pipefail

usage() {
    cat <<'EOF'
Использование: ./uninstall.sh [ПАРАМЕТРЫ]

Удаляет shiftfix: останавливает службу и окно, удаляет программы, ярлык,
значки, автозапуск, правило udev для /dev/uinput и выводит вас из группы input.
Настройки (~/.config/shiftfix) остаются — поставите снова, и всё будет как было.
Сначала показывает, что нашлось, и спрашивает.

  -y, --yes        не спрашивать
  --purge          удалить и настройки, и сборку (target/ рядом со скриптом)
  --keep-group     не выводить из группы input (если она нужна другим программам)
  --keep-build     с --purge: оставить сборку
  -h, --help       эта справка

Пакеты (wl-clipboard, xclip, libnotify, компилятор C) и Rust не удаляются:
ими пользуются и другие программы.
EOF
}

# Параметры.
parse_args() {
    local arg
    yes=0 purge=0 keep_group=0 keep_build=0
    for arg in "$@"; do
        case $arg in
        -y | --yes) yes=1 ;;
        --purge) purge=1 ;;
        --keep-config)
            # Так было до 0.4.0; теперь настройки и так остаются без --purge.
            echo "uninstall.sh: --keep-config больше не нужен — настройки удаляет только --purge" >&2
            ;;
        --keep-group) keep_group=1 ;;
        --keep-build) keep_build=1 ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            printf 'uninstall.sh: неизвестный параметр %s\n\n' "$arg" >&2
            usage >&2
            exit 2
            ;;
        esac
    done
}

# От кого запущен скрипт.
check_system() {
    if ((EUID == 0)); then
        echo "Запускайте uninstall.sh от своего пользователя, без sudo: программа стоит в" >&2
        echo "домашнем каталоге, а sudo скрипт попросит сам — для правила udev и группы input." >&2
        exit 1
    fi
    if [[ -z ${HOME:-} || ! -d $HOME ]]; then
        echo "uninstall.sh: не задан домашний каталог (HOME)" >&2
        exit 1
    fi
}

# Где что лежит — те же пути, что в install.sh.
init_paths() {
    HERE="$(cd "$(dirname "$0")" && pwd)"
    ME="$(id -un)"
    MY_UID="$(id -u)"
    # Те же пути, что в install.sh.
    BIN="$HOME/.local/bin"
    DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
    CONF="${XDG_CONFIG_HOME:-$HOME/.config}"
    UNITS="$CONF/systemd/user"
    UDEV_RULE=/etc/udev/rules.d/80-shiftfix-uinput.rules
    MODULES=/etc/modules-load.d/shiftfix-uinput.conf
    # Как в службе (env.rs): из консоли или по ssh XDG_RUNTIME_DIR бывает не задан —
    # тогда systemctl --user не видит службу, хотя она запущена.
    if [[ -z ${XDG_RUNTIME_DIR:-} && -d /run/user/$MY_UID ]]; then
        export XDG_RUNTIME_DIR="/run/user/$MY_UID"
    fi
    RUNTIME="${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}"
}

# Помощники.

step() { printf '\n\033[1;36m==>\033[0m %s\n' "$*"; }
note() { printf '    %s\n' "$*"; }
have() { command -v "$1" >/dev/null 2>&1; }
user_systemd() { have systemctl && systemctl --user show-environment >/dev/null 2>&1; }
pretty() { printf '%s' "${1/#"$HOME"/\~}"; }
running() { if have pgrep; then pgrep -x -u "$MY_UID" 'shiftfix(-gui)?' || true; fi; }

# Добавить в files те пути, что существуют (и висячие ссылки).
found() {
    local path
    for path; do
        if [[ -e $path || -L $path ]]; then files+=("$path"); fi
    done
}

# Что нашлось: файлы, системные настройки, группа, процессы и копии,
# поставленные не через install.sh.
find_installed() {
    local path name unit
    files=()
    found "$BIN/shiftfix" "$BIN/shiftfix-gui" \
        "$DATA/applications/shiftfix.desktop" \
        "$DATA/icons/hicolor/scalable/apps/shiftfix.svg" \
        "$DATA/icons/hicolor/scalable/apps/shiftfix-paused.svg" \
        "$CONF/autostart/shiftfix.desktop" \
        "$UNITS/shiftfix.service" "$UNITS/shiftfix.service.d" \
        "$UNITS"/*.wants/shiftfix.service
    settings=
    if [[ -e $CONF/shiftfix || -L $CONF/shiftfix ]]; then settings="$CONF/shiftfix"; fi
    if ((purge)) && [[ -n $settings ]]; then found "$settings"; fi

    # Сборку стираем только с --purge и только если скрипт лежит в папке проекта shiftfix.
    build=
    if ((purge && !keep_build)) && [[ -d $HERE/target && -f $HERE/Cargo.toml ]] &&
        grep -q '^name = "shiftfix"' "$HERE/Cargo.toml"; then
        build="$HERE/target"
    fi

    system=()
    for path in "$UDEV_RULE" "$MODULES"; do
        if [[ -e $path ]]; then system+=("$path"); fi
    done
    leave_group=0
    if ((!keep_group)) && [[ " $(id -nG "$ME") " == *" input "* ]]; then
        leave_group=1
    fi
    processes="$(running)"

    # Копии, которые поставил не install.sh (пакет, cargo install): их не трогаем.
    others=()
    for name in shiftfix shiftfix-gui; do
        while IFS= read -r path; do
            if [[ -n $path && ! $path -ef $BIN/$name && " ${others[*]} " != *" $path "* ]]; then
                others+=("$path")
            fi
        done < <(type -ap "$name" || true)
    done
    if user_systemd; then
        unit="$(systemctl --user show -p FragmentPath --value shiftfix.service 2>/dev/null || true)"
        if [[ -n $unit && $unit != "$UNITS/shiftfix.service" ]]; then others+=("$unit"); fi
    fi
}

# Сказать, что будет удалено; если удалять нечего — выйти.
show_plan() {
    local path size
    if ((${#files[@]} == 0 && ${#system[@]} == 0 && !leave_group)) && [[ -z $build && -z $processes ]]; then
        echo "shiftfix не установлен — удалять нечего."
        if [[ -n $settings ]] && ((!purge)); then
            note "Настройки остались: $(pretty "$settings") (удалить: ./uninstall.sh --purge)"
        fi
        for path in "${others[@]}"; do note "Есть копия, поставленная не через install.sh: $path"; done
        exit 0
    fi

    echo "Будет удалено:"
    for path in "${files[@]}"; do note "$(pretty "$path")"; done
    if [[ -n $build ]]; then
        size="$(du -sh -- "$build" 2>/dev/null | cut -f1)" || size="?"
        note "$(pretty "$build") — сборка, $size"
    fi
    for path in "${system[@]}"; do note "$path (через sudo)"; done
    if ((leave_group)); then
        note "вы ($ME) — из группы input (через sudo; она даёт любой программе читать клавиатуру)"
    fi
    if [[ -n $processes ]]; then note "будут остановлены: служба и окно shiftfix"; fi
    if [[ -n $settings ]] && ((!purge)); then
        note "настройки остаются: $(pretty "$settings") (удалить и их: --purge)"
    fi
}

# Спросить (без --yes) и сразу — пароль sudo, до того как что-то удалять.
confirm() {
    local answer
    if ((!yes)); then
        if [[ ! -t 0 ]]; then
            echo "Нет терминала, чтобы спросить, — запустите с --yes." >&2
            exit 1
        fi
        echo
        read -r -p "Удалить? [y/N] " answer || answer=
        # Через case, а не [YyДд]: без UTF-8 локали скобка сравнивает байты, и «н»
        # (тот же первый байт, что у «Д») сошло бы за «да».
        case $answer in
        [Yy]* | Д* | д*) ;;
        *) answer= ;;
        esac
        if [[ -z $answer ]]; then
            echo "Ничего не удалено."
            exit 0
        fi
    fi

        need_root=0
    if ((${#system[@]} || leave_group)); then need_root=1; fi
    root_ok=0
    if ((need_root)) && have sudo && sudo -v; then root_ok=1; fi
}

# Остановить службу и окно.
stop_running() {
    step "Остановка службы и окна"
    if user_systemd; then
        systemctl --user disable --now shiftfix.service >/dev/null 2>&1 || true
    fi
    if [[ -x $BIN/shiftfix ]]; then
        "$BIN/shiftfix" --quit >/dev/null 2>&1 || true # запущена без systemd
    fi
    if have pkill; then
        pkill -x -u "$MY_UID" 'shiftfix(-gui)?' || true
        for _ in {1..30}; do
            if [[ -z $(running) ]]; then break; fi
            sleep 0.1
        done
        pkill -KILL -x -u "$MY_UID" 'shiftfix(-gui)?' || true
    fi
}

# Файлы в домашнем каталоге, сокеты, сборка (с --purge), кеши меню.
remove_files() {
    local tool
    step "Файлы в домашнем каталоге"
    if ((${#files[@]})); then rm -rf -- "${files[@]}"; fi
    rm -f -- "$RUNTIME/shiftfix.sock" "$RUNTIME/shiftfix.lock" "$RUNTIME/shiftfix-gui.lock" \
        "$RUNTIME/shiftfix-gui.sock" "$RUNTIME/shiftfix-install.lock"
    if [[ -n $build ]]; then rm -rf -- "$build"; fi
    if user_systemd; then
        systemctl --user daemon-reload || true
        systemctl --user reset-failed shiftfix.service >/dev/null 2>&1 || true
    fi
    # Меню приложений и значки: иначе ярлык ещё повисит в кешах.
    if have update-desktop-database; then update-desktop-database "$DATA/applications" 2>/dev/null || true; fi
    if have gtk-update-icon-cache && [[ -f $DATA/icons/hicolor/icon-theme.cache ]]; then
        gtk-update-icon-cache -q "$DATA/icons/hicolor" 2>/dev/null || true
    fi
    for tool in kbuildsycoca6 kbuildsycoca5; do
        if have "$tool"; then "$tool" >/dev/null 2>&1 || true; fi
    done
    note "Готово."
}

# Правило udev, modules-load и группа input — через sudo.
remove_system() {
    if ((need_root)); then
        step "Системные настройки"
        if ((root_ok)); then
            if ((${#system[@]})); then
                sudo rm -f -- "${system[@]}"
                sudo udevadm control --reload-rules 2>/dev/null || true
                sudo udevadm trigger --sysname-match=uinput 2>/dev/null || true
                note "Правило udev удалено: права на /dev/uinput вернулись к системным."
            fi
            if ((leave_group)); then
                if sudo gpasswd -d "$ME" input >/dev/null; then
                    note "Вы выведены из группы input — подействует после перезахода в систему."
                else
                    note "Не получилось вывести из группы input: sudo gpasswd -d $ME input"
                fi
            fi
        else
            note "Без sudo это не удалить — выполните от root:"
            if ((${#system[@]})); then note "  rm -f ${system[*]} && udevadm control --reload-rules"; fi
            if ((leave_group)); then note "  gpasswd -d $ME input"; fi
        fi
    fi
}

# Итог и что осталось.
print_summary() {
    local path
    echo
    if ((need_root && !root_ok)); then
        echo "shiftfix удалён, кроме системных настроек — для них нужны команды выше."
    else
        echo "shiftfix удалён."
    fi
    if [[ -n $settings ]] && ((!purge)); then
        note "Настройки остались: $(pretty "$settings") — удалить и их: ./uninstall.sh --purge"
    fi
    if [[ -f $HERE/install.sh && -f $HERE/Cargo.toml ]]; then
        note "Папка с исходниками осталась: $(pretty "$HERE") — удалите её сами, если не нужна."
    fi
    note "Не удалялись (ими пользуются и другие программы): wl-clipboard, xclip, libnotify, компилятор C."
    if have rustup; then
        note "Rust тоже остался. Если ставили его только ради shiftfix: rustup self uninstall"
    fi
    for path in "${others[@]}"; do
        note "Осталась копия, поставленная не через install.sh: $path — удалите её тем же способом, каким ставили."
    done
}

# Что этапы нашли (files, system, leave_group…), они оставляют в глобальных
# переменных: их читают следующие этапы.
main() {
    parse_args "$@"
    check_system
    init_paths
    find_installed
    show_plan
    confirm
    stop_running
    remove_files
    remove_system
    print_summary
}

main "$@"

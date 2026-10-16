#!/usr/bin/env bash
# Полное удаление shiftfix: всё, что поставили install.sh и окно shiftfix-gui.
# Параметры — ./uninstall.sh --help.
set -euo pipefail

usage() {
    cat <<'EOF'
Использование: ./uninstall.sh [ПАРАМЕТРЫ]

Удаляет shiftfix с компьютера полностью: останавливает службу и окно, удаляет
программы, ярлык, значки, автозапуск, настройки, правило udev для /dev/uinput,
выводит вас из группы input и стирает сборку (target/ рядом со скриптом).
Сначала показывает, что нашлось, и спрашивает.

  -y, --yes        не спрашивать
  --keep-config    оставить настройки (~/.config/shiftfix)
  --keep-group     не выводить из группы input (если она нужна другим программам)
  --keep-build     оставить сборку (target/)
  -h, --help       эта справка

Пакеты (wl-clipboard, xclip, libnotify, компилятор C) и Rust не удаляются:
ими пользуются и другие программы.
EOF
}

yes=0 keep_config=0 keep_group=0 keep_build=0
for arg in "$@"; do
    case $arg in
    -y | --yes) yes=1 ;;
    --keep-config) keep_config=1 ;;
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

if ((EUID == 0)); then
    echo "Запускайте uninstall.sh от своего пользователя, без sudo: программа стоит в" >&2
    echo "домашнем каталоге, а sudo скрипт попросит сам — для правила udev и группы input." >&2
    exit 1
fi
if [[ -z ${HOME:-} || ! -d $HOME ]]; then
    echo "uninstall.sh: не задан домашний каталог (HOME)" >&2
    exit 1
fi

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
# Из консоли или по ssh XDG_RUNTIME_DIR бывает не задан — тогда systemctl --user
# не видит службу, хотя она запущена.
if [[ -z ${XDG_RUNTIME_DIR:-} && -d /run/user/$MY_UID ]]; then
    export XDG_RUNTIME_DIR="/run/user/$MY_UID"
fi
RUNTIME="${XDG_RUNTIME_DIR:-/run/user/$MY_UID}"

step() { printf '\n\033[1;36m==>\033[0m %s\n' "$*"; }
note() { printf '    %s\n' "$*"; }
have() { command -v "$1" >/dev/null 2>&1; }
user_systemd() { have systemctl && systemctl --user show-environment >/dev/null 2>&1; }
pretty() { printf '%s' "${1/#"$HOME"/\~}"; }
running() { if have pgrep; then pgrep -x -u "$MY_UID" 'shiftfix(-gui)?' || true; fi; }

# ─── Что нашлось ────────────────────────────────────────────────────────────

files=()
found() {
    local path
    for path; do
        if [[ -e $path || -L $path ]]; then files+=("$path"); fi
    done
}
found "$BIN/shiftfix" "$BIN/shiftfix-gui" \
    "$DATA/applications/shiftfix.desktop" \
    "$DATA/icons/hicolor/scalable/apps/shiftfix.svg" \
    "$DATA/icons/hicolor/scalable/apps/shiftfix-paused.svg" \
    "$CONF/autostart/shiftfix.desktop" \
    "$UNITS/shiftfix.service" "$UNITS/shiftfix.service.d" \
    "$UNITS"/*.wants/shiftfix.service
if ((!keep_config)); then
    found "$CONF/shiftfix"
fi

# Сборку стираем, только если скрипт лежит в папке проекта shiftfix.
build=
if ((!keep_build)) && [[ -d $HERE/target && -f $HERE/Cargo.toml ]] &&
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

if ((${#files[@]} == 0 && ${#system[@]} == 0 && !leave_group)) && [[ -z $build && -z $processes ]]; then
    echo "shiftfix не установлен — удалять нечего."
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
if ((keep_config)); then note "настройки остаются: $(pretty "$CONF/shiftfix")"; fi

if ((!yes)); then
    if [[ ! -t 0 ]]; then
        echo "Нет терминала, чтобы спросить, — запустите с --yes." >&2
        exit 1
    fi
    echo
    read -r -p "Удалить? [y/N] " answer || answer=
    if [[ ! $answer =~ ^[YyДд] ]]; then
        echo "Ничего не удалено."
        exit 0
    fi
fi

# Пароль sudo спрашиваем до того, как что-то удалять.
need_root=0
if ((${#system[@]} || leave_group)); then need_root=1; fi
root_ok=0
if ((need_root)) && have sudo && sudo -v; then root_ok=1; fi

# ─── Удаление ───────────────────────────────────────────────────────────────

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

step "Файлы в домашнем каталоге"
if ((${#files[@]})); then rm -rf -- "${files[@]}"; fi
rm -f -- "$RUNTIME/shiftfix.sock" "$RUNTIME/shiftfix.lock" "$RUNTIME/shiftfix-gui.lock"
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

echo
if ((need_root && !root_ok)); then
    echo "shiftfix удалён, кроме системных настроек — для них нужны команды выше."
else
    echo "shiftfix удалён."
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

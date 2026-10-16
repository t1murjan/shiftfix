#!/usr/bin/env bash
# Установка и обновление shiftfix для текущего пользователя — один скрипт.
#   ./install.sh              — установить, обновить или проверить, что всё на месте
#   ./install.sh --uninstall  — удалить (то же, что ./uninstall.sh)
# Повторный запуск ничего не ломает: меняется только то, что отличается.
# Пакеты ставятся сами в Arch/CachyOS/Manjaro, Debian/Ubuntu/Mint, Fedora и openSUSE.
set -euo pipefail
cd "$(dirname "$0")"

usage() {
    cat <<'EOF'
Использование: ./install.sh [ПАРАМЕТРЫ]

Собирает shiftfix и ставит его в ~/.local/bin, а если он уже стоит — обновляет.
Сначала сборка: если она не удалась, установленная версия остаётся как была.
Настройки (~/.config/shiftfix/config.toml) не перезаписываются никогда.
Работавшая служба после обновления запускается снова, выключенный автозапуск
остаётся выключенным. sudo нужен только для недостающего: пакетов, правила
udev для /dev/uinput и группы input.

  -y, --yes        не спрашивать (более старая версия, Rust через rustup)
  --uninstall      удалить shiftfix — то же, что ./uninstall.sh (его параметры)
  -h, --help       эта справка
EOF
}

for arg in "$@"; do
    if [[ $arg == --uninstall ]]; then
        rest=()
        for other in "$@"; do
            if [[ $other != --uninstall ]]; then rest+=("$other"); fi
        done
        exec ./uninstall.sh "${rest[@]}"
    fi
done
yes=0
for arg in "$@"; do
    case $arg in
    -y | --yes) yes=1 ;;
    -h | --help)
        usage
        exit 0
        ;;
    *)
        printf 'install.sh: неизвестный параметр %s\n\n' "$arg" >&2
        usage >&2
        exit 2
        ;;
    esac
done

if ((EUID == 0)); then
    echo "Запускайте install.sh от своего пользователя, без sudo: служба, конфиг и" >&2
    echo "бинарники ставятся в домашний каталог, а sudo скрипт попросит сам." >&2
    exit 1
fi
if [[ -z ${HOME:-} || ! -d $HOME ]]; then
    echo "install.sh: не задан домашний каталог (HOME)" >&2
    exit 1
fi
if [[ -e /etc/NIXOS ]] || grep -qs '^ID=nixos' /etc/os-release; then
    cat >&2 <<'EOF'
В NixOS этот скрипт не подходит: /etc и пакеты задаются конфигурацией системы.
Добавьте в configuration.nix:
    hardware.uinput.enable = true;          # /dev/uinput для группы uinput
    users.users.<вы>.extraGroups = [ "input" "uinput" ];
    environment.systemPackages = with pkgs; [ wl-clipboard xclip libnotify ];
Затем соберите shiftfix (cargo build --release) в nix-shell с rustc, cargo и gcc
и запустите юнит из `target/release/shiftfix --print-unit`.
EOF
    exit 1
fi
if [[ ! -f Cargo.toml ]]; then
    echo "install.sh: рядом нет Cargo.toml — запускайте скрипт из папки проекта shiftfix" >&2
    exit 1
fi

ME="$(id -un)"
MY_UID="$(id -u)"
BIN="$HOME/.local/bin"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
CONF="${XDG_CONFIG_HOME:-$HOME/.config}"
ICONS="$DATA/icons/hicolor/scalable/apps"
APPS="$DATA/applications"
UNIT="$CONF/systemd/user/shiftfix.service"
AUTOSTART="$CONF/autostart/shiftfix.desktop"
CONFIG="$CONF/shiftfix/config.toml"
UDEV_RULE=/etc/udev/rules.d/80-shiftfix-uinput.rules
UDEV_TEXT='KERNEL=="uinput", GROUP="input", MODE="0660", OPTIONS+="static_node=uinput"'
MODULES=/etc/modules-load.d/shiftfix-uinput.conf
MODULES_TEXT=uinput
TARGET="${CARGO_TARGET_DIR:-target}/release"
# Как в службе (env.rs): из консоли или по ssh XDG_RUNTIME_DIR бывает не задан —
# тогда systemctl --user не видит службу, хотя она запущена.
if [[ -z ${XDG_RUNTIME_DIR:-} && -d /run/user/$MY_UID ]]; then
    export XDG_RUNTIME_DIR="/run/user/$MY_UID"
fi
RUNTIME="${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}"

step() { printf '\n\033[1;36m==>\033[0m %s\n' "$*"; }
note() { printf '    %s\n' "$*"; }
have() { command -v "$1" >/dev/null 2>&1; }
pretty() { printf '%s' "${1/#"$HOME"/\~}"; }
in_group() { [[ " $1 " == *" input "* ]]; }
user_systemd() { have systemctl && systemctl --user show-environment >/dev/null 2>&1; }
not() { ! "$@"; }

version_ok() { # $1 >= $2
    [[ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -1)" == "$2" ]]
}
rust_ok() {
    have cargo && have rustc && version_ok "$(rustc --version | awk '{print $2}')" "$MIN_RUST"
}
# Значение ключа из [package] в Cargo.toml: версия одна — там.
package_field() {
    awk -F'"' -v key="$1" '
        /^\[/ { pkg = ($0 == "[package]") }
        pkg && $1 ~ "^" key " *= *$" { print $2; exit }' Cargo.toml
}

# Спросить да/нет; $2 — ответ по умолчанию (y|n). Без терминала нужен --yes.
ask() {
    local answer
    if ((yes)); then return 0; fi
    if [[ ! -t 0 ]]; then
        echo "Нет терминала, чтобы спросить, — запустите с --yes." >&2
        exit 1
    fi
    if [[ $2 == y ]]; then
        read -r -p "    $1 [Y/n] " answer || answer=n
        [[ ${answer:-y} =~ ^[YyДд] ]]
    else
        read -r -p "    $1 [y/N] " answer || answer=
        [[ $answer =~ ^[YyДд] ]]
    fi
}

# Файл блокировки держит живой процесс (служба и окно держат свой flock).
held() { [[ -e $1 ]] && have flock && ! flock -n "$1" true 2>/dev/null; }
daemon_running() {
    held "$RUNTIME/shiftfix.lock" || { have pgrep && pgrep -x -u "$MY_UID" shiftfix >/dev/null; }
}
gui_running() {
    held "$RUNTIME/shiftfix-gui.lock" || { have pgrep && pgrep -x -u "$MY_UID" shiftfix-gui >/dev/null; }
}
# Повторять команду до успеха: $1 попыток по 0.1 с.
wait_for() {
    local tries=$1
    shift
    while ((tries-- > 0)); do
        if "$@"; then return 0; fi
        sleep 0.1
    done
    return 1
}
# Запустить отдельно от скрипта и терминала; блокировку install.sh (fd 9) не передаём.
launch() {
    (cd "$HOME" && exec setsid -f "$@" >/dev/null 2>&1 </dev/null 9>&-)
}
installed_version() {
    local out=
    if [[ -e $BIN/shiftfix || -L $BIN/shiftfix ]]; then
        if have timeout; then
            out="$(timeout 5 "$BIN/shiftfix" --version 2>/dev/null)" || out=
        else
            out="$("$BIN/shiftfix" --version 2>/dev/null)" || out=
        fi
        out="${out#shiftfix }"
        [[ $out =~ ^[0-9]+(\.[0-9A-Za-z+~-]+)*$ ]] || out="?"
    fi
    printf '%s' "$out"
}
status_ok() { "$BIN/shiftfix" --status >/dev/null 2>&1; }
# Автозапуск сеанса есть и не выключен (KDE и GNOME выключают его строкой в файле).
autostart_on() {
    [[ -e $AUTOSTART ]] && ! grep -Eqs '^(Hidden=true|X-GNOME-Autostart-enabled=false)' "$AUTOSTART"
}

# Записать файл атомарно: временный рядом и mv — оборванная запись не оставит полфайла.
temps=()
put() { # $1 — источник, $2 — куда, $3 — права
    local tmp
    tmp="$(dirname -- "$2")/.$(basename -- "$2").new.$$"
    temps+=("$tmp")
    install -m "$3" -- "$1" "$tmp"
    mv -fT -- "$tmp" "$2"
}
put_text() { # $1 — куда, $2 — текст
    local tmp
    tmp="$(dirname -- "$1")/.$(basename -- "$1").new.$$"
    temps+=("$tmp")
    printf '%s\n' "$2" >"$tmp"
    mv -fT -- "$tmp" "$1"
}

# Если скрипт оборвётся после остановки службы, запустить её снова — старую или новую.
was_active=0 was_proc=0 gui_was=0 stopped=0 restarted=0
cleanup() {
    if ((${#temps[@]})); then rm -f -- "${temps[@]}"; fi
    if ((stopped && !restarted)); then
        restarted=1
        echo "install.sh: прервано — запускаю службу снова" >&2
        if ((was_active)); then
            systemctl --user start shiftfix.service >/dev/null 2>&1 || true
        elif ((was_proc)); then
            launch "$BIN/shiftfix" || true
        fi
        if ((gui_was)); then launch "$BIN/shiftfix-gui" || true; fi
    fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM HUP

# Два install.sh одновременно поставили бы файлы наперегонки.
if have flock && { exec 9>"$RUNTIME/shiftfix-install.lock"; } 2>/dev/null; then
    if ! flock -n 9; then
        echo "install.sh уже запущен в другом терминале — дождитесь его." >&2
        exit 1
    fi
fi

NEW="$(package_field version)"
MIN_RUST="$(package_field rust-version)"
if [[ -z $NEW || -z $MIN_RUST ]]; then
    echo "install.sh: в Cargo.toml не найдены version и rust-version" >&2
    exit 1
fi

# ─── Что стоит сейчас ───────────────────────────────────────────────────────

OLD="$(installed_version)"
OLD_TEXT="${OLD/#\?/(неизвестная)}"
if [[ -z $OLD ]]; then
    mode=install
    headline="установка $NEW"
elif [[ $OLD == "?" ]]; then
    mode=update
    headline="обновление: установленная версия неизвестна → $NEW"
elif [[ $OLD == "$NEW" ]]; then
    mode=same
    headline="уже установлена $NEW — проверяю, всё ли на месте"
elif version_ok "$OLD" "$NEW"; then
    mode=downgrade
    headline="установлена более новая версия: $OLD → $NEW"
else
    mode=update
    headline="обновление $OLD → $NEW"
fi
printf '\033[1mshiftfix: %s\033[0m\n' "$headline"
if [[ $mode == downgrade ]]; then
    if ! ask "Поставить $NEW вместо более новой $OLD?" n; then
        echo "Ничего не изменено."
        exit 0
    fi
fi

# ─── Зависимости ────────────────────────────────────────────────────────────

step "Зависимости"
package_of() { # $1 — менеджер пакетов, $2 — недостающая команда
    case $2 in
    wl-paste | wl-copy) echo wl-clipboard ;;
    xclip) echo xclip ;;
    notify-send)
        case $1 in
        apt-get) echo libnotify-bin ;;
        zypper) echo libnotify-tools ;;
        *) echo libnotify ;;
        esac
        ;;
    cc)
        case $1 in
        pacman) echo base-devel ;;
        apt-get) echo build-essential ;;
        *) echo gcc ;;
        esac
        ;;
    esac
}
if ! rust_ok && [[ -f $HOME/.cargo/env ]]; then
    # shellcheck source=/dev/null
    source "$HOME/.cargo/env"
fi
pm=
if have pacman; then
    pm=pacman
elif [[ -e /run/ostree-booted ]]; then
    pm=ostree # Silverblue, Kinoite, Bazzite…: пакеты слоями rpm-ostree требуют перезагрузки
else
    for candidate in apt-get dnf zypper; do
        if have "$candidate"; then
            pm=$candidate
            break
        fi
    done
fi
missing=() pkgs=()
for tool in wl-paste wl-copy xclip notify-send cc; do
    if ! have "$tool"; then
        missing+=("$tool")
        pkg="$(package_of "$pm" "$tool")"
        if [[ " ${pkgs[*]} " != *" $pkg "* ]]; then pkgs+=("$pkg"); fi
    fi
done
# Пакет rust конфликтует с rustup: при rustup свежий Rust ставится через него.
if [[ $pm == pacman ]] && ! rust_ok && ! have rustup; then pkgs+=(rust); fi
if ((${#pkgs[@]} == 0)); then
    note "всё на месте"
else
    case $pm in
    pacman) sudo pacman -S --needed --noconfirm "${pkgs[@]}" ;;
    apt-get) sudo apt-get install -y --no-upgrade "${pkgs[@]}" ;;
    dnf) sudo dnf install -y "${pkgs[@]}" ;;
    zypper) sudo zypper install -y "${pkgs[@]}" ;;
    ostree)
        note "Неизменяемая система (ostree), не хватает: ${missing[*]}."
        note "Поставьте ${pkgs[*]} через rpm-ostree install (и перезагрузитесь)"
        note "или соберите shiftfix в toolbox/distrobox."
        ;;
    *) note "Неизвестный пакетный менеджер: поставьте сами ${pkgs[*]} (не хватает: ${missing[*]})." ;;
    esac
fi
if ! have cc; then
    note "Нет компилятора C (cc): он нужен Rust для сборки. Поставьте gcc или clang и запустите снова."
    exit 1
fi
if ! rust_ok && have rustup; then
    if ! version_ok "$(rustup run stable rustc --version 2>/dev/null | awk '{print $2}')" "$MIN_RUST"; then
        note "Ставлю свежий Rust через rustup — только для сборки shiftfix"
        rustup toolchain install stable --profile minimal
    fi
    # Версию по умолчанию не трогаем: у других проектов она может быть выбрана нарочно.
    export RUSTUP_TOOLCHAIN=stable
fi
if ! rust_ok; then
    note "Нужен Rust $MIN_RUST или новее (сейчас: $(rustc --version 2>/dev/null || echo нет))."
    if ! have curl; then
        note "Поставьте Rust (https://rustup.rs) или curl и запустите снова."
        exit 1
    fi
    if ! ask "Установить свежий Rust через rustup в ~/.cargo?" y; then exit 1; fi
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
    # shellcheck source=/dev/null
    source "$HOME/.cargo/env"
fi

# ─── Сборка — до того, как что-то трогать ───────────────────────────────────

step "Сборка (первый раз — несколько минут)"
if ! cargo build --release --locked; then
    echo >&2
    if [[ $mode == install ]]; then
        echo "Сборка не удалась — ничего не установлено." >&2
    else
        echo "Сборка не удалась — установленная версия $OLD_TEXT не тронута." >&2
    fi
    exit 1
fi
for name in shiftfix shiftfix-gui; do
    got="$("$TARGET/$name" --version 2>/dev/null)" || got=
    if [[ $got != "$name $NEW" ]]; then
        echo "Собранный $TARGET/$name сообщает «$got», а в Cargo.toml версия $NEW — не ставлю." >&2
        exit 1
    fi
done
note "собрано: $NEW"

# ─── Доступ к /dev/uinput — только недостающее ──────────────────────────────

step "Доступ к /dev/uinput"
need_udev=0 need_modules=0 need_group=0 relogin=0
if [[ "$(cat -- "$UDEV_RULE" 2>/dev/null)" != "$UDEV_TEXT" ]]; then need_udev=1; fi
if [[ "$(cat -- "$MODULES" 2>/dev/null)" != "$MODULES_TEXT" ]]; then need_modules=1; fi
if ! in_group "$(id -nG "$ME" 2>/dev/null)"; then need_group=1; fi
# В группе по базе, но не в этом сеансе — значит, добавили недавно.
if ! in_group "$(id -nG 2>/dev/null)"; then relogin=1; fi
if ((need_udev + need_modules + need_group == 0)); then
    system_state="уже были"
    note "уже настроено: правило udev, загрузка uinput, группа input"
else
    system_state="настроены сейчас"
    note "Нужно сделать через sudo:"
    if ((need_udev)); then note "  правило udev $UDEV_RULE — /dev/uinput для группы input"; fi
    if ((need_modules)); then note "  $MODULES — загружать модуль uinput при старте"; fi
    if ((need_group)); then note "  добавить вас ($ME) в группу input — чтение клавиатуры"; fi
    if ! have sudo || ! sudo -v; then
        echo "Без sudo не обойтись. Выполните от root и запустите install.sh снова:" >&2
        if ((need_udev)); then echo "  echo '$UDEV_TEXT' > $UDEV_RULE" >&2; fi
        if ((need_modules)); then echo "  echo $MODULES_TEXT > $MODULES" >&2; fi
        if ((need_group)); then echo "  usermod -aG input $ME" >&2; fi
        exit 1
    fi
    if ((need_udev)); then
        sudo mkdir -p "$(dirname "$UDEV_RULE")"
        printf '%s\n' "$UDEV_TEXT" | sudo tee "$UDEV_RULE" >/dev/null
    fi
    if ((need_modules)); then
        sudo mkdir -p "$(dirname "$MODULES")"
        printf '%s\n' "$MODULES_TEXT" | sudo tee "$MODULES" >/dev/null
    fi
    if [[ ! -e /dev/uinput ]] && ((need_modules || need_udev)); then
        sudo modprobe uinput 2>/dev/null || true
    fi
    if ((need_udev)); then
        sudo udevadm control --reload-rules 2>/dev/null || true
        sudo udevadm trigger --sysname-match=uinput 2>/dev/null || true
    fi
    if ((need_group)); then
        # В неизменяемых системах (ostree) системные группы лежат в /usr/lib/group,
        # и usermod их не видит, пока строка не перенесена в /etc/group.
        if ! grep -qs '^input:' /etc/group && grep -qs '^input:' /usr/lib/group; then
            grep '^input:' /usr/lib/group | sudo tee -a /etc/group >/dev/null
        elif ! getent group input >/dev/null; then
            sudo groupadd --system input
        fi
        sudo usermod -aG input "$ME"
        relogin=1
    fi
fi
uinput="$(stat -c '%G %a' /dev/uinput 2>/dev/null)" || uinput=
if [[ -z $uinput ]]; then
    note "Устройства /dev/uinput пока нет — появится после перезагрузки."
elif [[ $uinput != "input 660" ]]; then
    note "/dev/uinput сейчас: группа и права «$uinput» — правило udev подействует после перезагрузки."
fi

# ─── Что изменится ──────────────────────────────────────────────────────────

desktop="$(mktemp)"
temps+=("$desktop")
sed "s|@BINDIR@|$BIN|g" assets/shiftfix.desktop >"$desktop"
srcs=("$TARGET/shiftfix" "$TARGET/shiftfix-gui" assets/shiftfix.svg assets/shiftfix-paused.svg "$desktop")
dsts=("$BIN/shiftfix" "$BIN/shiftfix-gui" "$ICONS/shiftfix.svg" "$ICONS/shiftfix-paused.svg" "$APPS/shiftfix.desktop")
perms=(755 755 644 644 644)
changed=()
for i in "${!srcs[@]}"; do
    if ! cmp -s -- "${srcs[i]}" "${dsts[i]}" || { ((perms[i] == 755)) && [[ ! -x ${dsts[i]} ]]; }; then
        changed+=("$i")
    fi
done
daemon_changed=0 gui_changed=0 menu_changed=0
for i in "${changed[@]}"; do
    case $i in
    0) daemon_changed=1 ;;
    1) gui_changed=1 ;;
    *) menu_changed=1 ;;
    esac
done
if [[ $mode == same ]] && ((daemon_changed || gui_changed)); then
    note "Файлы $NEW отличаются от собранных — переустановка."
fi

# ─── Остановка — только если программа меняется ─────────────────────────────

if ((daemon_changed)); then
    if user_systemd && systemctl --user is-active --quiet shiftfix.service; then
        was_active=1
    elif daemon_running; then
        was_proc=1
    fi
fi
if ((gui_changed)) && gui_running; then gui_was=1; fi
if ((was_active || was_proc || gui_was)); then
    step "Остановка"
    stopped=1
    if ((was_active || was_proc)); then
        if ((was_active)); then systemctl --user stop shiftfix.service 2>/dev/null || true; fi
        if daemon_running; then
            if have timeout; then
                timeout 5 "$BIN/shiftfix" --quit >/dev/null 2>&1 || true
            else
                "$BIN/shiftfix" --quit >/dev/null 2>&1 || true
            fi
        fi
        if ! wait_for 30 not daemon_running; then
            pkill -TERM -x -u "$MY_UID" shiftfix 2>/dev/null || true
            if ! wait_for 20 not daemon_running; then
                pkill -KILL -x -u "$MY_UID" shiftfix 2>/dev/null || true
                wait_for 10 not daemon_running || true
            fi
        fi
        note "служба остановлена"
    fi
    if ((gui_was)); then
        note "окно shiftfix закрывается и откроется снова — несохранённые правки настроек пропадут"
        pkill -TERM -x -u "$MY_UID" shiftfix-gui 2>/dev/null || true
        if ! wait_for 30 not gui_running; then
            pkill -KILL -x -u "$MY_UID" shiftfix-gui 2>/dev/null || true
            wait_for 10 not gui_running || true
        fi
    fi
fi

# ─── Файлы ──────────────────────────────────────────────────────────────────

step "Файлы"
mkdir -p "$BIN" "$ICONS" "$APPS"
for i in "${changed[@]}"; do
    put "${srcs[i]}" "${dsts[i]}" "${perms[i]}"
    note "$(pretty "${dsts[i]}")"
done
if ((${#changed[@]} == 0)); then note "всё как в сборке — ничего не заменялось"; fi
if ((menu_changed)); then
    # Меню приложений и значки: иначе там ещё повисит старое.
    if have update-desktop-database; then update-desktop-database "$APPS" 2>/dev/null || true; fi
    if have gtk-update-icon-cache; then gtk-update-icon-cache -q "$DATA/icons/hicolor" 2>/dev/null || true; fi
    if have kbuildsycoca6; then kbuildsycoca6 >/dev/null 2>&1 || true; fi
fi

# Конфиг пользователя не перезаписываем: новых параметров в нём нет — берутся по умолчанию.
if [[ -e $CONFIG || -L $CONFIG ]]; then
    config_state="сохранены как были (новые параметры — по умолчанию)"
else
    text="$("$BIN/shiftfix" --print-config)"
    mkdir -p "$(dirname "$CONFIG")"
    put_text "$CONFIG" "$text"
    config_state="созданы"
fi

# ─── Автозапуск — выбор пользователя не трогаем ─────────────────────────────

if user_systemd; then
    new_unit="$("$BIN/shiftfix" --print-unit)"
    old_unit='' old_wanted='' new_wanted=''
    unit_existed=0
    if [[ -e $UNIT ]]; then
        unit_existed=1
        old_unit="$(cat -- "$UNIT")"
        old_wanted="$(sed -n 's/^WantedBy=//p' <<<"$old_unit")"
        new_wanted="$(sed -n 's/^WantedBy=//p' <<<"$new_unit")"
        # Вне графического сеанса (ssh, консоль) graphical-session.target не видна —
        # тогда прежний WantedBy вернее нового.
        if [[ -n $old_wanted && $old_wanted != "$new_wanted" ]] &&
            ! systemctl --user is-active --quiet graphical-session.target; then
            new_unit="${new_unit/$'\n'"WantedBy=$new_wanted"/$'\n'"WantedBy=$old_wanted"}"
            new_wanted=$old_wanted
        fi
    fi
    enabled=0
    if systemctl --user is-enabled --quiet shiftfix.service 2>/dev/null; then enabled=1; fi
    if [[ $old_unit != "$new_unit" ]]; then
        mkdir -p "$(dirname "$UNIT")"
        put_text "$UNIT" "$new_unit"
        systemctl --user daemon-reload
    fi
    if ((!unit_existed)); then
        # Юнита не было: первая установка или переход с автозапуска сеанса.
        if [[ $mode == install ]] || autostart_on; then
            if systemctl --user enable shiftfix.service >/dev/null 2>&1; then
                enabled=1
            else
                note "Автозапуск не включился: systemctl --user enable shiftfix"
            fi
        fi
    elif ((enabled)) && [[ $old_wanted != "$new_wanted" ]]; then
        systemctl --user reenable shiftfix.service >/dev/null 2>&1 || true
    fi
    # Службу запускает юнит — второй запуск из автозапуска сеанса не нужен.
    rm -f -- "$AUTOSTART"
    if ((enabled)); then
        autostart_state="юнит systemd $(pretty "$UNIT") — включён"
    else
        autostart_state="юнит systemd $(pretty "$UNIT") — выключен (как было)"
    fi
elif [[ -e $UNIT ]]; then
    autostart_state="юнит systemd есть, но systemd --user сейчас недоступен — не трогаю"
elif [[ -e $AUTOSTART ]] && ! autostart_on; then
    autostart_state="$(pretty "$AUTOSTART") — выключен (как было)"
elif [[ -e $AUTOSTART || $mode == install ]]; then
    text="$("$BIN/shiftfix" --print-autostart)"
    if [[ "$(cat -- "$AUTOSTART" 2>/dev/null)" != "$text" ]]; then
        mkdir -p "$(dirname "$AUTOSTART")"
        put_text "$AUTOSTART" "$text"
    fi
    autostart_state="автозапуск сеанса $(pretty "$AUTOSTART") — включён"
else
    # Файл удалили из окна (автозапуск выключен) — не возвращаем.
    autostart_state="выключен (как было)"
fi

# ─── Запуск ─────────────────────────────────────────────────────────────────

start=
if ((was_active)); then
    start=systemd
elif ((was_proc)); then
    start=process
elif [[ $mode == install ]] && ((!relogin)); then
    if user_systemd; then start=systemd; else start=process; fi
fi
restarted=1
if [[ -n $start ]]; then
    step "Запуск"
    if [[ $start == systemd ]]; then
        systemctl --user start shiftfix.service || true
    else
        wait_for 30 not held "$RUNTIME/shiftfix.lock" || true
        launch "$BIN/shiftfix"
    fi
    if wait_for 50 status_ok; then
        if ((stopped)); then service_state="перезапущена"; else service_state="запущена"; fi
        line="$("$BIN/shiftfix" --status 2>/dev/null)" || line=
        service_state+=": ${line%%$'\n'*}"
    else
        service_state="не поднялась — что не так, покажет: shiftfix --check"
        if user_systemd; then service_state+=" (журнал: journalctl --user -u shiftfix)"; fi
        if ((relogin)); then service_state+="; скорее всего, нужно перезайти в систему (группа input)"; fi
    fi
    note "служба $service_state"
elif ((relogin)) && [[ $mode == install ]]; then
    service_state="запустится после перезахода в систему (группа input)"
elif status_ok; then
    service_state="работает, не перезапускалась"
else
    service_state="не запущена (и до этого не работала)"
    if user_systemd && systemctl --user is-failed --quiet shiftfix.service; then
        service_state="остановилась с ошибкой — что не так, покажет: shiftfix --check"
    fi
fi
gui_state=
if ((gui_was)); then
    if [[ -n ${WAYLAND_DISPLAY:-}${DISPLAY:-} ]]; then
        wait_for 30 not held "$RUNTIME/shiftfix-gui.lock" || true
        launch "$BIN/shiftfix-gui"
        gui_state="открыто снова"
    else
        gui_state="было открыто — откройте снова из меню приложений"
    fi
fi

# ─── Итог ───────────────────────────────────────────────────────────────────

step "Итог"
case $mode in
install) summary="shiftfix $NEW установлен" ;;
update) summary="shiftfix обновлён: $OLD_TEXT → $NEW" ;;
downgrade) summary="shiftfix $NEW поставлен вместо $OLD" ;;
same)
    if ((daemon_changed || gui_changed)); then
        summary="shiftfix $NEW переустановлен (файлы изменились)"
    elif ((${#changed[@]})) || [[ $system_state != "уже были" ]]; then
        summary="shiftfix $NEW уже был установлен — недостающее поставлено"
    else
        summary="shiftfix $NEW уже установлен — всё на месте, ничего не менялось"
    fi
    ;;
esac
printf '\033[1m%s\033[0m\n' "$summary"
if ((daemon_changed || gui_changed)); then
    programs="обновлены"
    if [[ $mode == install ]]; then programs="установлены"; fi
else
    programs="без изменений"
fi
menu="без изменений"
if ((menu_changed)); then menu="обновлены"; fi
note "программы:  $(pretty "$BIN/shiftfix"), shiftfix-gui — $programs"
note "ярлык и значки: $menu"
note "настройки:  $(pretty "$CONFIG") — $config_state"
note "автозапуск: $autostart_state"
note "система:    правило udev, загрузка uinput, группа input — $system_state"
note "служба:     $service_state"
if [[ -n $gui_state ]]; then note "окно:       $gui_state"; fi
echo
if ((need_group)); then
    echo "Вас добавили в группу input — перезайдите в систему (или перезагрузитесь)."
elif ((relogin)); then
    echo "Группа input подействует после перезахода в систему (или перезагрузки)."
fi
echo "Откройте «shiftfix» в меню приложений: там настройки, устройства и инструкция."
[[ ":$PATH:" == *":$BIN:"* ]] || note "Совет: добавьте $BIN в PATH, чтобы команды shiftfix --toggle и --status работали в терминале."

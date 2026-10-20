# shellcheck shell=bash
# Общее для install.sh и uninstall.sh: пути, имена файлов и помощники.
# Подключается из обоих скриптов (source) и сам не запускается.

# Не от root и с домашним каталогом. $@ — строки, почему без sudo.
check_user() {
    if ((EUID == 0)); then
        printf '%s\n' "$@" >&2
        exit 1
    fi
    if [[ -z ${HOME:-} || ! -d $HOME ]]; then
        echo "${0##*/}: не задан домашний каталог (HOME)" >&2
        exit 1
    fi
}

# Куда что ставится — одни пути для установки и удаления.
# shellcheck disable=SC2034 # переменные читают install.sh и uninstall.sh
init_paths() {
    ME="$(id -un)"
    MY_UID="$(id -u)"
    BIN="$HOME/.local/bin"
    DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
    CONF="${XDG_CONFIG_HOME:-$HOME/.config}"
    ICONS="$DATA/icons/hicolor/scalable/apps"
    APPS="$DATA/applications"
    UNITS="$CONF/systemd/user"
    UNIT="$UNITS/shiftfix.service"
    AUTOSTART="$CONF/autostart/shiftfix.desktop"
    SETTINGS="$CONF/shiftfix"
    CONFIG="$SETTINGS/config.toml"
    # Что ставит install.sh из сборки и assets/ (в этом порядке) и удаляет uninstall.sh.
    # Роль — откуда файл и что обновить после замены: daemon и gui — программы
    # (перезапустить), icon — значок, menu — ярлык (обновить меню).
    # Новая роль — новая ветка в plan_files (install.sh).
    FILES=("$BIN/shiftfix" "$BIN/shiftfix-gui" "$ICONS/shiftfix.svg" "$ICONS/shiftfix-paused.svg"
        "$APPS/shiftfix.desktop")
    ROLES=(daemon gui icon icon menu)
    STATE="${XDG_STATE_HOME:-$HOME/.local/state}"
    # Метка «первая установка не закончена»: её ставит install.sh перед заменой файлов
    # и снимает в конце; пока она есть, повторный запуск доделывает первую установку.
    INCOMPLETE="$STATE/shiftfix-install-incomplete"
    # Путь к папке, откуда запускали install.sh: оттуда окно проверяет новую версию
    # и обновляет программу (git pull и ./install.sh в терминале).
    SOURCE_FILE="$STATE/shiftfix-source"
    UDEV_RULE=/etc/udev/rules.d/80-shiftfix-uinput.rules
    UDEV_TEXT='KERNEL=="uinput", GROUP="input", MODE="0660", OPTIONS+="static_node=uinput"'
    MODULES=/etc/modules-load.d/shiftfix-uinput.conf
    MODULES_TEXT=uinput
    # Каталог — как paths::runtime_dir (src/sys/paths.rs). Из консоли или по ssh
    # XDG_RUNTIME_DIR бывает не задан — тогда systemctl --user не видит службу, хотя
    # она запущена. Нет и /run/user/UID — свой каталог во временном, его создаёт take_lock.
    if [[ -z ${XDG_RUNTIME_DIR:-} && -d /run/user/$MY_UID ]]; then
        export XDG_RUNTIME_DIR="/run/user/$MY_UID"
    fi
    RUNTIME="${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}/shiftfix-$MY_UID}"
    # Блокировки и сокеты службы, окна и попапа (имена — в src/) и блокировка install.sh.
    DAEMON_LOCK="$RUNTIME/shiftfix.lock"
    DAEMON_SOCK="$RUNTIME/shiftfix.sock"
    GUI_LOCK="$RUNTIME/shiftfix-gui.lock"
    GUI_SOCK="$RUNTIME/shiftfix-gui.sock"
    POPUP_LOCK="$RUNTIME/shiftfix-popup.lock"
    POPUP_SOCK="$RUNTIME/shiftfix-popup.sock"
    INSTALL_LOCK="$RUNTIME/shiftfix-install.lock"
}

# Помощники.

step() { printf '\n\033[1;36m==>\033[0m %s\n' "$*"; }
note() { printf '    %s\n' "$*"; }
have() { command -v "$1" >/dev/null 2>&1; }
pretty() { printf '%s' "${1/#"$HOME"/\~}"; }
user_systemd() { have systemctl && systemctl --user show-environment >/dev/null 2>&1; }
not() { ! "$@"; }
# Есть ли input среди групп $1 (вывод id -nG).
in_group() { [[ " $1 " == *" input "* ]]; }

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
# Остановить процессы $1: TERM, ждать $3 попыток по 0.1 с, пока $2 не скажет, что их
# нет, потом KILL.
terminate() { # $1 — имя процесса (шаблон pkill -x), $2 — проверка «ещё работает», $3 — попыток
    pkill -TERM -x -u "$MY_UID" "$1" 2>/dev/null || true
    if ! wait_for "$3" not "$2"; then
        pkill -KILL -x -u "$MY_UID" "$1" 2>/dev/null || true
        wait_for 10 not "$2" || true
    fi
}

# «Да»: y, yes, д, да. Через case, а не [YyДд]: без UTF-8 локали скобка сравнивает
# байты, и «н» (тот же первый байт, что у «Д») сошло бы за «да».
is_yes() {
    case $1 in
    [Yy]* | Д* | д*) return 0 ;;
    *) return 1 ;;
    esac
}
# Спросить да/нет; $2 — ответ по умолчанию (y|n). С --yes (yes=1) — «да»,
# без терминала — выход: спросить некого.
# shellcheck disable=SC2154 # yes задаёт parse_args скрипта
ask() {
    local answer
    if ((yes)); then return 0; fi
    if [[ ! -t 0 ]]; then
        echo "Нет терминала, чтобы спросить, — запустите с --yes." >&2
        exit 1
    fi
    if [[ $2 == y ]]; then
        read -r -p "    $1 [Y/n] " answer || answer=n
        [[ -z $answer ]] || is_yes "$answer"
    else
        read -r -p "    $1 [y/N] " answer || answer=
        is_yes "$answer"
    fi
}

# Меню приложений и значки: иначе там ещё повисит старое. Кеш значков — только если
# он уже есть: без него GTK читает каталог сам. kbuildsycoca — кеш меню KDE (6 и 5).
refresh_menu_caches() {
    local tool
    if have update-desktop-database; then update-desktop-database "$APPS" 2>/dev/null 9>&- || true; fi
    if have gtk-update-icon-cache && [[ -f $DATA/icons/hicolor/icon-theme.cache ]]; then
        gtk-update-icon-cache -q "$DATA/icons/hicolor" 2>/dev/null 9>&- || true
    fi
    for tool in kbuildsycoca6 kbuildsycoca5; do
        if have "$tool"; then "$tool" >/dev/null 2>&1 9>&- || true; fi
    done
}

# Два install.sh или install.sh и uninstall.sh одновременно меняли бы файлы
# наперегонки. Блокировку держит fd 9 до выхода; дочерним процессам, которые
# живут дольше скрипта, — 9>&-. Файл блокировки не удаляется: второй скрипт
# создал бы новый и не заметил, что старый ещё занят.
take_lock() {
    # Своего каталога во временном (см. init_paths) может ещё не быть. Если каталог
    # с этим именем создал другой пользователь, файл блокировки в нём может оказаться
    # ссылкой на наш файл — тогда без блокировки, как без flock. Файл открывается без
    # усечения (>>), как у службы.
    if [[ ! -d $RUNTIME ]]; then mkdir -m 700 "$RUNTIME" 2>/dev/null || true; fi
    [[ -d $RUNTIME && -O $RUNTIME ]] || return 0
    if have flock && { exec 9>>"$INSTALL_LOCK"; } 2>/dev/null; then
        if ! flock -n 9; then
            echo "install.sh или uninstall.sh уже запущен в другом терминале — дождитесь его." >&2
            exit 1
        fi
    fi
}

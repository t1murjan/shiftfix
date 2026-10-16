#!/usr/bin/env bash
# Сборка и установка shiftfix для текущего пользователя.
#   ./install.sh              — собрать и установить
#   ./install.sh --uninstall  — удалить
# Пакеты ставятся сами в Arch/CachyOS/Manjaro, Debian/Ubuntu/Mint, Fedora и openSUSE.
set -euo pipefail
cd "$(dirname "$0")"

MIN_RUST="1.95"
BIN="$HOME/.local/bin"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
CONF="${XDG_CONFIG_HOME:-$HOME/.config}"
UNIT="$CONF/systemd/user/shiftfix.service"
AUTOSTART="$CONF/autostart/shiftfix.desktop"
UDEV_RULE=/etc/udev/rules.d/80-shiftfix-uinput.rules
MODULES=/etc/modules-load.d/shiftfix-uinput.conf

step() { printf '\n\033[1;36m==>\033[0m %s\n' "$*"; }
note() { printf '    %s\n' "$*"; }
in_group() { [[ " $1 " == *" input "* ]]; }
have() { command -v "$1" >/dev/null 2>&1; }
user_systemd() { have systemctl && systemctl --user show-environment >/dev/null 2>&1; }

uninstall() {
    step "Удаление shiftfix"
    if user_systemd; then
        systemctl --user disable --now shiftfix 2>/dev/null || true
    fi
    "$BIN/shiftfix" --quit 2>/dev/null || true
    rm -f "$BIN/shiftfix" "$BIN/shiftfix-gui" "$UNIT" "$AUTOSTART" \
        "$DATA/applications/shiftfix.desktop" \
        "$DATA/icons/hicolor/scalable/apps/shiftfix.svg" \
        "$DATA/icons/hicolor/scalable/apps/shiftfix-paused.svg"
    user_systemd && systemctl --user daemon-reload || true
    sudo rm -f "$UDEV_RULE" "$MODULES"
    note "Готово. Конфиг остался в $CONF/shiftfix — удалите его, если не нужен."
    exit 0
}
[[ ${1:-} == "--uninstall" ]] && uninstall

version_ok() { # $1 >= $2
    [[ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -1)" == "$2" ]]
}
rust_ok() {
    have cargo && have rustc && version_ok "$(rustc --version | awk '{print $2}')" "$MIN_RUST"
}

step "Зависимости"
if have pacman; then
    pkgs=(wl-clipboard libnotify base-devel)
    rust_ok || pkgs+=(rust)
    sudo pacman -S --needed --noconfirm "${pkgs[@]}"
elif have apt-get; then
    sudo apt-get install -y wl-clipboard libnotify-bin build-essential
elif have dnf; then
    sudo dnf install -y wl-clipboard libnotify gcc
elif have zypper; then
    sudo zypper install -y wl-clipboard libnotify-tools gcc
else
    note "Неизвестный пакетный менеджер: поставьте wl-clipboard, notify-send и компилятор C сами."
fi
if ! rust_ok; then
    # shellcheck source=/dev/null
    [[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"
fi
if ! rust_ok; then
    note "Нужен Rust $MIN_RUST или новее (сейчас: $(rustc --version 2>/dev/null || echo нет))."
    read -r -p "    Установить свежий Rust через rustup в ~/.cargo? [Y/n] " answer
    if [[ ${answer:-y} =~ ^[YyДд]?$ ]]; then
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
        # shellcheck source=/dev/null
        source "$HOME/.cargo/env"
    else
        exit 1
    fi
fi

step "Сборка (первый раз — несколько минут)"
cargo build --release --locked

step "Доступ к /dev/uinput для группы input"
echo 'KERNEL=="uinput", GROUP="input", MODE="0660", OPTIONS+="static_node=uinput"' | sudo tee "$UDEV_RULE" >/dev/null
echo uinput | sudo tee "$MODULES" >/dev/null
sudo modprobe uinput || true
sudo udevadm control --reload-rules
sudo udevadm trigger --sysname-match=uinput

relogin=0
if ! in_group "$(id -nG)"; then
    in_group "$(id -nG "$USER")" || sudo usermod -aG input "$USER"
    relogin=1
fi

step "Установка"
# Работающую службу останавливаем, чтобы файл не был занят.
user_systemd && systemctl --user stop shiftfix 2>/dev/null || true
install -Dm755 target/release/shiftfix "$BIN/shiftfix"
install -Dm755 target/release/shiftfix-gui "$BIN/shiftfix-gui"
install -Dm644 assets/shiftfix.svg "$DATA/icons/hicolor/scalable/apps/shiftfix.svg"
install -Dm644 assets/shiftfix-paused.svg "$DATA/icons/hicolor/scalable/apps/shiftfix-paused.svg"
mkdir -p "$DATA/applications"
sed "s|@BINDIR@|$BIN|g" assets/shiftfix.desktop >"$DATA/applications/shiftfix.desktop"
have update-desktop-database && update-desktop-database "$DATA/applications" 2>/dev/null || true
have gtk-update-icon-cache && gtk-update-icon-cache -q "$DATA/icons/hicolor" 2>/dev/null || true
have kbuildsycoca6 && kbuildsycoca6 >/dev/null 2>&1 || true

config="$CONF/shiftfix/config.toml"
if [[ ! -f $config ]]; then
    mkdir -p "$(dirname "$config")"
    "$BIN/shiftfix" --print-config >"$config"
    note "Конфиг: $config"
fi

if user_systemd; then
    mkdir -p "$(dirname "$UNIT")"
    cat >"$UNIT" <<EOF
[Unit]
Description=shiftfix: double Shift fixes the layout of the last word or selection
PartOf=graphical-session.target
After=graphical-session.target

[Service]
ExecStart=$BIN/shiftfix
Restart=on-failure
RestartSec=3
# Код 2 — ошибка настройки (нет доступа к устройствам и т.п.), перезапуск не поможет.
RestartPreventExitStatus=2

[Install]
WantedBy=graphical-session.target
EOF
    systemctl --user daemon-reload
    systemctl --user enable shiftfix >/dev/null
    rm -f "$AUTOSTART"
else
    mkdir -p "$(dirname "$AUTOSTART")"
    printf '[Desktop Entry]\nType=Application\nName=shiftfix\nExec=%s\nIcon=shiftfix\nNoDisplay=true\n' "$BIN/shiftfix" >"$AUTOSTART"
    note "Пользовательского systemd нет — служба будет запускаться через автозапуск сеанса."
fi

echo
if ((relogin)); then
    echo "Готово. Вас добавили в группу input — перезагрузитесь, служба запустится сама."
else
    if user_systemd; then
        systemctl --user restart shiftfix
    else
        nohup "$BIN/shiftfix" >/dev/null 2>&1 &
    fi
    echo "Готово, служба запущена."
fi
echo "Откройте «shiftfix» в меню приложений: там настройки, устройства и инструкция."
[[ ":$PATH:" == *":$BIN:"* ]] || note "Совет: добавьте $BIN в PATH, чтобы команды shiftfix --toggle и --status работали в терминале."

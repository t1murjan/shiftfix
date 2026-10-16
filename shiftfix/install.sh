#!/usr/bin/env bash
# Сборка и установка shiftfix для текущего пользователя.
#   ./install.sh              — собрать и установить
#   ./install.sh --uninstall  — удалить (то же, что ./uninstall.sh)
# Пакеты ставятся сами в Arch/CachyOS/Manjaro, Debian/Ubuntu/Mint, Fedora и openSUSE.
set -euo pipefail
cd "$(dirname "$0")"

if ((EUID == 0)); then
    echo "Запускайте install.sh от своего пользователя, без sudo: служба, конфиг и" >&2
    echo "бинарники ставятся в домашний каталог, а sudo скрипт попросит сам." >&2
    exit 1
fi
if [[ ${1:-} == "--uninstall" ]]; then
    shift
    exec ./uninstall.sh "$@"
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

version_ok() { # $1 >= $2
    [[ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -1)" == "$2" ]]
}
rust_ok() {
    have cargo && have rustc && version_ok "$(rustc --version | awk '{print $2}')" "$MIN_RUST"
}

step "Зависимости"
if have pacman; then
    pkgs=(wl-clipboard xclip libnotify base-devel)
    # Пакет rust конфликтует с rustup: при rustup свежий Rust ставится через него.
    rust_ok || have rustup || pkgs+=(rust)
    sudo pacman -S --needed --noconfirm "${pkgs[@]}"
elif [[ -e /run/ostree-booted ]]; then
    # Silverblue, Kinoite, Bazzite…: пакеты слоями rpm-ostree требуют перезагрузки.
    missing=()
    for tool in wl-paste xclip notify-send cc; do have "$tool" || missing+=("$tool"); done
    if ((${#missing[@]})); then
        note "Неизменяемая система (ostree), не хватает: ${missing[*]}."
        note "Поставьте wl-clipboard xclip libnotify gcc через rpm-ostree install (и перезагрузитесь)"
        note "или соберите shiftfix в toolbox/distrobox."
    fi
elif have apt-get; then
    sudo apt-get install -y wl-clipboard xclip libnotify-bin build-essential
elif have dnf; then
    sudo dnf install -y wl-clipboard xclip libnotify gcc
elif have zypper; then
    sudo zypper install -y wl-clipboard xclip libnotify-tools gcc
else
    note "Неизвестный пакетный менеджер: поставьте wl-clipboard, xclip, notify-send и компилятор C сами."
fi
if ! have cc; then
    note "Нет компилятора C (cc): он нужен Rust для сборки. Поставьте gcc или clang и запустите снова."
    exit 1
fi
if ! rust_ok; then
    # shellcheck source=/dev/null
    [[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"
fi
if ! rust_ok && have rustup; then
    note "Ставлю свежий Rust через rustup — только для сборки shiftfix"
    rustup toolchain install stable --profile minimal
    # Версию по умолчанию не трогаем: у других проектов она может быть выбрана нарочно.
    export RUSTUP_TOOLCHAIN=stable
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
    if ! in_group "$(id -nG "$USER")"; then
        # В неизменяемых системах (ostree) системные группы лежат в /usr/lib/group,
        # и usermod их не видит, пока строка не перенесена в /etc/group.
        if ! grep -q '^input:' /etc/group && grep -qs '^input:' /usr/lib/group; then
            grep '^input:' /usr/lib/group | sudo tee -a /etc/group >/dev/null
        fi
        sudo usermod -aG input "$USER"
    fi
    relogin=1
fi

step "Установка"
# Работающую службу останавливаем, чтобы файл не был занят.
if user_systemd; then systemctl --user stop shiftfix 2>/dev/null || true; fi
install -Dm755 target/release/shiftfix "$BIN/shiftfix"
install -Dm755 target/release/shiftfix-gui "$BIN/shiftfix-gui"
install -Dm644 assets/shiftfix.svg "$DATA/icons/hicolor/scalable/apps/shiftfix.svg"
install -Dm644 assets/shiftfix-paused.svg "$DATA/icons/hicolor/scalable/apps/shiftfix-paused.svg"
mkdir -p "$DATA/applications"
sed "s|@BINDIR@|$BIN|g" assets/shiftfix.desktop >"$DATA/applications/shiftfix.desktop"
if have update-desktop-database; then update-desktop-database "$DATA/applications" 2>/dev/null || true; fi
if have gtk-update-icon-cache; then gtk-update-icon-cache -q "$DATA/icons/hicolor" 2>/dev/null || true; fi
if have kbuildsycoca6; then kbuildsycoca6 >/dev/null 2>&1 || true; fi

config="$CONF/shiftfix/config.toml"
if [[ ! -f $config ]]; then
    mkdir -p "$(dirname "$config")"
    "$BIN/shiftfix" --print-config >"$config"
    note "Конфиг: $config"
fi

if user_systemd; then
    mkdir -p "$(dirname "$UNIT")"
    # Сам бинарник знает, поднимает ли этот сеанс graphical-session.target.
    "$BIN/shiftfix" --print-unit >"$UNIT"
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

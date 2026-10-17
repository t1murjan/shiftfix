#!/bin/bash
# Стенд для install.sh и uninstall.sh: сценарии установки, обновления и удаления
# от временного пользователя с поддельными systemctl, cargo и sudo.
#
# Только в одноразовом контейнере: создаёт пользователя sftest, пишет в /etc
# (sudoers, правило udev, modules-load), может создать группу input. В конце всё
# это убирается.
#
#   sudo tests/install/run.sh --container ПРОЕКТ БИНАРНИКИ ЖУРНАЛЫ
#
# ПРОЕКТ — папка с install.sh, uninstall.sh, scripts/, Cargo.toml и assets/;
# БИНАРНИКИ — папка с собранными shiftfix и shiftfix-gui (подойдёт target/debug);
# ЖУРНАЛЫ — сюда на каждый сценарий файл: код выхода, вывод скрипта, вызовы
# sudo, systemctl и cargo, файлы в домашнем каталоге. Чтобы проверить правку
# скриптов, прогоните стенд до и после неё и сравните: diff -r ДО ПОСЛЕ.
set -euo pipefail

if [[ ${1:-} != --container || $# -ne 4 ]]; then
    sed -n '2,15s/^# \{0,1\}//p' "$0" >&2
    exit 2
fi
if ((EUID != 0)); then
    echo "run.sh: нужен root" >&2
    exit 2
fi
PROJECT="$(readlink -f "$2")"
BINS="$(readlink -f "$3")"
OUT="$(readlink -m "$4")"
HERE="$(dirname "$(readlink -f "$0")")"
U=sftest
H=/home/$U
WORK=/opt/sftest
had_udev=0 had_group=0
[[ -d /etc/udev ]] && had_udev=1
getent group input >/dev/null && had_group=1

cleanup() {
    pkill -KILL -u "$U" 2>/dev/null || true
    sleep 0.3
    userdel -r "$U" 2>/dev/null || true
    rm -rf "/run/user/${UID_:-none}" "$WORK" /etc/sudoers.d/sftest
    rm -f /etc/udev/rules.d/80-shiftfix-uinput.rules /etc/modules-load.d/shiftfix-uinput.conf
    if ((!had_udev)); then rm -rf /etc/udev; fi
    if ((!had_group)); then groupdel input 2>/dev/null || true; fi
}
trap cleanup EXIT

# ─── Пользователь, подделки и проект ────────────────────────────────────────

useradd -m -s /bin/bash "$U"
UID_="$(id -u "$U")"
RUN=/run/user/$UID_
install -d -m 700 -o "$U" -g "$U" "$RUN"
echo "$U ALL=(ALL) NOPASSWD: ALL" >/etc/sudoers.d/sftest
chmod 440 /etc/sudoers.d/sftest

mkdir -p "$WORK"/{real,py,build}
cp -r "$HERE/fakes" "$WORK/fakes"
cp "$HERE/fake_daemon.py" "$WORK/"
cp "$BINS/shiftfix" "$BINS/shiftfix-gui" "$WORK/real/"
# Копия python с именем shiftfix: так pgrep -x shiftfix находит поддельную службу.
cp "$(readlink -f "$(command -v python3)")" "$WORK/py/shiftfix"
VERSION="$("$WORK/real/shiftfix" --version)"
VERSION="${VERSION#shiftfix }"
# «Собранная» служба: без параметров — поддельная (настоящей нужен /dev/uinput),
# с параметрами — настоящий shiftfix. В юнит и файл автозапуска настоящий пишет
# свой путь — подставляем путь обёртки, как это было бы с настоящей службой.
cat >"$WORK/build/shiftfix" <<EOF
#!/bin/bash
case \$1 in
"") exec $WORK/py/shiftfix -I $WORK/fake_daemon.py "\$XDG_RUNTIME_DIR" $VERSION ;;
--print-unit | --print-autostart)
    $WORK/real/shiftfix "\$@" | sed "s#$WORK/real/shiftfix#\$(readlink -f "\$0")#" ;;
*) exec $WORK/real/shiftfix "\$@" ;;
esac
EOF
cp "$WORK/real/shiftfix-gui" "$WORK/build/"
chmod -R a+rX "$WORK"
chmod 755 "$WORK/build/shiftfix"

SRC=$H/src/shiftfix
mkdir -p "$SRC"
cp -r "$PROJECT/install.sh" "$PROJECT/uninstall.sh" "$PROJECT/scripts" "$PROJECT/Cargo.toml" \
    "$PROJECT/assets" "$SRC/"
chown -R "$U:$U" "$H/src"
mkdir -p "$OUT"

# ─── Запуск и журнал сценария ───────────────────────────────────────────────

# Команда от пользователя в папке проекта, с подделками первыми в PATH.
as_user() {
    runuser -u "$U" -- env -i HOME="$H" PATH="$WORK/fakes:/usr/bin:/bin" \
        XDG_RUNTIME_DIR="$RUN" LANG=C.UTF-8 TERM=dumb FAKE_BUILD="$WORK/build" \
        bash -c "cd $SRC && $1"
}

# Убрать то, что меняется от запуска к запуску.
normalize() {
    sed -E "s#/run/user/$UID_#/run/user/UID#g; s/pid [0-9]+/pid N/g; s/\.new\.[0-9]+/.new.N/g"
}

# Что лежит в домашнем каталоге и в /etc после сценария.
snapshot() {
    (
        cd "$H"
        find . -path ./log -prune -o -path ./.fakesd -prune -o -path ./src -prune -o \
            \( -type f -o -type l \) -print | sort | while read -r f; do
            if [[ -L $f ]]; then
                echo "$f -> $(readlink "$f")"
            else
                echo "$f $(stat -c %a "$f") $(sha256sum <"$f" | cut -c1-16)"
            fi
        done
    )
    for f in /etc/udev/rules.d/80-shiftfix-uinput.rules /etc/modules-load.d/shiftfix-uinput.conf; do
        if [[ -e $f ]]; then echo "$f: $(cat "$f")"; else echo "$f: нет"; fi
    done
    echo "группы: $(id -nG "$U")"
    echo "служба: $(pgrep -x -u "$U" shiftfix >/dev/null && echo работает || echo нет)"
}

# Записать журнал сценария $1: код выхода $2, вывод из файла $3.
report() {
    {
        echo "== код выхода: $2"
        echo "== вывод"
        normalize <"$3"
        for log in sudo systemctl cargo daemon; do
            echo "== $log"
            if [[ -f $H/log/$log.log ]]; then normalize <"$H/log/$log.log"; fi
        done
        echo "== файлы"
        snapshot
    } >"$OUT/$1.txt"
    rm -f "$H"/log/{sudo,systemctl,cargo,daemon}.log
    echo "$1: код $2"
}

# Сценарий: имя и команда.
scenario() {
    local out code
    out="$(mktemp)"
    if as_user "$2" >"$out" 2>&1; then code=0; else code=$?; fi
    report "$1" "$code" "$out"
    rm -f "$out"
}

# Поставить в ~/.local/bin «установленную» службу версии $1.
installed() {
    as_user "mkdir -p ~/.local/bin && rm -f ~/.local/bin/shiftfix && printf '%s\n' \
        '#!/bin/bash' '[[ \$1 == --version ]] && { echo \"shiftfix $1\"; exit 0; }' \
        'exec $WORK/build/shiftfix \"\$@\"' >~/.local/bin/shiftfix && chmod 755 ~/.local/bin/shiftfix"
}

# ─── Сценарии ───────────────────────────────────────────────────────────────

scenario 01-install './install.sh'
scenario 02-same './install.sh'
as_user 'systemctl --user start shiftfix.service'
installed 0.3.0
scenario 03-update './install.sh'
installed 0.3.0
as_user 'touch ~/.fakesd/cargo-fail'
scenario 04-build-fails './install.sh'
as_user 'rm ~/.fakesd/cargo-fail'
scenario 05-update-after-fail './install.sh'
installed 99.0.0
scenario 06-older-no-terminal './install.sh'
scenario 07-older-answer-no "printf 'n\n' | script -qec ./install.sh /dev/null"
scenario 08-older-yes './install.sh --yes'
as_user 'systemctl --user disable shiftfix.service'
installed 0.3.0
scenario 09-autostart-disabled './install.sh'
as_user 'systemctl --user enable shiftfix.service'
as_user "rm ~/.local/bin/shiftfix && ln -s $WORK/build/shiftfix ~/.local/bin/shiftfix"
scenario 10-symlink './install.sh'

# Ctrl+C, пока служба останавливается, а вывод идёт через tee: служба должна
# подняться снова.
as_user 'systemctl --user start shiftfix.service; touch ~/.fakesd/slow-stop'
installed 0.3.0
out="$(mktemp)"
as_user 'exec setsid bash -c "./install.sh 2>&1 | tee ~/log/tee.out"' >"$out" 2>&1 &
for _ in {1..100}; do
    grep -qs -- '--user stop' "$H/log/systemctl.log" && break
    sleep 0.1
done
# Ctrl+C в терминале получают все процессы сеанса: install.sh, tee, systemctl.
if pid="$(pgrep -u "$U" -f '^bash ./install.sh' | head -1)" && [[ -n $pid ]]; then
    pkill -INT -s "$(ps -o sid= -p "$pid" | tr -d ' ')" || true
else
    echo "install.sh не дошёл до остановки службы" >>"$out"
fi
wait || true
sleep 1
report 11-interrupted-stop "-" "$out"
rm -f "$out"
as_user 'rm -f ~/.fakesd/slow-stop'

scenario 12-repair-after-interrupt './install.sh'
as_user 'touch ~/.fakesd/off'
scenario 13-no-systemd-unit-kept './install.sh'
as_user "./uninstall.sh --yes --keep-group >/dev/null 2>&1; rm -f ~/.config/systemd/user/shiftfix.service"
scenario 14-no-systemd-install './install.sh'
as_user 'rm ~/.fakesd/off'
scenario 15-uninstall-answer-no "printf 'n\n' | script -qec ./uninstall.sh /dev/null"
scenario 16-uninstall './uninstall.sh --yes'
scenario 17-uninstall-again './uninstall.sh --yes'
scenario 18-install-again './install.sh --yes'
scenario 19-uninstall-purge './uninstall.sh --yes --purge'

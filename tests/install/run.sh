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
    awk 'NR > 1 { if (!/^#/) exit; sub(/^# ?/, ""); print }' "$0" >&2
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
    # Убитый процесс, пока его не подобрал родитель (в контейнере init подбирает не
    # сразу), остаётся зомби с UID пользователя. Удалим пользователя раньше — следующий
    # прогон получит тот же UID, и pgrep в install.sh и в журнале найдёт старую службу.
    for _ in {1..100}; do
        pgrep -u "$U" >/dev/null 2>&1 || break
        sleep 0.1
    done
    if pgrep -u "$U" >/dev/null 2>&1; then
        echo "run.sh: у $U остались процессы: $(pgrep -u "$U" -d ' ')" >&2
    fi
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
cp "$HERE/fake_daemon.py" "$HERE/fake_status.json" "$WORK/"
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
# Журналы прошлого прогона: иначе от переименованных сценариев остались бы старые.
rm -f -- "${OUT:?}"/*.txt

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

# Работает ли у пользователя процесс $1. Зомби (см. cleanup) — уже не работает.
alive() { pgrep -x -u "$U" -r R,S,D,T "$1" >/dev/null; }

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
    echo "служба: $(alive shiftfix && echo работает || echo нет)"
}

# Записать журнал сценария $1: ожидаемый код выхода $2, настоящий $3, вывод из файла $4.
report() {
    {
        echo "== код выхода: $3"
        echo "== вывод"
        normalize <"$4"
        for log in sudo systemctl cargo daemon; do
            echo "== $log"
            if [[ -f $H/log/$log.log ]]; then normalize <"$H/log/$log.log"; fi
        done
        echo "== файлы"
        snapshot
    } >"$OUT/$1.txt"
    rm -f "$H"/log/{sudo,systemctl,cargo,daemon}.log
    if [[ $3 == "$2" ]]; then
        echo "$1: код $3"
    else
        echo "$1: код $3, а ожидался $2"
        unexpected+=("$1")
    fi
}

# Сценарий: имя, ожидаемый код выхода и команда.
scenario() {
    local out code
    out="$(mktemp)"
    if as_user "$3" >"$out" 2>&1; then code=0; else code=$?; fi
    report "$1" "$2" "$code" "$out"
    rm -f "$out"
}

# Поставить в ~/.local/bin «установленную» службу версии $1.
fake_installed_version() {
    as_user "mkdir -p ~/.local/bin && rm -f ~/.local/bin/shiftfix && printf '%s\n' \
        '#!/bin/bash' '[[ \$1 == --version ]] && { echo \"shiftfix $1\"; exit 0; }' \
        'exec $WORK/build/shiftfix \"\$@\"' >~/.local/bin/shiftfix && chmod 755 ~/.local/bin/shiftfix"
}

# Сценарий $1: install.sh с выводом через tee, Ctrl+C — как только он начнёт
# останавливать службу. Ctrl+C в терминале получают все процессы сеанса: install.sh,
# tee, systemctl. $2 — ожидаемый код выхода.
interrupted_stop() {
    local out pid job code
    out="$(mktemp)"
    as_user 'exec setsid bash -c "./install.sh 2>&1 | tee ~/log/tee.out"' >"$out" 2>&1 &
    job=$!
    for _ in {1..100}; do
        grep -qs -- '--user stop' "$H/log/systemctl.log" && break
        sleep 0.1
    done
    if pid="$(pgrep -u "$U" -f '^bash ./install.sh' | head -1)" && [[ -n $pid ]]; then
        pkill -INT -s "$(ps -o sid= -p "$pid" | tr -d ' ')" || true
    else
        echo "install.sh не дошёл до остановки службы" >>"$out"
    fi
    if wait "$job"; then code=0; else code=$?; fi
    sleep 1
    report "$1" "$2" "$code" "$out"
    rm -f "$out"
}

# ─── Сценарии ───────────────────────────────────────────────────────────────
# scenario ИМЯ КОД КОМАНДА: КОД — какой код выхода ожидается. Если у какого-то
# сценария код другой, run.sh в конце назовёт их и выйдет с кодом 1 (так падает CI).
unexpected=()

scenario 01-install 0 './install.sh'
scenario 02-same 0 './install.sh'
as_user 'systemctl --user start shiftfix.service'
fake_installed_version 0.3.0
scenario 03-update 0 './install.sh'
# Сборка падает: код 1, установленная версия не тронута; следующий запуск обновляет.
fake_installed_version 0.3.0
as_user 'touch ~/.fakesd/cargo-fail'
scenario 04-build-fails 1 './install.sh'
as_user 'rm ~/.fakesd/cargo-fail'
scenario 05-update-after-fail 0 './install.sh'
# Установлена более новая версия: без терминала спросить некого — код 1; ответ «нет» —
# ничего не меняется; --yes — ставится.
fake_installed_version 99.0.0
scenario 06-older-no-terminal 1 './install.sh'
scenario 07-older-answer-no 0 "printf 'n\n' | script -qec ./install.sh /dev/null"
scenario 08-older-yes 0 './install.sh --yes'
as_user 'systemctl --user disable shiftfix.service'
fake_installed_version 0.3.0
scenario 09-autostart-disabled 0 './install.sh'
as_user 'systemctl --user enable shiftfix.service'
as_user "rm ~/.local/bin/shiftfix && ln -s $WORK/build/shiftfix ~/.local/bin/shiftfix"
scenario 10-symlink 0 './install.sh'

# Ctrl+C, пока служба останавливается (медленно), а вывод идёт через tee: служба
# должна подняться снова.
as_user 'systemctl --user start shiftfix.service; touch ~/.fakesd/slow-stop'
fake_installed_version 0.3.0
interrupted_stop 11-interrupted-stop 130
as_user 'rm -f ~/.fakesd/slow-stop'

scenario 12-repair-after-interrupt 0 './install.sh'
as_user 'touch ~/.fakesd/off'
scenario 13-no-systemd-unit-kept 0 './install.sh'
as_user "./uninstall.sh --yes --keep-group >/dev/null 2>&1; rm -f ~/.config/systemd/user/shiftfix.service"
scenario 14-no-systemd-install 0 './install.sh'
as_user 'rm ~/.fakesd/off'
scenario 15-uninstall-answer-no 0 "printf 'n\n' | script -qec ./uninstall.sh /dev/null"
scenario 16-uninstall 0 './uninstall.sh --yes'
scenario 17-uninstall-again 0 './uninstall.sh --yes'
scenario 18-install-again 0 './install.sh --yes'
scenario 19-uninstall-purge 0 './uninstall.sh --yes --purge'
# Пока install.sh держит блокировку (идёт в другом терминале), uninstall.sh ничего не
# трогает и выходит с кодом 1.
scenario 20-uninstall-while-locked 1 "flock $RUN/shiftfix-install.lock ./uninstall.sh --yes"

# Первая установка обрывается после замены файлов (daemon-reload падает, код 1):
# повторный запуск доделывает её — включает автозапуск и запускает службу, а
# uninstall.sh убирает и метку незаконченной установки.
as_user 'touch ~/.fakesd/reload-fail'
scenario 21-first-install-interrupted 1 './install.sh'
scenario 22-uninstall-interrupted 0 './uninstall.sh --yes --keep-group'
as_user './install.sh >/dev/null 2>&1; rm ~/.fakesd/reload-fail ~/log/*.log' # снова обрыв, без журнала
scenario 23-first-install-resumed 0 './install.sh'

# Юнит выключен, а служба запускается из автозапуска сеанса (его записал сеанс без
# systemd): install.sh этот файл не трогает.
as_user 'systemctl --user disable shiftfix.service && mkdir -p ~/.config/autostart &&
    ~/.local/bin/shiftfix --print-autostart >~/.config/autostart/shiftfix.desktop'
scenario 24-unit-off-session-autostart 0 './install.sh'

# Домашний каталог с пробелом: путь в Exec= ярлыка — в кавычках, установка и
# удаление не спотыкаются. Журналы и состояние systemctl — общие с основным.
SPACE_HOME="$H/my home"
as_user "systemctl --user stop shiftfix.service && mkdir '$SPACE_HOME' && ln -s ~/log ~/.fakesd '$SPACE_HOME/'"
scenario 25-home-with-space 0 "HOME='$SPACE_HOME' ./install.sh &&
    grep '^Exec=' '$SPACE_HOME/.local/share/applications/shiftfix.desktop'"
scenario 26-home-with-space-uninstall 0 "HOME='$SPACE_HOME' ./uninstall.sh --yes --keep-group"

# Та же версия, но правила udev нет (его убрало удаление выше): ставится только оно.
scenario 27-same-system-repaired 0 './install.sh'

# Метка незаконченной установки лежит, а блокировку держит install.sh в другом
# терминале (метку положил он): второй только просит подождать (код 1), не «запустите
# снова».
as_user 'mkdir -p ~/.local/state && touch ~/.local/state/shiftfix-install-incomplete'
scenario 28-install-while-locked 1 "flock $RUN/shiftfix-install.lock ./install.sh"
as_user 'rm ~/.local/state/shiftfix-install-incomplete'

# Первая установка (программ в ~/.local/bin нет), а юнит уже есть и выключен — его
# записало окно, потом автозапуск выключили: install.sh выбор не трогает.
as_user 'rm ~/.local/bin/shiftfix ~/.local/bin/shiftfix-gui ~/.config/autostart/shiftfix.desktop'
scenario 29-first-install-unit-off 0 './install.sh'

# Доделка оборванной установки, а цели включённого юнита сменились: обрыв был вне
# графического сеанса (WantedBy с default.target). Ссылка из default.target.wants
# убирается, как при обновлении.
as_user 'touch ~/.fakesd/no-graphical && ~/.local/bin/shiftfix --print-unit >~/.config/systemd/user/shiftfix.service &&
    rm ~/.fakesd/no-graphical && systemctl --user enable shiftfix.service &&
    touch ~/.local/state/shiftfix-install-incomplete && rm ~/log/*.log'
scenario 30-resumed-wanted-changed 0 './install.sh'

# Обновление вне графического сеанса (ssh): новый юнит хотел бы WantedBy и с
# default.target, но прежние цели вернее — юнит и ссылки остаются как были.
as_user 'touch ~/.fakesd/no-graphical'
fake_installed_version 0.3.0
scenario 31-update-no-graphical 0 './install.sh'
as_user 'rm ~/.fakesd/no-graphical'

# Без systemd служба — процесс из автозапуска сеанса: обновление останавливает его и
# запускает новый тоже процессом.
as_user './uninstall.sh --yes --keep-group >/dev/null 2>&1; touch ~/.fakesd/off &&
    ./install.sh >/dev/null 2>&1; rm -f ~/log/*.log'
fake_installed_version 0.3.0
scenario 32-no-systemd-update-process 0 './install.sh'

# systemd снова есть, а служба — всё тот же процесс из автозапуска сеанса: обновление
# записывает и включает юнит, убирает файл автозапуска сеанса и запускает службу уже
# через systemd.
as_user 'rm ~/.fakesd/off'
fake_installed_version 0.3.0
scenario 33-session-autostart-to-unit 0 './install.sh'

# Служба упала (is-failed), программы те же: install.sh её не запускает, но в итоге
# говорит, что она остановилась с ошибкой.
as_user 'systemctl --user stop shiftfix.service && touch ~/.fakesd/failed'
scenario 34-same-service-failed 0 './install.sh'

# Нет sudo (пользователь не из sudoers): uninstall.sh удаляет файлы в домашнем
# каталоге, а для системных настроек (правило udev, modules-load, группа input)
# печатает команды для root.
as_user 'touch ~/.fakesd/no-sudo'
scenario 35-uninstall-no-sudo 0 './uninstall.sh --yes'
# Команды для root выполнены, кроме вывода из группы. Правила udev нет, sudo нет:
# install.sh печатает, что выполнить от root, и выходит с кодом 1, ничего не поставив.
rm -f /etc/udev/rules.d/80-shiftfix-uinput.rules /etc/modules-load.d/shiftfix-uinput.conf
scenario 36-install-no-sudo 1 './install.sh'
# sudo снова есть: install.sh ставит правило udev и загрузку модуля (группа уже есть).
as_user 'rm ~/.fakesd/no-sudo'
scenario 37-install-with-sudo 0 './install.sh'

# install.sh --uninstall — это uninstall.sh с теми же параметрами; --keep-build
# оставляет сборку и при --purge.
scenario 38-uninstall-via-install-keep-build 0 './install.sh --uninstall --yes --purge --keep-build &&
    ls target/release'

if ((${#unexpected[@]})); then
    echo "Код выхода не тот, что ожидался: ${unexpected[*]}" >&2
    exit 1
fi

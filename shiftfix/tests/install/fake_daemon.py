# Поддельная служба shiftfix: блокировка и сокет как у настоящей, ответ на status и quit.
# Аргументы: XDG_RUNTIME_DIR, версия. Запускать копией python с именем shiftfix,
# чтобы pgrep -x shiftfix её находил.
import fcntl, json, os, signal, socket, sys
run, version = sys.argv[1], sys.argv[2]
lock_path = os.path.join(run, "shiftfix.lock"); lock = open(lock_path, "w")
fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
path = os.path.join(run, "shiftfix.sock")
def leave(*_):
    for f in (path, lock_path):
        try: os.unlink(f)
        except FileNotFoundError: pass
    sys.exit(0)
signal.signal(signal.SIGTERM, leave)
try: os.unlink(path)
except FileNotFoundError: pass
srv = socket.socket(socket.AF_UNIX); srv.bind(path); srv.listen(5)
status = {"version": version, "pid": 4242, "uptime_secs": 5, "enabled": True,
  "layouts": {"backend": "KDE", "codes": ["us","ru"], "names": ["English (US)","Russian"],
              "current": 0, "from_keymap": True, "error": None},
  "selection": True, "selection_note": None, "selection_via": "wl-clipboard",
  "tray": True, "tray_waiting": False,
  "devices": [
    {"path": "/dev/input/event3", "name": "AT Translated Set 2 keyboard", "key": "0001:0001 AT Translated Set 2 keyboard",
     "detected": "keyboard", "role": "keyboard", "overridden": False, "virtual_device": False, "idle_ms": None},
    {"path": "/dev/input/event5", "name": "Logitech USB Mouse", "key": "046d:c077 Logitech USB Mouse",
     "detected": "mouse", "role": "mouse", "overridden": False, "virtual_device": False, "idle_ms": None},
    {"path": "/dev/input/event7", "name": "SynPS/2 Touchpad", "key": "0002:0007 SynPS/2 Touchpad",
     "detected": "touchpad", "role": "ignore", "overridden": True, "virtual_device": False, "idle_ms": None}],
  "actions": [
    {"time": 1760000000, "kind": "word", "chars": 6, "from": "us", "to": "ru"},
    {"time": 1760000060, "kind": "selection", "chars": 10, "from": "us", "to": "ru"},
    {"time": 1760000065, "kind": "undo", "chars": 10, "from": "ru", "to": "us"}],
  "log": [
    {"time": 1760000000, "level": "INFO", "text": "готов: раскладки us, ru (KDE), выделение исправляется"},
    {"time": 1760000001, "level": "WARN", "text": "пример предупреждения"},
    {"time": 1760000002, "level": "ERROR", "text": "пример ошибки"}],
  "config_path": os.path.join(os.environ.get("XDG_CONFIG_HOME", ""), "shiftfix/config.toml")}
while True:
    c, _ = srv.accept(); f = c.makefile("rwb")
    try: req = json.loads(f.readline())
    except Exception: req = {}
    resp = {"ok": True}
    if req.get("cmd") == "status": resp["status"] = status
    f.write((json.dumps(resp, ensure_ascii=False) + "\n").encode()); f.flush(); c.close()
    if req.get("cmd") == "quit": leave()

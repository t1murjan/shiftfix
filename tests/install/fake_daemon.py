# Поддельная служба shiftfix: блокировка и сокет как у настоящей, ответ на status и quit.
# Аргументы: XDG_RUNTIME_DIR, версия. Запускать копией python с именем shiftfix,
# чтобы pgrep -x shiftfix её находил. Статус — из fake_status.json рядом (в формате
# ipc::Status); version и config_path в нём подставляются здесь.
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
with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "fake_status.json"), encoding="utf-8") as f:
    status = json.load(f)
config_home = os.environ.get("XDG_CONFIG_HOME") or os.path.expanduser("~/.config")
status.update(version=version, config_path=os.path.join(config_home, "shiftfix/config.toml"))
while True:
    c, _ = srv.accept(); f = c.makefile("rwb")
    try: req = json.loads(f.readline())
    except Exception: req = {}
    resp = {"ok": True}
    if req.get("cmd") == "status": resp["status"] = status
    try: f.write((json.dumps(resp, ensure_ascii=False) + "\n").encode()); f.flush()
    except OSError: pass  # клиент не дождался ответа и закрыл соединение
    c.close()
    if req.get("cmd") == "quit": leave()

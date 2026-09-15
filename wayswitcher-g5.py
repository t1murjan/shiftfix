import evdev
import time
import subprocess
import threading
import sys
import os
import uuid
import tkinter as tk
from tkinter import ttk, messagebox
try:
    import pystray
    from PIL import Image, ImageDraw
except ImportError:
    pystray = None

# --- ГЛОБАЛЬНЫЕ НАСТРОЙКИ ---
CONFIG_DIR = os.path.expanduser("~/.config/wayland_switcher")
CONFIG_FILE = os.path.join(CONFIG_DIR, "config.txt")
THEME_FILE = os.path.join(CONFIG_DIR, "theme.txt")
LAYOUT_FILE = os.path.join(CONFIG_DIR, "layout_shortcut.txt")
STOP_FILE = "/tmp/wayland_switcher.run"

# Варианты шортката переключения раскладки: имя → список кодов клавиш
# Первый элемент списка — модификаторы, последний — основная клавиша
LAYOUT_SHORTCUTS = {
    "Alt + Shift":   ["KEY_LEFTALT",   "KEY_LEFTSHIFT"],
    "Ctrl + Shift":  ["KEY_LEFTCTRL",  "KEY_LEFTSHIFT"],
    "Super + Space": ["KEY_LEFTMETA",  "KEY_SPACE"],
    "Отключено":     [],
}

SHIFT_KEYS = [evdev.ecodes.KEY_LEFTSHIFT, evdev.ecodes.KEY_RIGHTSHIFT]

EN = "qwertyuiop[]asdfghjkl;'zxcvbnm,./`"
RU = "йцукенгшщзхъфывапролджэячсмитьбю.ё"
EN_UPPER = "QWERTYUIOP{}ASDFGHJKL:\"ZXCVBNM<>/~"
RU_UPPER = "ЙЦУКЕНГШЩЗХЪФЫВАПРОЛДЖЭЯЧСМИТЬБЮ,Ё"

MAP_EN_TO_RU = dict(zip(EN + EN_UPPER, RU + RU_UPPER))
MAP_RU_TO_EN = dict(zip(RU + RU_UPPER, EN + EN_UPPER))

# FIX 1: Используем Lock вместо bool — атомарная защита от race condition
_processing_lock = threading.Lock()

# ==========================================
#       ЧАСТЬ 1: ФОНОВЫЙ ДЕМОН (ROOT)
# ==========================================

def watch_stop_signal():
    """Следит за удалением файла-флага. Если файл исчез - завершает процесс."""
    while True:
        if not os.path.exists(STOP_FILE):
            print("Сигнал остановки получен. Завершение работы...")
            os._exit(0)
        time.sleep(1)

def convert_text(text):
    """
    Определяем направление по первому значимому символу.
    Возвращает кортеж (конвертированный_текст, язык_результата).
    язык_результата: 'RU', 'EN' или None если конвертация не нужна.
    """
    if not text.strip():
        return text, None
    first = next(
        (c for c in text if c in MAP_RU_TO_EN or c in MAP_EN_TO_RU),
        None
    )
    if first is None:
        return text, None
    if first in MAP_RU_TO_EN:
        # RU → EN
        return ''.join(MAP_RU_TO_EN.get(c, c) for c in text), 'EN'
    else:
        # EN → RU
        return ''.join(MAP_EN_TO_RU.get(c, c) for c in text), 'RU'

def get_clipboard():
    try:
        return subprocess.check_output(
            ['wl-paste', '--no-newline'], text=True, env=os.environ,
            stderr=subprocess.DEVNULL, timeout=1
        )
    except Exception:
        return ""

def set_clipboard(text):
    try:
        subprocess.run(
            ['wl-copy'], input=text, text=True, check=True,
            env=os.environ, stderr=subprocess.DEVNULL, timeout=1
        )
        return True
    except Exception as e:
        print(f"Ошибка буфера: {e}")
        return False

def simulate_key(ui, key_code, state):
    ui.write(evdev.ecodes.EV_KEY, key_code, state)
    ui.syn()

def tap_key(ui, key_code):
    simulate_key(ui, key_code, 1)
    time.sleep(0.02)
    simulate_key(ui, key_code, 0)

def switch_layout(ui, key_names):
    """
    Симулирует шорткат переключения раскладки через uinput.
    key_names — список строк вида ['KEY_LEFTALT', 'KEY_LEFTSHIFT'].
    Нажимает все клавиши по порядку, потом отпускает в обратном.
    """
    if not key_names:
        return
    codes = []
    for name in key_names:
        code = getattr(evdev.ecodes, name, None)
        if code is None:
            print(f"Неизвестный код клавиши: {name}")
            return
        codes.append(code)

    # Нажимаем все клавиши
    for code in codes:
        simulate_key(ui, code, 1)
        time.sleep(0.02)
    # Отпускаем в обратном порядке
    for code in reversed(codes):
        simulate_key(ui, code, 0)
        time.sleep(0.02)

def execute_replacement(ui, layout_key_names):
    # FIX 1: non-blocking acquire — если уже выполняется, молча выходим
    if not _processing_lock.acquire(blocking=False):
        return
    try:
        old_clipboard = get_clipboard()
        # Маркер 1: проверяем, выделен ли уже текст
        marker1 = "W_SW_" + str(uuid.uuid4())
        if not set_clipboard(marker1):
            return
        time.sleep(0.05)

        simulate_key(ui, evdev.ecodes.KEY_LEFTCTRL, 1)
        tap_key(ui, evdev.ecodes.KEY_C)
        simulate_key(ui, evdev.ecodes.KEY_LEFTCTRL, 0)
        time.sleep(0.15)

        clip1 = get_clipboard()
        text_to_convert = ""

        if clip1 == marker1:
            # Текст не был выделен — пытаемся выделить последнее слово
            simulate_key(ui, evdev.ecodes.KEY_LEFTCTRL, 1)
            simulate_key(ui, evdev.ecodes.KEY_LEFTSHIFT, 1)
            tap_key(ui, evdev.ecodes.KEY_LEFT)
            simulate_key(ui, evdev.ecodes.KEY_LEFTSHIFT, 0)
            simulate_key(ui, evdev.ecodes.KEY_LEFTCTRL, 0)
            time.sleep(0.1)

            marker2 = "W_SW_" + str(uuid.uuid4())
            if not set_clipboard(marker2):
                return
            time.sleep(0.05)

            simulate_key(ui, evdev.ecodes.KEY_LEFTCTRL, 1)
            tap_key(ui, evdev.ecodes.KEY_C)
            simulate_key(ui, evdev.ecodes.KEY_LEFTCTRL, 0)
            time.sleep(0.15)

            clip2 = get_clipboard()
            if clip2 != marker2 and clip2:
                text_to_convert = clip2
        else:
            if clip1:
                text_to_convert = clip1

        if text_to_convert:
            new_text, target_lang = convert_text(text_to_convert)

            if new_text != text_to_convert:
                if not set_clipboard(new_text):
                    return
                time.sleep(0.1)

                simulate_key(ui, evdev.ecodes.KEY_LEFTCTRL, 1)
                tap_key(ui, evdev.ecodes.KEY_V)
                simulate_key(ui, evdev.ecodes.KEY_LEFTCTRL, 0)
                time.sleep(0.1)

                # Переключаем раскладку после вставки чтобы
                # продолжать писать на нужном языке
                switch_layout(ui, layout_key_names)

        # Восстанавливаем буфер, включая случай, когда конвертация не нужна.
        set_clipboard(old_clipboard)

    finally:
        _processing_lock.release()

def daemon_main(timeout_val, layout_key_names):
    if not os.environ.get("WAYLAND_DISPLAY"):
        print("Ошибка: WAYLAND_DISPLAY не установлен в демоне.")
        sys.exit(1)

    threading.Thread(target=watch_stop_signal, daemon=True).start()

    devices = [evdev.InputDevice(path) for path in evdev.list_devices()]
    keyboard = None
    for device in devices:
        capabilities = device.capabilities().get(evdev.ecodes.EV_KEY, [])
        if evdev.ecodes.KEY_ESC in capabilities and evdev.ecodes.KEY_LEFTSHIFT in capabilities:
            keyboard = device
            break

    if not keyboard:
        print("Ошибка: клавиатура не найдена.")
        sys.exit(1)

    ui = evdev.UInput()
    last_shift_time = 0
    # FIX 4: отслеживаем состояние Shift — чтобы не сбрасывать таймер
    # при Shift+стрелка (выделение текста)
    shift_held = False

    try:
        # FIX 2: эксклюзивный захват устройства — события не дублируются
        keyboard.grab()
        for event in keyboard.read_loop():
            if event.type != evdev.ecodes.EV_KEY:
                continue

            is_shift = event.code in SHIFT_KEYS

            if is_shift:
                if event.value == 1:  # нажат
                    shift_held = True
                    current_time = time.time()
                    if current_time - last_shift_time < timeout_val:
                        threading.Thread(
                            target=execute_replacement, args=(ui, layout_key_names)
                        ).start()
                        last_shift_time = 0
                    else:
                        last_shift_time = current_time
                elif event.value == 0:  # отпущен
                    shift_held = False
            else:
                # FIX 4: сбрасываем таймер только если Shift не удерживается
                # (чтобы Shift+стрелка не ломала double-shift)
                if event.value == 1 and not shift_held:
                    last_shift_time = 0

            # Пробрасываем все события дальше (grab перехватил их от системы)
            ui.write(event.type, event.code, event.value)
            ui.syn()

    except KeyboardInterrupt:
        pass
    finally:
        try:
            keyboard.ungrab()
        except Exception:
            pass
        ui.close()

# ==========================================
#    ЧАСТЬ 2: ГРАФИЧЕСКИЙ ИНТЕРФЕЙС (GUI)
# ==========================================

# Постоянная палитра в стиле терминала.
THEME = {
    "bg": "#202124", "panel": "#292a2d", "fg": "#e0b15a",
    "muted": "#a8864b", "accent": "#f0c66a", "accent_hover": "#ffd98a",
    "accent_fg": "#202124", "border": "#5b4a2c", "entry_bg": "#17181a",
    "status_run": "#d9b45f", "status_stop": "#c96b5b", "button_bg": "#38352d",
}
MONO_FONT = "TkFixedFont"

class SwitcherApp:
    def __init__(self, root):
        self.root = root
        self.root.title("Wayland Switcher")
        self.root.geometry("430x295")
        self.root.resizable(False, False)

        self.timeout = self.load_config()
        self.layout_shortcut = self.load_layout_shortcut()
        self.is_running = False
        self._proc = None
        self._tray = None

        if os.path.exists(STOP_FILE):
            os.remove(STOP_FILE)

        self.build_ui()
        self._tray = None
        self._init_tray()
        self.apply_theme()
        self.check_status_loop()

    # ---- Конфигурация ----

    def load_layout_shortcut(self):
        if os.path.exists(LAYOUT_FILE):
            try:
                with open(LAYOUT_FILE) as f:
                    val = f.read().strip()
                    if val in LAYOUT_SHORTCUTS:
                        return val
            except Exception:
                pass
        return "Alt + Shift"

    def save_layout_shortcut(self, name):
        os.makedirs(CONFIG_DIR, exist_ok=True)
        with open(LAYOUT_FILE, 'w') as f:
            f.write(name)

    def apply_theme(self):
        t = THEME
        self.root.configure(bg=t["bg"])
        for frame in (self.main_frame, self.bottom_frame):
            frame.configure(bg=t["bg"])
        for frame in (self.card_frame, self.timeout_frame, self.layout_frame):
            frame.configure(bg=t["panel"])
        self.title_lbl.configure(bg=t["bg"], fg=t["accent"])
        self.status_label.configure(bg=t["panel"], fg=self._get_status_color(t))
        self.timeout_lbl.configure(bg=t["panel"], fg=t["muted"])
        self.layout_lbl.configure(bg=t["panel"], fg=t["muted"])
        self.card_frame.configure(bg=t["panel"], highlightbackground=t["border"], highlightthickness=1)
        self.toggle_btn.configure(bg=t["accent"], fg=t["accent_fg"], activebackground=t["accent_hover"], activeforeground=t["accent_fg"])
        self.save_btn.configure(bg=t["button_bg"], fg=t["fg"], activebackground=t["border"], activeforeground=t["fg"])
        self.minimize_btn.configure(bg=t["bg"], fg=t["muted"], activebackground=t["bg"], activeforeground=t["accent"])
        self.timeout_entry.configure(bg=t["entry_bg"], fg=t["fg"], insertbackground=t["fg"], highlightbackground=t["border"], highlightcolor=t["accent"])
        self.layout_menu.configure(bg=t["entry_bg"], fg=t["fg"], activebackground=t["accent"], activeforeground=t["accent_fg"], highlightbackground=t["border"])
        self.hint_lbl.configure(bg=t["bg"], fg=t["muted"])

    def _get_status_color(self, t=None):
        t = THEME if t is None else t
        return t["status_run"] if self.is_running else t["status_stop"]

    # ---- UI ----

    def build_ui(self):
        t = THEME

        self.main_frame = tk.Frame(self.root)
        self.main_frame.pack(fill="both", expand=True, padx=20, pady=16)

        # Заголовок + кнопка темы
        header = tk.Frame(self.main_frame)
        header.pack(fill="x", pady=(0, 12))
        header.configure(bg=t["bg"])

        self.title_lbl = tk.Label(
            header, text="Wayland Switcher",
            font=(MONO_FONT, 15, "bold")
        )
        self.title_lbl.pack(side="left")

        self.minimize_btn = tk.Button(header, text="_", command=self.minimize_to_tray, font=(MONO_FONT, 12, "bold"), relief="flat", bd=0, cursor="hand2")
        self.minimize_btn.pack(side="right")

        # Карточка статуса
        self.card_frame = tk.Frame(
            self.main_frame, relief="flat",
            highlightthickness=1, bd=0
        )
        self.card_frame.pack(fill="x", pady=(0, 12))

        inner = tk.Frame(self.card_frame)
        inner.pack(fill="x", padx=14, pady=10)
        inner.configure(bg=t["panel"])

        dot_text = "● Работает" if self.is_running else "● Остановлен"
        self.status_var = tk.StringVar(value=dot_text)
        self.status_label = tk.Label(
            inner, textvariable=self.status_var,
            font=(MONO_FONT, 12, "bold"), bg=t["panel"]
        )
        self.status_label.pack(side="left")

        self.toggle_btn = tk.Button(
            inner, text="Запустить",
            font=(MONO_FONT, 10, "bold"),
            relief="raised", cursor="hand2",
            padx=12, pady=4, bd=0,
            command=self.toggle_service
        )
        self.toggle_btn.pack(side="right")

        # Таймаут
        self.timeout_frame = tk.Frame(self.main_frame)
        self.timeout_frame.pack(fill="x", pady=(0, 8))
        self.timeout_frame.configure(bg=t["panel"])

        self.timeout_lbl = tk.Label(
            self.timeout_frame,
            text="Таймаут двойного Shift (сек):",
            font=(MONO_FONT, 10)
        )
        self.timeout_lbl.pack(side="left")

        self.timeout_var = tk.StringVar(value=str(self.timeout))
        self.timeout_entry = tk.Entry(
            self.timeout_frame, textvariable=self.timeout_var,
            width=5, font=(MONO_FONT, 10),
            relief="solid", bd=1, highlightthickness=1
        )
        self.timeout_entry.pack(side="left", padx=(8, 6))

        self.save_btn = tk.Button(
            self.timeout_frame, text="Сохранить",
            font=(MONO_FONT, 10),
            relief="raised", cursor="hand2",
            padx=8, pady=2, bd=0,
            command=self.save_config
        )
        self.save_btn.pack(side="left")

        # Переключение раскладки
        self.layout_frame = tk.Frame(self.main_frame)
        self.layout_frame.pack(fill="x", pady=(0, 8))
        self.layout_frame.configure(bg=t["panel"])

        self.layout_lbl = tk.Label(
            self.layout_frame,
            text="Переключение раскладки:",
            font=(MONO_FONT, 10)
        )
        self.layout_lbl.pack(side="left")

        self.layout_var = tk.StringVar(value=self.layout_shortcut)
        self.layout_menu = tk.OptionMenu(
            self.layout_frame, self.layout_var,
            *LAYOUT_SHORTCUTS.keys(),
            command=self._on_layout_change
        )
        self.layout_menu.configure(
            font=(MONO_FONT, 10), relief="flat",
            cursor="hand2", bd=0, padx=6, pady=2
        )
        self.layout_menu.pack(side="left", padx=(8, 0))

        # Подпись внизу
        self.bottom_frame = tk.Frame(self.main_frame)
        self.bottom_frame.pack(fill="x", side="bottom")
        self.bottom_frame.configure(bg=t["bg"])

        self.hint_lbl = tk.Label(
            self.bottom_frame,
            text="Двойной Shift — конвертация последнего слова",
            font=(MONO_FONT, 8), fg="#888"
        )
        self.hint_lbl.pack()
        self.hint_lbl.configure(bg=t["bg"])

    # ---- Сервис ----

    def _on_layout_change(self, choice):
        self.layout_shortcut = choice
        self.save_layout_shortcut(choice)

    def toggle_service(self):
        if not self.is_running:
            self.start_service()
        else:
            self.stop_service()

    def start_service(self):
        with open(STOP_FILE, 'w') as f:
            f.write("run")

        wayland_disp = os.environ.get("WAYLAND_DISPLAY", "wayland-0")
        xdg_runtime = os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}")

        # Передаём список клавиш шортката через аргументы (через запятую)
        key_names = LAYOUT_SHORTCUTS.get(self.layout_shortcut, [])
        layout_arg = ",".join(key_names) if key_names else "none"

        cmd = [
            "pkexec", "env",
            f"WAYLAND_DISPLAY={wayland_disp}",
            f"XDG_RUNTIME_DIR={xdg_runtime}",
            sys.executable,
            os.path.abspath(__file__),
            "--daemon",
            str(self.timeout),
            layout_arg,
        ]

        try:
            # FIX 5: сохраняем процесс для последующего poll()
            self._proc = subprocess.Popen(
                cmd, stderr=subprocess.DEVNULL, stdout=subprocess.DEVNULL
            )
            # Даём процессу 600мс на старт перед тем как считать его живым
            self.root.after(600, self._confirm_proc_alive)
        except Exception as e:
            if os.path.exists(STOP_FILE):
                os.remove(STOP_FILE)
            messagebox.showerror("Ошибка", f"Не удалось запустить:\n{e}")

    def _confirm_proc_alive(self):
        """Проверяем что pkexec/демон реально запустился, а не упал мгновенно."""
        if self._proc and self._proc.poll() is None:
            self.is_running = True
            self.update_ui_state()
        else:
            # Процесс уже завершился — пользователь отклонил pkexec или ошибка
            if os.path.exists(STOP_FILE):
                os.remove(STOP_FILE)
            self._proc = None
            self.is_running = False
            self.update_ui_state()

    def stop_service(self):
        if os.path.exists(STOP_FILE):
            os.remove(STOP_FILE)
        self._proc = None
        self.is_running = False
        self.update_ui_state()

    def update_ui_state(self):
        t = THEME
        if self.is_running:
            self.status_var.set("● Работает")
            self.toggle_btn.config(text="Остановить")
        else:
            self.status_var.set("● Остановлен")
            self.toggle_btn.config(text="Запустить")
        self.status_label.configure(fg=self._get_status_color(t))

    def check_status_loop(self):
        """
        FIX 5: проверяем poll() реального процесса, а не только файл.
        Ловим краш демона даже если файл ещё не успел исчезнуть.
        """
        if self.is_running:
            proc_dead = self._proc is not None and self._proc.poll() is not None
            file_gone = not os.path.exists(STOP_FILE)
            if proc_dead or file_gone:
                self.is_running = False
                self._proc = None
                self.update_ui_state()
        self.root.after(1000, self.check_status_loop)

    # ---- Конфиг ----

    def load_config(self):
        if os.path.exists(CONFIG_FILE):
            try:
                with open(CONFIG_FILE) as f:
                    return float(f.read().strip())
            except Exception:
                pass
        return 0.4

    def save_config(self):
        try:
            val = float(self.timeout_var.get())
            if val < 0.1 or val > 2:
                raise ValueError
            self.timeout = val
            os.makedirs(CONFIG_DIR, exist_ok=True)
            with open(CONFIG_FILE, 'w') as f:
                f.write(str(val))
            messagebox.showinfo("Успех", "Настройки сохранены.\nПерезапустите сервис, чтобы применить.")
        except ValueError:
            messagebox.showwarning("Ошибка", "Введите корректное число от 0.1 до 2.0 (например, 0.4)")

    def on_closing(self):
        self.stop_service()
        if self._tray:
            self._tray.stop()
        self.root.destroy()

    def minimize_to_tray(self):
        # withdraw removes the window from the taskbar while keeping Tk alive.
        if self._tray is not None:
            self.root.withdraw()
        else:
            self.root.iconify()

    def _init_tray(self):
        """Initialize tray support when optional pystray is installed."""
        if pystray is None:
            return
        image = Image.new("RGB", (32, 32), THEME["accent"])
        menu = pystray.Menu(
            pystray.MenuItem("Показать", lambda: self.root.after(0, self.restore_from_tray)),
            pystray.MenuItem("Выход", lambda: self.root.after(0, self.on_closing)),
        )
        self._tray = pystray.Icon("wayswitcher", image, "Wayland Switcher", menu)
        threading.Thread(target=self._tray.run, daemon=True).start()

    def restore_from_tray(self):
        self.root.deiconify()
        self.root.state("normal")
        self.root.lift()
        self.root.focus_force()


def main():
    if len(sys.argv) >= 3 and sys.argv[1] == "--daemon":
        try:
            timeout_val = float(sys.argv[2])
            if not 0.1 <= timeout_val <= 2:
                raise ValueError
        except ValueError:
            timeout_val = 0.4
        # Парсим список клавиш раскладки из argv[3]
        if len(sys.argv) >= 4 and sys.argv[3] != "none":
            layout_key_names = sys.argv[3].split(",")
        else:
            layout_key_names = []
        daemon_main(timeout_val, layout_key_names)
    else:
        try:
            root = tk.Tk()
        except tk.TclError as exc:
            print(
                "Не удалось открыть GUI: нет доступа к графическому дисплею. "
                "Запустите программу внутри Wayland/X11-сеанса.",
                file=sys.stderr,
            )
            sys.exit(1)
        app = SwitcherApp(root)
        root.protocol("WM_DELETE_WINDOW", app.on_closing)
        root.mainloop()


if __name__ == "__main__":
    main()

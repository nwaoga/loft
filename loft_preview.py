"""Interactive, offline notch preview. Run with python loft_preview.py."""
import argparse
import json
import re
import sqlite3
import sys
import time
import queue
import threading
import subprocess
import traceback
from pathlib import Path
import tkinter as tk
from tkinter import scrolledtext, filedialog, messagebox

from loft_bench import ROOT, FakeEmbedding, Index
from loft_engine import CODE, format_dropped
from loft_theme import DEFAULT, windows_palette

BG = "#0d0e12"
PANEL = "#181a20"
TEXT = "#f4f5f7"
MUTED = "#8b919c"
ACCENT = "#4f8cff"
GLOW = "#1e4a8c"
GLOW_SOFT = "#2a5080"
HOVER = "#2a2d35"
ERROR = "#f0b4a8"
SUCCESS = "#3ecf8e"
CARD = "#1e2128"
BUBBLE_ASSIST = "#22252c"
BUBBLE_RADIUS = 14
BUBBLE_WRAP = 340
CONVERSATION_HEIGHT = 620
STOP = set("a an the i my have has is are was were for to of in on does do where when which files involved taking today me find paid".split())
CODE_WORDS = set("def class return if elif else for while import from as try except with lambda pass break continue and or not in True False None python powershell pwsh".split())


def monitors():
    """Work areas of attached screens. The primary screen is first."""
    if sys.platform != "win32":
        return [{"primary": True, "left": 0, "top": 0, "right": 1920, "bottom": 1080}]
    import ctypes
    class RECT(ctypes.Structure):
        _fields_ = [("left", ctypes.c_long), ("top", ctypes.c_long), ("right", ctypes.c_long), ("bottom", ctypes.c_long)]
    class MONITORINFO(ctypes.Structure):
        _fields_ = [("cbSize", ctypes.c_ulong), ("rcMonitor", RECT), ("rcWork", RECT), ("dwFlags", ctypes.c_ulong)]
    found = []
    def collect(monitor, hdc, rect, data):
        info = MONITORINFO()
        info.cbSize = ctypes.sizeof(MONITORINFO)
        ctypes.windll.user32.GetMonitorInfoW(monitor, ctypes.byref(info))
        area = info.rcWork
        found.append({"primary": bool(info.dwFlags & 1), "left": area.left, "top": area.top,
                      "right": area.right, "bottom": area.bottom})
        return 1
    callback = ctypes.WINFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p, ctypes.POINTER(RECT), ctypes.c_ssize_t)
    ctypes.windll.user32.EnumDisplayMonitors(0, 0, callback(collect), 0)
    return sorted(found, key=lambda screen: (not screen["primary"], screen["left"], screen["top"])) or [
        {"primary": True, "left": 0, "top": 0, "right": 1920, "bottom": 1080}]


def virtual_bounds(screens):
    return (min(s["left"] for s in screens), min(s["top"] for s in screens),
            max(s["right"] for s in screens), max(s["bottom"] for s in screens))


def on_screen(x, y, screens):
    return any(screen["left"] <= x < screen["right"] and screen["top"] <= y < screen["bottom"] for screen in screens)


def window_handle(widget):
    import ctypes
    child = widget.winfo_id()
    parent = ctypes.windll.user32.GetParent(child)
    return parent or child


def apply_window_material(widget, width, height, glass, opacity):
    """Rounded window. Glass tints the wallpaper through the panel on Windows."""
    if sys.platform != "win32" or width < 2 or height < 2:
        return
    import ctypes
    widget.attributes("-alpha", opacity if glass else 1)
    hwnd = window_handle(widget)
    class ACCENTPOLICY(ctypes.Structure):
        _fields_ = [("AccentState", ctypes.c_uint), ("AccentFlags", ctypes.c_uint),
                    ("GradientColor", ctypes.c_uint), ("AnimationId", ctypes.c_uint)]
    class WINCOMPATTRDATA(ctypes.Structure):
        _fields_ = [("Attribute", ctypes.c_int), ("Data", ctypes.POINTER(ACCENTPOLICY)),
                    ("SizeOfData", ctypes.c_size_t)]
    accent = ACCENTPOLICY()
    accent.AccentState = 4 if glass else 0
    accent.AccentFlags = 2
    accent.GradientColor = (170 << 24) | (0x14 << 16) | (0x16 << 8) | 0x12
    data = WINCOMPATTRDATA()
    data.Attribute = 19
    data.SizeOfData = ctypes.sizeof(accent)
    data.Data = ctypes.pointer(accent)
    ctypes.windll.user32.SetWindowCompositionAttribute(hwnd, ctypes.byref(data))
    backdrop = ctypes.c_int(3 if glass else 1)
    ctypes.windll.dwmapi.DwmSetWindowAttribute(hwnd, 38, ctypes.byref(backdrop), ctypes.sizeof(backdrop))
    radius = 18 if height < 64 else 28
    region = ctypes.windll.gdi32.CreateRoundRectRgn(0, 0, int(width) + 1, int(height) + 1, radius * 2, radius * 2)
    ctypes.windll.user32.SetWindowRgn(hwnd, region, True)


def motion_enabled():
    if sys.platform != "win32":
        return False  # Until native reduced-motion preferences are supported.
    import ctypes
    from ctypes import wintypes
    enabled = wintypes.BOOL()
    return bool(ctypes.windll.user32.SystemParametersInfoW(
        0x1042, 0, ctypes.byref(enabled), 0) and enabled.value)


class DemoSearch:
    def __init__(self):
        passages = json.loads((ROOT / "fixtures/passages.json").read_text(encoding="utf-8"))
        self.indexes = {}
        for scope in ("All", "Documents", "Code"):
            index = Index(":memory:", FakeEmbedding())
            index.sync([p for p in passages if scope == "All" or
                        (p["path"].startswith("src/")) == (scope == "Code")])
            self.indexes[scope] = index

    def search(self, question, scope):
        terms = set(re.findall(r"\w+", question.lower())) - STOP
        if not terms:
            return []
        # Hash vectors are synthetic: require lexical evidence before displaying a hit.
        hits = self.indexes[scope].search(question, limit=5)
        hits = [h for h in hits if terms.intersection(re.findall(r"\w+", (h["text"] + " " + h["path"]).lower()))]
        for i, hit in enumerate(hits, 1):
            hit["citation"] = f"S{i}"
        return hits[:3]

    def close(self):
        for index in self.indexes.values():
            index.db.close()


def enable_dpi_awareness():
    if sys.platform != "win32":
        return
    import ctypes
    try:
        ctypes.windll.shcore.SetProcessDpiAwareness(2)
    except (OSError, AttributeError):
        try:
            ctypes.windll.user32.SetProcessDPIAware()
        except (OSError, AttributeError):
            pass


def hex_to_rgb(colour):
    colour = colour.lstrip("#")
    return tuple(int(colour[i:i + 2], 16) for i in (0, 2, 4))


def smooth_round_photo(master, width, height, radius, fill, outline=None, outline_width=0):
    """Antialiased rounded rect as a PhotoImage (Pillow), or None if unavailable."""
    width, height = max(1, int(width)), max(1, int(height))
    radius = min(int(radius), width // 2, height // 2)
    try:
        from PIL import Image, ImageDraw, ImageTk
    except ImportError:
        return None
    scale = 3
    w, h = width * scale, height * scale
    r = min(radius * scale, w // 2, h // 2)
    img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    box = (0, 0, w - 1, h - 1)
    if outline and outline_width:
        ow = max(1, int(outline_width * scale))
        rgb_o = hex_to_rgb(outline)
        draw.rounded_rectangle(box, radius=r, outline=rgb_o + (255,), width=ow)
        inset = ow // 2 + scale
        inner = (inset, inset, w - 1 - inset, h - 1 - inset)
        ir = max(1, r - inset)
        draw.rounded_rectangle(inner, radius=ir, fill=hex_to_rgb(fill) + (255,))
    else:
        draw.rounded_rectangle(box, radius=r, fill=hex_to_rgb(fill) + (255,))
    img = img.resize((width, height), Image.Resampling.LANCZOS)
    return ImageTk.PhotoImage(img, master=master)


def canvas_round_rect(canvas, x1, y1, x2, y2, radius, fill, tags="bubble"):
    canvas.create_rectangle(x1 + radius, y1, x2 - radius, y2, fill=fill, outline="", tags=tags)
    canvas.create_rectangle(x1, y1 + radius, x2, y2 - radius, fill=fill, outline="", tags=tags)
    for x, y, start in ((x1, y1, 90), (x2 - 2 * radius, y1, 0),
                        (x1, y2 - 2 * radius, 180), (x2 - 2 * radius, y2 - 2 * radius, 270)):
        canvas.create_arc(x, y, x + 2 * radius, y + 2 * radius, start=start, extent=90,
                          fill=fill, outline="", tags=tags)


class BubbleView:
    """Rounded chat bubble backed by a canvas + text item."""
    def __init__(self, canvas, text_id, pad, fill):
        self.canvas = canvas
        self.text_id = text_id
        self.pad = pad
        self.fill = fill
        self._photo = None

    def winfo_exists(self):
        try:
            return self.canvas.winfo_exists()
        except tk.TclError:
            return False

    def configure(self, *, text=None, fg=None):
        if text is not None:
            self.canvas.itemconfigure(self.text_id, text=text)
        if fg is not None:
            self.canvas.itemconfigure(self.text_id, fill=fg)
        self._resize()

    def _resize(self):
        self.canvas.update_idletasks()
        box = self.canvas.bbox(self.text_id)
        if not box:
            return
        x1, y1, x2, y2 = box
        w = int(x2 - x1 + self.pad * 2)
        h = int(y2 - y1 + self.pad * 2)
        self.canvas.configure(width=w, height=h)
        self.canvas.delete("bubble")
        photo = smooth_round_photo(self.canvas, w, h, BUBBLE_RADIUS, self.fill)
        if photo is not None:
            self._photo = photo
            self.canvas.create_image(0, 0, anchor="nw", image=photo, tags="bubble")
        else:
            canvas_round_rect(self.canvas, 0, 0, w, h, BUBBLE_RADIUS, self.fill)
        self.canvas.tag_lower("bubble")
        self.canvas.coords(self.text_id, self.pad, self.pad)


def bubble_text_id(canvas, text, pad, fg, font):
    """Create wrapped text without forcing full wrap width on short messages."""
    base = dict(text=text or " ", anchor="nw", fill=fg, font=font)
    tid = canvas.create_text(pad, pad, **base)
    canvas.update_idletasks()
    box = canvas.bbox(tid)
    if box and box[2] - box[0] > BUBBLE_WRAP:
        canvas.delete(tid)
        tid = canvas.create_text(pad, pad, **base, width=BUBBLE_WRAP)
    return tid


class RoundedSurface(tk.Canvas):
    """Rounded search field with accent border."""
    def __init__(self, parent):
        super().__init__(parent, bg=BG, height=48, highlightthickness=0)
        self.inner = tk.Frame(self, bg=PANEL)
        self.window_id = self.create_window(14, 9, window=self.inner, anchor="nw")
        self.bind("<Configure>", self.draw)
        self._outline = ACCENT
        self._busy = False
        self._compose = False
        self._surface_photo = None

    def set_compose(self, active):
        self._compose = bool(active)
        self.draw()

    def set_busy(self, active):
        self._busy = bool(active)
        self.draw()

    def set_outline(self, color):
        self._outline = color
        self.draw()

    def draw(self, event=None):
        width = max(self.winfo_width(), 120)
        self.delete("surface")
        height = 52 if self._compose else 48
        radius = 18 if self._compose else 16
        outline = ACCENT if self._busy else (HOVER if self._compose else self._outline)
        ow = 1 if self._compose and not self._busy else 2
        photo = smooth_round_photo(self, width, height, radius, PANEL, outline=outline, outline_width=ow)
        if photo is not None:
            self._surface_photo = photo
            self.create_image(0, 0, anchor="nw", image=photo, tags="surface")
        else:
            self._round_rect(1, 1, width - 1, height - 1, radius, outline=outline, width=ow)
            self._round_rect(2, 2, width - 2, height - 2, radius - 1, fill=PANEL)
        self.tag_lower("surface")
        self.configure(height=height)
        inner_h = 34 if self._compose else 30
        y_off = 10 if self._compose else 9
        self.coords(self.window_id, 14, y_off)
        self.itemconfigure(self.window_id, width=max(1, width - 28), height=inner_h)

    def _round_rect(self, x1, y1, x2, y2, radius, fill=None, outline="", width=1):
        tags = "surface"
        if fill:
            self.create_rectangle(x1 + radius, y1, x2 - radius, y2, fill=fill, outline="", tags=tags)
            self.create_rectangle(x1, y1 + radius, x2, y2 - radius, fill=fill, outline="", tags=tags)
            for x, y, start in ((x1, y1, 90), (x2 - 2 * radius, y1, 0),
                                (x1, y2 - 2 * radius, 180), (x2 - 2 * radius, y2 - 2 * radius, 270)):
                self.create_arc(x, y, x + 2 * radius, y + 2 * radius, start=start, extent=90,
                                fill=fill, outline="", tags=tags)
        elif outline:
            self.create_arc(x1, y1, x1 + 2 * radius, y1 + 2 * radius, start=90, extent=90,
                            style="arc", outline=outline, width=width, tags=tags)
            self.create_arc(x2 - 2 * radius, y1, x2, y1 + 2 * radius, start=0, extent=90,
                            style="arc", outline=outline, width=width, tags=tags)
            self.create_arc(x1, y2 - 2 * radius, x1 + 2 * radius, y2, start=180, extent=90,
                            style="arc", outline=outline, width=width, tags=tags)
            self.create_arc(x2 - 2 * radius, y2 - 2 * radius, x2, y2, start=270, extent=90,
                            style="arc", outline=outline, width=width, tags=tags)
            self.create_line(x1 + radius, y1, x2 - radius, y1, fill=outline, width=width, tags=tags)
            self.create_line(x1 + radius, y2, x2 - radius, y2, fill=outline, width=width, tags=tags)
            self.create_line(x1, y1 + radius, x1, y2 - radius, fill=outline, width=width, tags=tags)
            self.create_line(x2, y1 + radius, x2, y2 - radius, fill=outline, width=width, tags=tags)


class LoftPreview:
    def __init__(self, root, demo=False, use_settings=True):
        self.root = root
        self.demo = demo
        self.searcher = DemoSearch() if demo else None
        self.engine = None
        self.busy = False
        self.busy_started = None
        self.phase_text = ""
        self.job_kind = None
        self._shown_second = None
        self._shown_phase = None
        self.closed = False
        self.events = queue.Queue()
        self.cancel_work = threading.Event()
        self.worker = None
        self.watcher = None
        self.library_window = None
        self.answer_window = None
        if not demo:
            from loft_engine import Engine
            from loft_watch import FolderWatcher
            self.engine = Engine()
            self.watcher = FolderWatcher(self._watched_folder_changed)
            self.sync_watcher()
        self.expanded = False
        self.animation_timer = None
        self.busy_anim_timer = None
        self.busy_pulse = 0.0
        self.spinner_angle = 0
        self.answer_skeleton = None
        self.progress_track = None
        self.motion_ready = False
        self.scope = "All"
        self.source_window = None
        self.last_hits = []
        self.answer_card = None
        self.answer_text = None
        self.answer_buffer = ""
        self.stop_button = None
        self.sources_detail = None
        self.sources_toggle = None
        self.sources_open = False
        self.support_gaps = []
        self.support_note = None
        self.answer_heading = None
        self.answer_body = None
        self.caution_frame = None
        self.caution_label = None
        self.sources_wrap = None
        self.chat_history = []
        self.thread = []
        self.file_thread = []
        self.chat_thread = []
        self._bubble_photos = []
        self._pinned_height = None
        self._input_at_bottom = False
        self.pending_chat = ""
        self._stale_checked = 0.0
        self.library_topmost = False
        self.pending_folders = []
        self.folder_rows = []
        self._status_error = False
        self.awaiting_empty_note = False
        self.in_settings = False
        self.origin = None
        self._press = None
        self.use_settings = use_settings
        self.prefs = self.load_prefs() if use_settings else {}
        self.glass = bool(self.prefs.get("glass", True)) if use_settings else False
        self.opacity = float(self.prefs.get("opacity", 0.78))
        self.topmost = bool(self.prefs.get("topmost", True))
        if use_settings and "x" in self.prefs and "y" in self.prefs:
            self.origin = (int(self.prefs["x"]), int(self.prefs["y"]))
            screens = monitors()
            # A remembered point on an unplugged display would open off-screen.
            if len(screens) == 1 or not on_screen(self.origin[0], self.origin[1], screens):
                self.origin = None
        root.title("Loft — desktop preview")
        root.configure(bg=BG)
        root.overrideredirect(True)
        root.attributes("-topmost", self.topmost)
        root.protocol("WM_DELETE_WINDOW", self.close)
        root.bind("<Escape>", self.on_escape)
        root.bind("<Control-q>", lambda _: self.close())
        root.bind("<Control-k>", lambda _: self.focus_search())
        root.bind("<Command-k>", lambda _: self.focus_search()) if sys.platform == "darwin" else None
        self.canvas = tk.Canvas(root, bg=BG, highlightthickness=0)
        self.canvas.pack(fill="both", expand=True)
        self.body = tk.Frame(self.canvas, bg=BG)
        self.body_id = self.canvas.create_window(24, 18, window=self.body, anchor="nw")
        self.compact = tk.Frame(self.canvas, bg=BG)
        self.compact_id = self.canvas.create_window(18, 5, window=self.compact, anchor="nw")
        self.compact_title = tk.Label(self.compact, text="loft    Ask your files    Ctrl+K", bg=BG, fg=TEXT,
                                      font=("Segoe UI", 10), cursor="fleur")
        self.compact_title.pack(side="left", padx=(4, 0))
        self.bind_drag(self.compact_title, self.focus_search)
        self.compact_stop = self.button(self.compact, "Stop", lambda: self.cancel_work.set(), muted=True)
        self.compact_close = self.button(self.compact, "×", self.close, muted=True)
        self.compact_close.pack(side="left")
        header = self.header = tk.Frame(self.body, bg=BG, cursor="fleur")
        header.pack(fill="x")
        self.bind_drag(header)
        title = tk.Label(header, text="loft", bg=BG, fg=TEXT, font=("Segoe UI", 14, "bold"), cursor="fleur")
        title.pack(side="left")
        self.bind_drag(title)
        dot = tk.Canvas(header, width=10, height=10, bg=BG, highlightthickness=0, cursor="fleur")
        dot.create_oval(2, 2, 8, 8, fill=ACCENT, outline="")
        dot.pack(side="left", padx=(8, 4))
        self.bind_drag(dot)
        local = tk.Label(header, text="Local", bg=BG, fg=MUTED, font=("Segoe UI", 10), cursor="fleur")
        local.pack(side="left")
        self.bind_drag(local)
        self.button(header, "⚙", self.show_settings, muted=True).pack(side="right", padx=(4, 0))
        self.button(header, "×", self.close, muted=True).pack(side="right")
        self.input_surface = RoundedSurface(self.body)
        self.input_surface.pack(fill="x", pady=(10, 8))
        row = self.input_surface.inner
        self.compose_plus = self.button(row, "+", self.compose_attach, muted=True)
        self.search_icon = tk.Label(row, text="🔍", bg=PANEL, fg=MUTED, font=("Segoe UI", 11))
        self.search_icon.pack(side="left", padx=(0, 6))
        self.question = tk.StringVar()
        self.entry = tk.Entry(row, textvariable=self.question, bg=PANEL, fg=TEXT,
                              insertbackground=ACCENT, relief="flat", font=("Segoe UI", 12),
                              disabledbackground=PANEL, disabledforeground=MUTED, readonlybackground=PANEL,
                              highlightthickness=0, bd=0)
        self.entry.pack(side="left", fill="x", expand=True, ipady=2)
        self.entry.bind("<Return>", lambda _: self.search())
        self.model_picker = tk.Button(
            row, text="Model  ▾", command=self.open_model_menu, bg=PANEL, fg=MUTED,
            activebackground=PANEL, activeforeground=TEXT, relief="flat", bd=0, padx=6, pady=2,
            cursor="hand2", font=("Segoe UI", 9), highlightthickness=0,
        )
        self.model_menu = tk.Menu(self.root, tearoff=0, bg=PANEL, fg=TEXT,
                                  activebackground=HOVER, activeforeground=TEXT, borderwidth=0)
        self.send_button = tk.Button(
            row, text="↑", command=self.search, bg=ACCENT, fg=BG,
            activebackground=ACCENT, activeforeground=BG, relief="flat", bd=0,
            width=2, padx=6, pady=0, cursor="hand2", font=("Segoe UI", 12, "bold"),
            highlightthickness=0,
        )
        self.send_button.pack(side="right", padx=(4, 0))
        self.placeholder = tk.Label(self.entry, text="Ask your files...", bg=PANEL,
                                    fg=MUTED, font=("Segoe UI", 12), cursor="xterm")
        self.placeholder.place(x=0, rely=0.5, anchor="w")
        self.placeholder.bind("<Button-1>", lambda _: self.entry.focus_set())
        self.question.trace_add("write", self.update_placeholder)
        self._compose_bar = False
        scopes = self.scopes = tk.Frame(self.body, bg=BG)
        scopes.pack(fill="x", pady=(0, 8))
        self.scope_buttons = {}
        scope_labels = {
            "All": "▦  Everything",
            "Documents": "📄  Documents",
            "Code": "</>  Code",
            "Chat": "💬  Chat",
        }
        for scope in (("All", "Documents", "Code") if demo else ("All", "Documents", "Code", "Chat")):
            button = self.button(scopes, scope_labels[scope], lambda s=scope: self.select_scope(s), muted=True)
            button.pack(side="left", padx=(0, 6))
            self.scope_buttons[scope] = button
        self.status_row = tk.Frame(self.body, bg=BG)
        self.status_row.pack(fill="x", pady=(0, 4))
        self.spinner = tk.Canvas(self.status_row, width=18, height=18, bg=BG, highlightthickness=0)
        self.status = tk.Label(self.status_row, bg=BG, fg=MUTED, anchor="w", justify="left",
                               font=("Segoe UI", 10), wraplength=470)
        self.status.pack(side="left", fill="x", expand=True)
        self.scroll_host = tk.Frame(self.body, bg=BG)
        self.scroll_host.pack(fill="x")
        self.scroll = tk.Canvas(self.scroll_host, bg=BG, height=1, highlightthickness=0, bd=0)
        self.vbar = tk.Scrollbar(self.scroll_host, orient="vertical", command=self.scroll.yview,
                                 bg=PANEL, troughcolor=BG, activebackground=HOVER, highlightthickness=0)
        self.scroll.configure(yscrollcommand=self._on_results_scroll)
        self.scroll.pack(side="left", fill="x", expand=True)
        self.scroll_inner = tk.Frame(self.scroll, bg=BG)
        self.scroll_window = self.scroll.create_window((0, 0), window=self.scroll_inner, anchor="nw")
        self.results = tk.Frame(self.scroll_inner, bg=BG)
        self.results.pack(fill="x")
        self.scroll.bind("<Configure>", self._sync_scroll_width)
        self.root.bind_all("<MouseWheel>", self._on_results_wheel)
        self.root.bind_all("<Button-4>", self._on_results_wheel)
        self.root.bind_all("<Button-5>", self._on_results_wheel)
        footer = self.footer = tk.Frame(self.body, bg=BG)
        footer.pack(fill="x", pady=(10, 0))
        self.library_label = tk.Label(footer, text=self.library_summary(),
                 bg=BG, fg=MUTED, font=("Segoe UI", 9))
        self.library_label.pack(side="left")
        local_row = self.local_row = tk.Frame(footer, bg=BG)
        local_row.pack(side="right")
        live = tk.Canvas(local_row, width=10, height=10, bg=BG, highlightthickness=0)
        live.create_oval(2, 2, 8, 8, fill=SUCCESS, outline="")
        live.pack(side="left", padx=(0, 6))
        tk.Label(local_row, text="Local", bg=BG, fg=MUTED, font=("Segoe UI", 9)).pack(side="left")
        self.style_scopes()
        self.show_welcome()
        self.collapse()
        self.motion_ready = True
        self.theme_timer = None
        self.refresh_theme()
        self.event_timer = self.root.after(50, self.poll_events) if not demo else None

    def on_escape(self, _event=None):
        if self.busy and not self.demo:
            self.cancel_work.set()
            self.note_phase("Stopping…")
            return
        self.collapse()

    def start_job(self, operation, kind=None):
        if self.busy:
            self.note_phase("An operation is running. Use Stop, then try again.")
            return False
        self.busy = True
        self.job_kind = kind
        self._status_error = False
        self.busy_started = time.perf_counter()
        self._shown_second = None
        self._shown_phase = None
        self.cancel_work = threading.Event()
        self.entry.configure(state="disabled")
        self.sync_entry_style()
        self.root.after_idle(self.fit_content)
        captured = kind
        def emit(kind, value):
            self.events.put((kind, value))
        def work():
            try:
                result = operation(self.cancel_work, emit)
                emit("done", result)
            except Exception as exc:
                emit("error", self._worker_error(exc, captured))
            finally:
                emit("idle", {"cancelled": self.cancel_work.is_set()})
        self.worker = threading.Thread(target=work, daemon=True)
        self.worker.start()
        self.start_busy_fx()
        return True

    def _worker_error(self, exc, kind):
        if self.cancel_work.is_set():
            if kind == "index":
                return "Stopped. Your saved index is unchanged."
            return "Stopped."
        text = str(exc).strip() or exc.__class__.__name__
        return text

    def show_stop(self):
        if self.stop_button is None or not self.stop_button.winfo_exists():
            self.stop_button = self.button(self.footer, "Stop", lambda: self.cancel_work.set())
            self.stop_button.configure(bg=ACCENT, fg=BG, activebackground=ACCENT, activeforeground=BG)
            self.stop_button.pack(side="right", padx=(0, 8), before=self.local_row)
        if self.compact_stop is not None and not self.compact_stop.winfo_ismapped():
            self.compact_stop.pack(side="left", before=self.compact_close)

    def hide_stop(self):
        if self.stop_button is not None and self.stop_button.winfo_exists():
            self.stop_button.destroy()
        self.stop_button = None
        if self.compact_stop is not None and self.compact_stop.winfo_ismapped():
            self.compact_stop.pack_forget()

    def restore_compact(self):
        self.compact_title.configure(text="loft    Ask your files    Ctrl+K")

    def note_phase(self, text):
        self.phase_text = text
        if self.busy:
            self.paint_activity(force=True)
        else:
            self.status.configure(text=text, fg=ERROR if self._status_error else MUTED)

    def paint_activity(self, force=False):
        if not self.busy or self.busy_started is None or not self.phase_text:
            return
        second = int(time.perf_counter() - self.busy_started)
        if not force and second == self._shown_second and self.phase_text == self._shown_phase:
            return
        self._shown_second = second
        self._shown_phase = self.phase_text
        self.status.configure(text=f"{self.phase_text}  {second}s", fg=ACCENT)
        short = self.phase_text if len(self.phase_text) <= 42 else self.phase_text[:41] + "…"
        self.compact_title.configure(text=f"loft    {short}    {second}s")

    def start_busy_fx(self):
        self.stop_busy_fx(keep_phase=True)
        self.spinner.pack(side="left", padx=(0, 8), before=self.status)
        self.tick_busy_fx()

    def stop_busy_fx(self, keep_phase=False):
        if self.busy_anim_timer is not None:
            self.root.after_cancel(self.busy_anim_timer)
            self.busy_anim_timer = None
        if not keep_phase:
            self.busy_pulse = 0.0
            self.spinner_angle = 0
        self.canvas.delete("work_border")
        if hasattr(self, "input_surface"):
            self.input_surface.set_busy(False)
            self.input_surface.set_outline(ACCENT)
        if hasattr(self, "spinner") and self.spinner.winfo_exists():
            self.spinner.delete("all")
            if self.spinner.winfo_ismapped():
                self.spinner.pack_forget()

    def tick_busy_fx(self):
        if self.closed or not self.busy:
            self.stop_busy_fx()
            return
        self.draw_spinner()
        self.busy_anim_timer = self.root.after(120, self.tick_busy_fx)

    def draw_spinner(self):
        self.spinner.delete("all")
        self.spinner.create_oval(3, 3, 15, 15, outline=HOVER, width=2)
        start = self.spinner_angle
        self.spinner_angle = (self.spinner_angle + 24) % 360
        self.spinner.create_arc(3, 3, 15, 15, start=start, extent=75, style="arc",
                                outline=ACCENT, width=2)

    def draw_work_border(self):
        self.canvas.delete("work_border")
        target = self.body_id if self.expanded else self.compact_id
        try:
            box = self.canvas.bbox(target)
        except tk.TclError:
            return
        if not box:
            return
        pad = 6
        x1, y1, x2, y2 = box[0] - pad, box[1] - pad, box[2] + pad, box[3] + pad
        radius = 22 if self.expanded else 14
        self._canvas_round_outline(self.canvas, x1, y1, x2, y2, radius, GLOW_SOFT, 4, "work_border")
        inset = 2
        self._canvas_round_outline(self.canvas, x1 + inset, y1 + inset, x2 - inset, y2 - inset,
                                   max(radius - 2, 8), ACCENT, 2, "work_border")

    def _canvas_round_outline(self, canvas, x1, y1, x2, y2, radius, color, width, tags):
        canvas.create_arc(x1, y1, x1 + 2 * radius, y1 + 2 * radius, start=90, extent=90,
                          style="arc", outline=color, width=width, tags=tags)
        canvas.create_arc(x2 - 2 * radius, y1, x2, y1 + 2 * radius, start=0, extent=90,
                          style="arc", outline=color, width=width, tags=tags)
        canvas.create_arc(x1, y2 - 2 * radius, x1 + 2 * radius, y2, start=180, extent=90,
                          style="arc", outline=color, width=width, tags=tags)
        canvas.create_arc(x2 - 2 * radius, y2 - 2 * radius, x2, y2, start=270, extent=90,
                          style="arc", outline=color, width=width, tags=tags)
        canvas.create_line(x1 + radius, y1, x2 - radius, y1, fill=color, width=width, tags=tags)
        canvas.create_line(x1 + radius, y2, x2 - radius, y2, fill=color, width=width, tags=tags)
        canvas.create_line(x1, y1 + radius, x1, y2 - radius, fill=color, width=width, tags=tags)
        canvas.create_line(x2, y1 + radius, x2, y2 - radius, fill=color, width=width, tags=tags)

    def draw_answer_progress(self):
        if self.progress_track is None or not self.progress_track.winfo_exists():
            return
        if self.answer_buffer.strip():
            return
        self.progress_track.delete("all")
        width = max(self.progress_track.winfo_width(), 240)
        self.progress_track.create_rectangle(0, 1, width, 5, fill=HOVER, outline="")
        segment = max(48, width // 4)
        travel = max(1, width - segment)
        x = int(travel * self.busy_pulse)
        self.progress_track.create_rectangle(x, 1, x + segment, 5, fill=ACCENT, outline="")

    def hide_answer_loading_chrome(self):
        if self.progress_track is not None and self.progress_track.winfo_exists() and self.progress_track.winfo_ismapped():
            self.progress_track.pack_forget()
        if self.answer_skeleton is not None and self.answer_skeleton.winfo_exists() and self.answer_skeleton.winfo_ismapped():
            self.answer_skeleton.pack_forget()

    def show_note(self, message, color=None):
        tk.Label(self.results, text=message, bg=BG, fg=color or TEXT, wraplength=490,
                 justify="left", anchor="w", font=("Segoe UI", 11)).pack(anchor="w", fill="x", pady=(4, 6))

    def poll_events(self):
        if self.closed:
            return
        try:
            self.drain_events()
            self.refresh_stale_markers()
        except Exception:
            traceback.print_exc()
            if self.worker is None or not self.worker.is_alive():
                self.busy = False
                self.job_kind = None
                self.hide_stop()
                try:
                    self.entry.configure(state="normal")
                    self.sync_entry_style()
                except tk.TclError:
                    pass
            try:
                self.status.configure(text="Something went wrong updating the window. You can try again.", fg=ERROR)
            except tk.TclError:
                pass
        finally:
            if not self.closed:
                try:
                    self.event_timer = self.root.after(50, self.poll_events)
                except tk.TclError:
                    pass

    def drain_events(self):
        changed = False
        for _ in range(100):
            try:
                kind, value = self.events.get_nowait()
            except queue.Empty:
                break
            if kind == "stale":
                if not self.busy:
                    self.refresh_library_list()
            elif kind == "status":
                self._status_error = False
                self.note_phase(value)
                if self.job_kind == "index":
                    self.set_library_status(value, ACCENT)
                if self.awaiting_empty_note:
                    self.render_thread()
                changed = True
            elif kind == "chat":
                self.awaiting_empty_note = False
                if self.pending_chat:
                    self.thread.append({"role": "user", "text": self.pending_chat})
                    self.pending_chat = ""
                self.thread.append({"role": "assistant", "text": value})
                self.chat_history = self.chat_context()
                if self.scope == "Chat":
                    self.chat_thread = list(self.thread)
                self.render_thread()
                self.root.lift()
                changed = True
            elif kind == "support":
                self.support_gaps = list(value or [])
                self.render_thread()
                changed = True
            elif kind == "sources":
                self.support_gaps = []
                self.last_hits = value or []
                self.sources_open = bool(value)
                if value:
                    self.awaiting_empty_note = False
                else:
                    self.awaiting_empty_note = True
                self.render_thread()
                self.root.lift()
                changed = True
            elif kind == "token":
                self.answer_buffer += value
                preview = self.answer_buffer[:450]
                shown = preview + ("…" if len(self.answer_buffer) > 450 else "")
                if self.live_assistant_label is not None and self.live_assistant_label.winfo_exists():
                    self.live_assistant_label.configure(text=shown, fg=TEXT)
                elif self.answer_text is not None and self.answer_text.winfo_exists():
                    self.answer_text.configure(text=shown, fg=TEXT)
                else:
                    self.render_thread()
                if self.answer_window and self.answer_window.winfo_exists() and getattr(self, "full_answer", None):
                    self.full_answer.configure(state="normal")
                    self.full_answer.insert("end", value)
                    self.full_answer.see("end")
                    self.full_answer.configure(state="disabled")
                changed = True
            elif kind == "warning":
                tk.Label(self.results, text=value, bg=BG, fg=ACCENT, wraplength=490,
                         justify="left", font=("Segoe UI", 9)).pack(fill="x", pady=5)
                changed = True
            elif kind == "error":
                self.phase_text = value
                self._status_error = True
                self.awaiting_empty_note = False
                self.status.configure(text=value, fg=ERROR)
                self.show_note(value, ERROR)
                if self.answer_text is not None and self.answer_text.winfo_exists() and not self.answer_buffer:
                    self.answer_text.configure(text="The answer stopped before it finished.")
                if self.job_kind == "index":
                    self.set_library_status("Indexing stopped: " + value, ERROR)
                changed = True
            elif kind == "done" and isinstance(value, dict) and "model_ready" in value:
                self.phase_text = value["model_ready"] + " ready · choose it in Settings."
                self._status_error = False
                self.model_ready_pending = True
            elif kind == "done" and isinstance(value, dict) and "indexed" in value:
                summary = f"{value['indexed']} updated, {value['unchanged']} unchanged, {value['deleted']} removed"
                if value.get("skipped"):
                    summary += f", {value['skipped']} skipped"
                if value.get("unreadable"):
                    summary += f", {value['unreadable']} unreadable"
                self.phase_text = "Ready · " + summary
                self._status_error = False
                detail = format_dropped(value)
                shown = summary if not detail else summary + "\n" + detail
                self.set_library_status("Indexing complete · " + shown, ACCENT)
                if detail:
                    self.show_note(detail, MUTED)
            elif kind == "idle":
                cancelled = isinstance(value, dict) and bool(value.get("cancelled"))
                finishing = self.job_kind
                self.busy = False
                self.job_kind = None
                self.hide_stop()
                self.finish_answer()
                if finishing == "ask" and not self.demo:
                    self.finalize_file_turn()
                elif finishing == "chat":
                    self.sync_status_strip()
                self.restore_compact()
                if self.phase_text and (self._status_error or not self.thread):
                    self.status.configure(text=self.phase_text, fg=ERROR if self._status_error else MUTED)
                self.sync_status_strip()
                self.entry.configure(state="normal")
                self.sync_entry_style()
                self.stop_busy_fx()
                changed = True
                if self.engine is not None:
                    self.refresh_library_summary()
                    self.sync_watcher()
                if self.library_window and self.library_window.winfo_exists():
                    self.refresh_library_list()
                self.set_library_topmost(False)
                if self.pending_folders and not cancelled:
                    self.start_next_pending_folder()
                elif cancelled and self.pending_folders:
                    waiting = len(self.pending_folders)
                    noun = "folder" if waiting == 1 else "folders"
                    self.set_library_status(f"Stopped. {waiting} {noun} still waiting — select one and choose Refresh.", MUTED)
                    self.refresh_library_list()
                if getattr(self, "model_ready_pending", False) and not self.busy:
                    self.model_ready_pending = False
                    self.show_settings()
                    self.status.configure(text=self.phase_text, fg=ACCENT)
        self.paint_activity()
        if changed:
            self.fit_content()

    def show_answer(self):
        if self.answer_window and self.answer_window.winfo_exists():
            self.answer_window.lift()
            return
        self.answer_window = tk.Toplevel(self.root)
        self.answer_window.title("Loft · Local answer")
        self.answer_window.configure(bg=BG)
        self.answer_window.attributes("-topmost", True)
        self.place_dialog(self.answer_window, 700, 500)
        self.full_answer = scrolledtext.ScrolledText(self.answer_window, bg=BG, fg=TEXT,
            wrap="word", padx=24, pady=24, font=("Segoe UI", 12))
        self.full_answer.pack(fill="both", expand=True)
        self.full_answer.insert("1.0", self.answer_buffer)
        self.mark_unsupported(self.full_answer, self.answer_buffer)
        self.full_answer.configure(state="disabled")
        if self.support_gaps:
            self.answer_window.title("Loft · Local answer · not fully supported")
            self.answer_gap_note = tk.Label(self.answer_window, text=self.support_line(), bg=BG, fg=ERROR,
                            wraplength=640, justify="left", anchor="w", font=("Segoe UI", 10), padx=16, pady=8)
            self.answer_gap_note.pack(anchor="w", before=self.full_answer)

    def manage_library(self):
        indexing = bool(self.busy and self.job_kind == "index")
        if self.library_window and self.library_window.winfo_exists():
            self.set_library_topmost(indexing)
            self.library_window.lift()
            return
        window = self.library_window = tk.Toplevel(self.root)
        window.title("Loft · Your folders")
        window.configure(bg=BG)
        self.library_topmost = indexing
        window.attributes("-topmost", indexing)
        self.place_dialog(window, 680, 380)
        tk.Label(window, text="Choose what Loft can search", bg=BG, fg=TEXT, font=("Segoe UI", 16), padx=20, pady=20).pack(anchor="w")
        tk.Label(window, text="Text, Markdown, PDF and code · changes are marked · Refresh to reindex",
                 bg=BG, fg=MUTED, padx=20).pack(anchor="w")
        self.folder_list = tk.Listbox(window, bg=PANEL, fg=TEXT, relief="flat", selectbackground=HOVER)
        self.folder_list.pack(fill="both", expand=True, padx=20, pady=15)
        self.library_status = tk.Label(window, text="Ready to add or refresh a folder", bg=BG, fg=MUTED,
                                       anchor="w", justify="left", wraplength=620, padx=20, font=("Segoe UI", 10))
        self.library_status.pack(fill="x", pady=(0, 8))
        actions = tk.Frame(window, bg=BG)
        actions.pack(fill="x", padx=20, pady=(0, 18))
        add_button = self.button(actions, "+ Add folder", self.add_folder)
        add_button.configure(bg=ACCENT, fg=BG, activebackground=HOVER, activeforeground=TEXT)
        add_button.pack(side="left", padx=(0, 8))
        self.button(actions, "Refresh selected", self.refresh_folder).pack(side="left")
        self.button(actions, "Remove selected", self.remove_folder).pack(side="left")
        self.button(actions, "Stop", lambda: self.cancel_work.set()).pack(side="right")
        self.refresh_library_list()
        if self.busy and self.job_kind == "index":
            self.set_library_status("Indexing… " + self.phase_text, ACCENT)

    def set_library_status(self, message, color=MUTED):
        if self.library_window and self.library_window.winfo_exists():
            self.library_status.configure(text=message, fg=color)

    def set_library_topmost(self, enabled):
        self.library_topmost = bool(enabled)
        if self.library_window and self.library_window.winfo_exists():
            try:
                self.library_window.attributes("-topmost", self.library_topmost)
            except tk.TclError:
                pass

    def refresh_library_list(self):
        if not (self.library_window and self.library_window.winfo_exists()):
            return
        selected = self.selected_folder()
        self.folder_list.delete(0, "end")
        self.folder_rows = []
        seen = set()
        for folder in self.engine.roots():
            count = self.engine.root_count(folder)
            noun = "file" if count == 1 else "files"
            label = f"{folder}    ·    {count} {noun}"
            if getattr(self.engine, "root_is_stale", None) and self.engine.root_is_stale(folder):
                label += "    ·    changed · refresh to update"
            self.folder_list.insert("end", label)
            self.folder_rows.append(folder)
            seen.add(folder)
        for folder in self.pending_folders:
            if folder in seen:
                continue
            self.folder_list.insert("end", f"{folder}    ·    waiting to index")
            self.folder_rows.append(folder)
        if selected:
            self.select_folder_row(selected)

    def selected_folder(self):
        if not (self.library_window and self.library_window.winfo_exists()):
            return None
        selected = self.folder_list.curselection()
        if not selected or selected[0] >= len(self.folder_rows):
            return None
        return self.folder_rows[selected[0]]

    def select_folder_row(self, folder):
        if folder in self.folder_rows and self.library_window and self.library_window.winfo_exists():
            index = self.folder_rows.index(folder)
            self.folder_list.selection_clear(0, "end")
            self.folder_list.selection_set(index)
            self.folder_list.see(index)

    def folder_error(self, message):
        self._status_error = True
        self.phase_text = message
        self.status.configure(text=message, fg=ERROR)
        self.set_library_status(message, ERROR)

    def pending_overlap(self, folder):
        root = Path(folder)
        for pending in self.pending_folders:
            other = Path(pending)
            if root != other and (root.is_relative_to(other) or other.is_relative_to(root)):
                return True
        return False

    def add_folder(self):
        folder = filedialog.askdirectory(parent=self.library_window, title="Choose a folder for Loft")
        if folder:
            self.include_folder(folder)

    def include_folder(self, folder):
        try:
            folder = self.engine.normalize_root(folder)
        except (OSError, ValueError) as exc:
            self.folder_error(str(exc))
            return
        if self.pending_overlap(folder):
            self.folder_error("That folder overlaps a folder that is waiting to index.")
            return
        if folder in self.pending_folders:
            self.set_library_status("That folder is already waiting to index.", MUTED)
            return
        if self.busy:
            self.pending_folders.append(folder)
            self.refresh_library_list()
            self.note_phase("Folder added. It will index when the current task finishes.")
            self.set_library_status("Folder added · waiting to index…", ACCENT)
            return
        self.index_now(folder)

    def index_now(self, folder):
        try:
            folder = self.engine.add_root(folder)
        except (OSError, ValueError) as exc:
            self.folder_error(str(exc))
            return
        except sqlite3.OperationalError:
            if folder not in self.pending_folders:
                self.pending_folders.append(folder)
            self.refresh_library_list()
            self.folder_error("The library is busy. The folder will index when it is free.")
            return
        self.refresh_library_list()
        self.select_folder_row(folder)
        self.sync_watcher()
        self.start_index(folder)

    def start_index(self, folder):
        if self.start_job(lambda cancel, emit, folder=folder: self.engine.index_folder(folder, cancel, emit), kind="index"):
            self.show_stop()
            self.set_library_topmost(True)
            if self.library_window and self.library_window.winfo_exists():
                self.library_window.lift()
            self.note_phase("Loading the search model…")
            self.set_library_status("Indexing… loading the search model", ACCENT)
            return
        if folder not in self.pending_folders:
            self.pending_folders.insert(0, folder)
        self.refresh_library_list()

    def start_next_pending_folder(self):
        while self.pending_folders:
            folder = self.pending_folders.pop(0)
            try:
                folder = self.engine.add_root(folder)
            except (OSError, ValueError) as exc:
                self.folder_error(str(exc))
                continue
            except sqlite3.OperationalError:
                self.pending_folders.insert(0, folder)
                self.refresh_library_list()
                self.folder_error("The library is busy. Select the folder and refresh to try again.")
                return
            self.refresh_library_list()
            self.select_folder_row(folder)
            self.sync_watcher()
            self.start_index(folder)
            return
        self.refresh_library_list()

    def refresh_folder(self):
        folder = self.selected_folder()
        if folder is None:
            self.set_library_status("Select a folder, then refresh it.", MUTED)
            return
        if self.busy:
            self.set_library_status("An operation is running. Use Stop, then refresh.", MUTED)
            return
        if folder in self.pending_folders:
            self.pending_folders.remove(folder)
        self.index_now(folder)

    def remove_folder(self):
        folder = self.selected_folder()
        if folder is None:
            self.set_library_status("Select a folder first.", MUTED)
            return
        if self.busy:
            self.set_library_status("An operation is running. Use Stop, then remove the folder.", MUTED)
            return
        if folder in self.pending_folders and folder not in self.engine.roots():
            if not self.confirm_remove(folder, 0, waiting=True):
                return
            self.pending_folders.remove(folder)
            self.refresh_library_list()
            self.set_library_status("Removed the folder that was waiting to index.", MUTED)
            return
        count = self.engine.root_count(folder) if folder in self.engine.roots() else 0
        if not self.confirm_remove(folder, count):
            return
        self.engine.remove(folder)
        if getattr(self.engine, "clear_root_changed", None):
            self.engine.clear_root_changed(folder)
        self.sync_watcher()
        if folder in self.pending_folders:
            self.pending_folders.remove(folder)
        self.refresh_library_list()
        self.refresh_library_summary()
        self.clear_results()
        self._status_error = False
        noun = "file" if count == 1 else "files"
        self.status.configure(text=f"Folder removed from Loft. {count} indexed {noun} are no longer searchable. Original files are unchanged.", fg=MUTED)
        self.fit_content()

    def confirm_remove(self, folder, count, waiting=False):
        if waiting:
            message = "This folder is waiting to index and is not searchable yet. Remove it from the list?"
        else:
            noun = "file" if count == 1 else "files"
            message = (f"Remove this folder from search?\n\n{count} indexed {noun} will no longer be searchable. "
                       "Original files stay on disk.")
        return messagebox.askyesno("Loft · Remove folder", message, parent=self.library_window)

    def sync_entry_style(self):
        if not getattr(self, "entry", None):
            return
        try:
            if not self.entry.winfo_exists():
                return
        except tk.TclError:
            return
        state = str(self.entry.cget("state"))
        self.entry.configure(bg=PANEL, insertbackground=ACCENT, disabledbackground=PANEL,
                             disabledforeground=MUTED, readonlybackground=PANEL,
                             fg=TEXT if state == "normal" else MUTED)
        if getattr(self, "placeholder", None) and self.placeholder.winfo_exists():
            self.placeholder.configure(bg=PANEL)
        if getattr(self, "model_picker", None) and self.model_picker.winfo_exists():
            self.model_picker.configure(bg=PANEL, activebackground=PANEL)
        self.refresh_model_picker()

    def update_placeholder(self, *_):
        if self.question.get():
            self.placeholder.place_forget()
        else:
            self.placeholder.place(x=0, rely=0.5, anchor="w")

    def selected_model_title(self):
        if self.demo or self.engine is None:
            return "Sample model"
        models_api = getattr(self.engine, "models", None)
        if models_api is None:
            return "Local model"
        try:
            selected = models_api.selected_answer()
        except (RuntimeError, KeyError, ValueError, OSError):
            return "Choose model"
        if not selected:
            return "Choose model"
        title = selected.get("title") or Path(selected["file"]).stem
        return title

    def refresh_model_picker(self):
        if not getattr(self, "model_picker", None) or not self.model_picker.winfo_exists():
            return
        title = self.selected_model_title()
        if len(title) > 20:
            title = title[:19] + "…"
        self.model_picker.configure(text=f"{title}  ▾")

    def open_model_menu(self):
        if self.demo or self.engine is None:
            self.note_phase("Models are not available in demo mode.")
            return
        self.model_menu.delete(0, "end")
        entries = self.model_entries(self.engine.models)
        if not entries:
            self.model_menu.add_command(label="No models · open Settings", command=self.show_settings)
        for item in entries:
            prefix = "✓ " if item["active"] else ""
            label = prefix + item["title"]
            action = item.get("action")
            if item["active"] or action is None:
                self.model_menu.add_command(label=label, state="disabled")
            elif action[0] == "Use":
                _, command = action
                self.model_menu.add_command(label=label, command=command)
            elif action[0] == "Download":
                _, command = action
                self.model_menu.add_command(label=f"{label} · download", command=command)
            else:
                _, command = action
                self.model_menu.add_command(label=f"{label} · add", command=command)
        self.model_menu.add_separator()
        self.model_menu.add_command(label="All model settings…", command=self.show_settings)
        try:
            x = self.model_picker.winfo_rootx()
            y = self.model_picker.winfo_rooty() + self.model_picker.winfo_height()
            self.model_menu.tk_popup(x, y)
        finally:
            self.model_menu.grab_release()

    def compose_attach(self):
        menu = tk.Menu(self.root, tearoff=0, bg=PANEL, fg=TEXT, activebackground=HOVER, activeforeground=TEXT)
        menu.add_command(label="Manage folders…", command=self.manage_library)
        menu.add_command(label="Model settings…", command=self.show_settings)
        try:
            x = self.compose_plus.winfo_rootx()
            y = self.compose_plus.winfo_rooty() + self.compose_plus.winfo_height()
            menu.tk_popup(x, y)
        finally:
            menu.grab_release()

    def sync_compose_bar(self):
        compose = bool(self._input_at_bottom or self.uses_conversation_shell())
        if compose == getattr(self, "_compose_bar", False):
            self.refresh_model_picker()
            return
        self._compose_bar = compose
        self.input_surface.set_compose(compose)
        if compose:
            self.search_icon.pack_forget()
            self.compose_plus.pack(side="left", padx=(0, 2), before=self.entry)
            self.model_picker.pack(side="right", padx=(6, 4), before=self.send_button)
            hint = "Send follow-up" if self.scope == "Chat" else "Ask a follow-up"
            self.placeholder.configure(text=hint)
        else:
            self.compose_plus.pack_forget()
            self.model_picker.pack_forget()
            self.search_icon.pack(side="left", padx=(0, 6), before=self.entry)
            self.placeholder.configure(text="Ask anything..." if self.scope == "Chat" else "Ask your files...")
        self.refresh_model_picker()
        self.update_placeholder()

    def refresh_theme(self):
        shell = windows_palette()
        dark = DEFAULT
        self.apply_theme((dark[0], dark[1], dark[2], dark[3], shell[4], dark[5]))
        self.theme_timer = self.root.after(3000, self.refresh_theme)

    def apply_theme(self, colours):
        global BG, PANEL, TEXT, MUTED, ACCENT, HOVER
        previous = (BG, PANEL, TEXT, MUTED, ACCENT, HOVER)
        if colours == previous:
            return
        replacements = dict(zip(previous, colours))
        def recolour(widget):
            for option in ("background", "foreground", "activebackground", "activeforeground",
                           "insertbackground", "highlightbackground", "highlightcolor",
                           "disabledbackground", "disabledforeground", "readonlybackground"):
                if option in widget.keys():
                    old = str(widget.cget(option))
                    if old in replacements:
                        widget.configure(**{option: replacements[old]})
            for child in widget.winfo_children():
                recolour(child)
        recolour(self.root)
        BG, PANEL, TEXT, MUTED, ACCENT, HOVER = colours
        self.canvas.configure(bg=BG)
        self.input_surface.set_outline(ACCENT)
        if self.busy:
            self.input_surface.set_busy(True)
        else:
            self.input_surface.set_busy(False)
        self.style_scopes()
        self.sync_entry_style()
        if self.busy:
            self.input_surface.set_busy(False)

    def library_summary(self):
        if self.demo:
            return "⬡  5 files · Sample"
        return f"⬡  {self.engine.count()} files · On device"

    def refresh_library_summary(self):
        if hasattr(self, "library_label"):
            self.library_label.configure(text=self.library_summary())

    def section_row(self, parent, title, link_text=None, link_command=None):
        row = tk.Frame(parent, bg=BG)
        row.pack(fill="x", pady=(4, 8))
        tk.Label(row, text=title, bg=BG, fg=TEXT, font=("Segoe UI", 11, "bold")).pack(side="left")
        if link_text and link_command:
            link = self.button(row, link_text, link_command, muted=True)
            link.configure(fg=ACCENT, font=("Segoe UI", 9))
            link.pack(side="right")

    def quick_action_card(self, parent, icon, title, subtitle, command):
        card = tk.Frame(parent, bg=CARD, padx=8, pady=6, cursor="hand2")
        top = tk.Frame(card, bg=CARD)
        top.pack(fill="x")
        icon_label = tk.Label(top, text=icon, bg=CARD, fg=ACCENT, font=("Segoe UI", 11))
        icon_label.pack(side="left", padx=(0, 6))
        text_col = tk.Frame(top, bg=CARD)
        text_col.pack(side="left", fill="x", expand=True)
        title_label = tk.Label(text_col, text=title, bg=CARD, fg=TEXT, font=("Segoe UI", 9, "bold"), anchor="w")
        title_label.pack(anchor="w")
        sub_label = tk.Label(text_col, text=subtitle, bg=CARD, fg=MUTED, font=("Segoe UI", 8), anchor="w")
        sub_label.pack(anchor="w")
        for widget in (card, top, icon_label, text_col, title_label, sub_label):
            widget.bind("<Button-1>", lambda _event: command())
        return card

    def accent_button(self, parent, text, command):
        return tk.Button(parent, text=text, command=command, bg=ACCENT, fg=BG,
                         activebackground=ACCENT, activeforeground=BG, relief="flat", bd=0,
                         padx=10, pady=4, cursor="hand2", font=("Segoe UI", 9, "bold"),
                         highlightthickness=0)

    def ghost_button(self, parent, text, command):
        return tk.Button(parent, text=text, command=command, bg=CARD, fg=ACCENT,
                         activebackground=HOVER, activeforeground=ACCENT, relief="flat", bd=0,
                         padx=10, pady=4, cursor="hand2", font=("Segoe UI", 9),
                         highlightthickness=1, highlightbackground=HOVER, highlightcolor=ACCENT)

    def model_entries(self, models_api):
        try:
            models = models_api.answer_models()
            selected = models_api.selected_answer() if models else None
        except (RuntimeError, KeyError, ValueError, OSError):
            models, selected = [], None
        installed = {model["file"]: model for model in models}
        entries = []
        for candidate in models_api.CATALOG:
            title, description, info = candidate
            filename = info[2]
            active = bool(selected and selected["file"] == filename)
            tags = []
            if active:
                tags.append("In use")
            elif filename in installed:
                tags.append("Ready")
            else:
                tags.extend(part.strip() for part in description.split("·"))
            action = None
            if active:
                action = None
            elif filename in installed:
                action = ("Use", lambda f=filename: self.select_answer_model(f))
            elif models_api.model_path(filename).is_file():
                action = ("Add", lambda c=candidate: self.add_existing_model(c))
            else:
                action = ("Download", lambda c=candidate: self.download_model(c))
            entries.append({"title": title, "tags": tags, "active": active, "action": action, "sort": 0 if active else 1})
        for model in models:
            if model["file"] in {item[2][2] for item in models_api.CATALOG}:
                continue
            title = model.get("title", model["file"])
            active = bool(selected and selected["file"] == model["file"])
            tags = ["In use"] if active else ["Ready"]
            action = None if active else ("Use", lambda f=model["file"]: self.select_answer_model(f))
            entries.append({"title": title, "tags": tags, "active": active, "action": action, "sort": 0 if active else 1})
        entries.sort(key=lambda item: (item["sort"], item["title"]))
        return entries

    def present_embedding_card(self, parent, models_api):
        embed = models_api.embedding_record() if hasattr(models_api, "embedding_record") else None
        if not embed:
            tk.Label(parent, text="File search embeddings are missing. Run setup_runtime.py before indexing.",
                     bg=BG, fg=ERROR, wraplength=480, justify="left", font=("Segoe UI", 9)).pack(anchor="w", pady=(0, 8))
            return
        ready = models_api.model_path(embed["file"]).is_file()
        shell = tk.Frame(parent, bg=BG)
        shell.pack(fill="x", pady=(0, 10))
        card = tk.Frame(shell, bg=CARD, highlightthickness=1, highlightbackground=HOVER)
        card.pack(fill="x")
        top = tk.Frame(card, bg=CARD)
        top.pack(fill="x", padx=12, pady=(10, 4))
        tk.Label(top, text="File search", bg=CARD, fg=MUTED, font=("Segoe UI", 9)).pack(side="left")
        tk.Label(top, text="·", bg=CARD, fg=MUTED, font=("Segoe UI", 9)).pack(side="left", padx=4)
        tk.Label(top, text="Nomic Embed v1.5", bg=CARD, fg=TEXT, font=("Segoe UI", 10, "bold")).pack(side="left")
        status = tk.Label(top, text="Ready" if ready else "Missing", bg="#1f3d2f" if ready else "#3d1f1f",
                          fg=SUCCESS if ready else ERROR, font=("Segoe UI", 8), padx=6, pady=2)
        status.pack(side="right")
        tk.Label(card, text=embed["file"], bg=CARD, fg=MUTED, font=("Segoe UI", 8),
                 anchor="w").pack(fill="x", padx=12, pady=(0, 10))

    def model_card(self, parent, title, tags, active, action):
        shell = tk.Frame(parent, bg=BG)
        shell.pack(fill="x", pady=(0, 8))
        border = ACCENT if active else HOVER
        card = tk.Frame(shell, bg=CARD, highlightthickness=2 if active else 1, highlightbackground=border)
        card.pack(fill="x")
        header = tk.Frame(card, bg=CARD)
        header.pack(fill="x", padx=12, pady=10)
        tk.Label(header, text=title, bg=CARD, fg=TEXT, font=("Segoe UI", 11, "bold"), anchor="w").pack(side="left")
        if active:
            tk.Label(header, text="In use", bg="#1f3d2f", fg=SUCCESS,
                     font=("Segoe UI", 8, "bold"), padx=8, pady=3).pack(side="right")
        elif action:
            label, command = action
            (self.accent_button if label in {"Use", "Download"} else self.ghost_button)(header, label, command).pack(side="right")
        visible_tags = [tag for tag in tags if tag != "In use"]
        if visible_tags:
            tag_row = tk.Frame(card, bg=CARD)
            tag_row.pack(fill="x", padx=12, pady=(0, 10))
            for tag in visible_tags:
                badge = tk.Label(tag_row, text=tag, bg=HOVER, fg=MUTED, font=("Segoe UI", 8), padx=6, pady=2)
                badge.pack(side="left", padx=(0, 6))

    def present_model_catalog(self, parent, show_embedding=False):
        if self.demo:
            self.section_row(parent, "Models")
            self.model_card(parent, "Qwen 2.5 · 1.5B", ("Suggested",), True, None)
            self.model_card(parent, "Qwen 2.5 · 0.5B", ("Smallest", "Less reliable citations"), False, None)
            return
        models_api = getattr(self.engine, "models", None)
        if models_api is None:
            return
        self.section_row(parent, "Models", "Add a GGUF model", self.import_model)
        if not getattr(models_api, "manifest", None):
            tk.Label(parent, text="Run setup_runtime.py to download local models.",
                     bg=BG, fg=MUTED, wraplength=480, justify="left", font=("Segoe UI", 9)).pack(anchor="w")
            return
        if show_embedding:
            self.present_embedding_card(parent, models_api)
        for entry in self.model_entries(models_api):
            self.model_card(parent, entry["title"], entry["tags"], entry["active"], entry["action"])
        tk.Label(parent, text="Answers use the selected model. File search keeps a separate embedding model.",
                 bg=BG, fg=MUTED, wraplength=480, justify="left", font=("Segoe UI", 8)).pack(anchor="w", pady=(2, 0))

    def present_home_models(self):
        self.present_model_catalog(self.results, show_embedding=False)

    def present_model_settings(self):
        self.present_model_catalog(self.results, show_embedding=True)

    def button(self, parent, text, command, muted=False):
        return tk.Button(parent, text=text, command=command, bg=parent.cget("bg"), fg=MUTED if muted else TEXT,
                         activebackground=HOVER, activeforeground=TEXT, relief="flat", bd=0,
                         padx=8, pady=3, cursor="hand2", font=("Segoe UI", 10),
                         highlightthickness=0, highlightbackground=BG, highlightcolor=ACCENT)

    def style_scopes(self):
        for name, button in self.scope_buttons.items():
            if name == self.scope:
                button.configure(bg=ACCENT, fg=BG, activebackground=ACCENT, activeforeground=BG,
                                 font=("Segoe UI", 9, "bold"))
            else:
                button.configure(bg=CARD, fg=MUTED, activebackground=HOVER, activeforeground=TEXT,
                                 font=("Segoe UI", 9))

    def focus_search(self):
        self.expand()
        self.entry.focus_set()

    def load_prefs(self):
        path = ROOT / "data" / "window.json"
        try:
            return json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return {}

    def save_prefs(self):
        if not self.use_settings:
            return
        path = ROOT / "data" / "window.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        payload = {"glass": self.glass, "opacity": self.opacity, "topmost": self.topmost}
        if self.origin is not None:
            payload["x"], payload["y"] = (int(self.origin[0]), int(self.origin[1]))
        path.write_text(json.dumps(payload), encoding="utf-8")

    def bind_drag(self, widget, on_click=None):
        widget.bind("<ButtonPress-1>", self.drag_start)
        widget.bind("<B1-Motion>", self.drag_move)
        widget.bind("<ButtonRelease-1>", lambda event, action=on_click: self.drag_end(event, action))

    def drag_start(self, event):
        self._press = (event.x_root, event.y_root, self.root.winfo_x(), self.root.winfo_y(), False)

    def drag_move(self, event):
        if not self._press:
            return
        x0, y0, wx, wy, _moved = self._press
        if abs(event.x_root - x0) + abs(event.y_root - y0) < 4:
            return
        self._press = (x0, y0, wx, wy, True)
        self.place_window(wx + event.x_root - x0, wy + event.y_root - y0, remember=False)

    def drag_end(self, event, on_click):
        moved = bool(self._press and self._press[4])
        self._press = None
        if moved:
            self.save_prefs()
        elif on_click:
            on_click()

    def place_dialog(self, window, width, height):
        self.root.update_idletasks()
        x = self.root.winfo_x()
        y = self.root.winfo_y() + max(self.root.winfo_height(), 48) + 8
        left, top, right, bottom = virtual_bounds(monitors())
        width = min(width, max(280, right - left - 40))
        height = min(height, max(180, bottom - top - 40))
        if y + height > bottom:
            y = max(top + 8, bottom - height - 8)
        if x + width > right:
            x = max(left + 8, right - width - 8)
        window.geometry(f"{int(width)}x{int(height)}+{int(max(left, x))}+{int(max(top, y))}")
        window.bind("<Escape>", lambda _event: window.destroy())

    def place_window(self, x, y, remember=True):
        screens = monitors()
        left, top, right, bottom = virtual_bounds(screens)
        width, height = max(self.root.winfo_width(), 40), max(self.root.winfo_height(), 40)
        x = int(min(max(x, left), right - min(80, width)))
        y = int(min(max(y, top), bottom - min(40, height)))
        self.origin = (x, y)
        self.root.geometry(f"+{x}+{y}")
        if remember:
            self.save_prefs()

    def move_to_screen(self, screen):
        width, height = self.root.winfo_width(), self.root.winfo_height()
        x = screen["left"] + max(0, (screen["right"] - screen["left"] - width) // 2)
        self.place_window(x, screen["top"])

    def set_glass(self, enabled):
        self.glass = enabled
        self.apply_material()
        self.save_prefs()
        self.show_settings()

    def set_opacity(self, value):
        self.opacity = float(value)
        if self.glass:
            self.root.attributes("-alpha", self.opacity)
        self.save_prefs()

    def set_topmost(self):
        self.topmost = not self.topmost
        self.root.attributes("-topmost", self.topmost)
        self.save_prefs()
        self.show_settings()

    def apply_material(self):
        self.root.update_idletasks()
        apply_window_material(self.root, self.root.winfo_width(), self.root.winfo_height(), self.glass, self.opacity)

    def show_settings(self):
        if self.busy and self.job_kind != "chat":
            self.note_phase("Finish or stop the current operation before changing settings.")
            return
        if self.busy and self.job_kind == "chat":
            self.note_phase("Models apply to your next message · wait or press Stop to switch mid-reply.")
        if self.library_window and self.library_window.winfo_exists():
            self.library_window.destroy()
        if not self.expanded:
            self.expand()
        self.reset_results_view()
        self.in_settings = True
        self.status.configure(text="Settings", fg=TEXT)
        self.present_model_settings()
        self.section_row(self.results, "Appearance")
        appearance = tk.Frame(self.results, bg=BG)
        appearance.pack(fill="x", pady=(2, 0))
        self.button(appearance, "Glass", lambda: self.set_glass(True)).pack(side="left", padx=(0, 4))
        self.button(appearance, "Solid", lambda: self.set_glass(False)).pack(side="left")
        for name, button in (("Glass", appearance.winfo_children()[0]), ("Solid", appearance.winfo_children()[1])):
            chosen = (name == "Glass") == self.glass
            button.configure(bg=ACCENT if chosen else BG, fg=BG if chosen else MUTED)
        if self.glass:
            scale = tk.Scale(self.results, from_=0.55, to=0.92, resolution=0.01, orient="horizontal",
                             bg=BG, fg=TEXT, troughcolor=PANEL, highlightthickness=0, length=240,
                             showvalue=False, command=self.set_opacity)
            scale.set(self.opacity)
            scale.pack(anchor="w", pady=(4, 0))
        staying = self.button(self.results, "Always on top" if self.topmost else "Not always on top", self.set_topmost, muted=not self.topmost)
        staying.pack(anchor="w", pady=(4, 0))
        screens = tk.Frame(self.results, bg=BG)
        screens.pack(fill="x", pady=(4, 0))
        for index, screen in enumerate(monitors(), 1):
            label = f"Screen {index}" + (" · primary" if screen["primary"] else "")
            self.button(screens, label, lambda s=screen: self.move_to_screen(s), muted=True).pack(side="left", padx=(0, 4))
        self.button(self.results, "← Back", self.leave_settings, muted=True).pack(anchor="w", pady=(8, 0))
        self.fit_content()
        self.sync_conversation_layout()

    def leave_settings(self):
        self.in_settings = False
        if self.thread:
            self.render_thread()
            self.sync_status_strip()
            return
        if self.scope == "Chat" and self.chat_thread:
            self.thread = list(self.chat_thread)
            self.render_thread()
            self.sync_status_strip()
            return
        if self.file_thread:
            self.thread = list(self.file_thread)
            self.render_thread()
            self.sync_status_strip()
            return
        self.show_welcome()

    def show_models(self):
        self.show_settings()

    def download_model(self, candidate):
        if self.busy:
            self.note_phase("An operation is running. Use Stop, then try again.")
            return
        models = getattr(self.engine, "models", None)
        if models is not None and not getattr(models, "manifest", None):
            self._status_error = True
            self.phase_text = "Local models are not installed. Run setup_runtime.py first."
            self.status.configure(text=self.phase_text, fg=ERROR)
            return
        if self.start_job(lambda cancel, emit: self.engine.models.download_answer(candidate, cancel, emit), kind="model"):
            self.show_stop()

    def add_existing_model(self, candidate):
        if self.busy:
            self.note_phase("An operation is running. Use Stop, then try again.")
            return
        filename = self.engine.models.model_path(candidate[2][2])
        if self.start_job(lambda cancel, emit: self.engine.models.import_answer(filename, cancel, emit), kind="model"):
            self.show_stop()

    def import_model(self):
        if self.busy:
            self.note_phase("An operation is running. Use Stop, then try again.")
            return
        models = getattr(self.engine, "models", None)
        if models is not None and not getattr(models, "manifest", None):
            self._status_error = True
            self.phase_text = "Local models are not installed. Run setup_runtime.py first."
            self.status.configure(text=self.phase_text, fg=ERROR)
            return
        filename = filedialog.askopenfilename(parent=self.root, title="Choose an answer model", filetypes=[("GGUF models", "*.gguf")])
        if filename and self.start_job(lambda cancel, emit: self.engine.models.import_answer(filename, cancel, emit), kind="model"):
            self.show_stop()

    def select_answer_model(self, filename):
        if self.busy:
            if self.scope == "Chat":
                self.note_phase("Wait for the reply or press Stop, then choose a model.")
            else:
                self.note_phase("An operation is running. Use Stop, then try again.")
            return
        try:
            self.engine.models.select_answer(filename)
        except (OSError, ValueError, RuntimeError) as exc:
            self._status_error = True
            self.phase_text = str(exc)
            self.status.configure(text=str(exc), fg=ERROR)
            return
        if self.in_settings:
            self.show_settings()
        elif self.conversation_active() or self.thread or self.chat_thread or self.file_thread:
            self.render_thread()
            self.sync_conversation_layout()
            self.sync_status_strip()
            self.size_results_viewport()
        else:
            self.show_welcome()
        self.status.configure(text="Saved · this model will answer your next question.", fg=ACCENT)
        self.refresh_model_picker()

    def uses_conversation_shell(self):
        return self.scope == "Chat" or bool(self.thread) or bool(self.pending_chat)

    def conversation_active(self):
        if not self.expanded or self.in_settings:
            return False
        return self.uses_conversation_shell()

    def pin_window_height(self):
        if self._pinned_height is not None:
            return
        screens = monitors()
        home = next(screen for screen in screens if screen["primary"])
        limit = home["bottom"] - home["top"] - 48
        self._pinned_height = min(CONVERSATION_HEIGHT, max(520, limit))

    def sync_conversation_layout(self):
        if not self.expanded:
            return
        bottom = self.uses_conversation_shell() or (self.in_settings and self._input_at_bottom)
        if bottom != self._input_at_bottom:
            self._input_at_bottom = bottom
            for widget in (self.input_surface, self.scopes, self.status_row, self.scroll_host, self.footer):
                widget.pack_forget()
            self.header.pack(fill="x")
            if bottom:
                self.scopes.pack(fill="x", pady=(8, 6))
                self.status_row.pack(fill="x", pady=(0, 4))
                self.scroll_host.pack(fill="both", expand=True)
                self.input_surface.pack(fill="x", pady=(6, 8))
                self.footer.pack(fill="x")
            else:
                self.input_surface.pack(fill="x", pady=(10, 8))
                self.scopes.pack(fill="x", pady=(0, 8))
                self.status_row.pack(fill="x", pady=(0, 4))
                self.scroll_host.pack(fill="x")
                self.footer.pack(fill="x", pady=(10, 0))
        self.sync_compose_bar()

    def layout(self, width, height):
        screens = monitors()
        if self.origin is not None and not on_screen(self.origin[0], self.origin[1], screens):
            self.origin = None
        if self.origin is None:
            home = next(screen for screen in screens if screen["primary"])
            width = min(width, home["right"] - home["left"] - 20)
            height = min(height, home["bottom"] - home["top"] - 20)
            x = home["left"] + (home["right"] - home["left"] - width) // 2
            y = home["top"]
        else:
            x, y = self.origin
            width = min(width, 1200)
            height = min(height, 900)
        self.root.geometry(f"{int(width)}x{int(height)}+{int(x)}+{int(y)}")
        body_h = max(1, int(height) - 42)
        self.canvas.itemconfigure(self.body_id, width=int(width) - 48, height=body_h)
        self.canvas.configure(bg=BG)
        self.apply_material()

    def fit_content(self):
        if self.closed or not self.expanded:
            return
        try:
            if not self.root.winfo_exists():
                return
        except tk.TclError:
            return
        self.cancel_animation()
        if self.in_settings:
            height = self._pinned_height or max(self.body.winfo_reqheight() + 42, 520)
            self.layout(560, height)
            self.finish_transition()
            return
        self.canvas.itemconfigure(self.body_id, width=512)
        self.root.update_idletasks()
        if self.conversation_active():
            self.pin_window_height()
            self.sync_conversation_layout()
            self.root.update_idletasks()
            self.size_results_viewport()
            self.layout(560, self._pinned_height)
        else:
            if self._input_at_bottom:
                self._input_at_bottom = False
                self.sync_conversation_layout()
            self.size_results_viewport()
            self.root.update_idletasks()
            self.layout(560, self.body.winfo_reqheight() + 42)
        self.finish_transition()

    def size_results_viewport(self):
        """Grow the results area with content, or fill a fixed panel during a conversation."""
        self.root.update_idletasks()
        inner = max(self.scroll_inner.winfo_reqheight(), 1)
        width = max(self.scroll.winfo_width(), 480)
        self.scroll.itemconfigure(self.scroll_window, width=width)
        self.scroll.configure(scrollregion=(0, 0, width, inner))
        if self.conversation_active():
            self.body.update_idletasks()
            host_h = max(self.scroll_host.winfo_height(), 140)
            self.scroll.configure(height=host_h)
            if self.thread:
                if not self.vbar.winfo_ismapped():
                    self.vbar.pack(side="right", fill="y")
            elif inner > host_h + 2:
                if not self.vbar.winfo_ismapped():
                    self.vbar.pack(side="right", fill="y")
            elif self.vbar.winfo_ismapped():
                self.vbar.pack_forget()
            return
        chrome = max(self.body.winfo_reqheight() - int(self.scroll.cget("height")), 0)
        screens = monitors()
        if self.origin is None:
            home = next(screen for screen in screens if screen["primary"])
            limit = home["bottom"] - home["top"] - 20
        else:
            limit = 900
        viewport = max(1, min(inner, max(80, limit - 42 - chrome)))
        self.scroll.configure(height=viewport)
        width = max(self.scroll.winfo_width(), 480)
        self.scroll.itemconfigure(self.scroll_window, width=width)
        self.scroll.configure(scrollregion=(0, 0, width, inner))
        overflowing = inner > viewport + 2
        if overflowing and not self.vbar.winfo_ismapped():
            self.vbar.pack(side="right", fill="y")
        elif not overflowing and self.vbar.winfo_ismapped():
            self.vbar.pack_forget()
            self.scroll.yview_moveto(0)

    def _sync_scroll_width(self, event=None):
        if event is not None and event.width > 1:
            self.scroll.itemconfigure(self.scroll_window, width=event.width)

    def _on_results_scroll(self, first, last):
        self.vbar.set(first, last)

    def _pointer_over_results(self):
        widget = self.root.winfo_containing(self.root.winfo_pointerx(), self.root.winfo_pointery())
        while widget is not None:
            if widget == self.scroll_host:
                return True
            widget = getattr(widget, "master", None)
        return False

    def _on_results_wheel(self, event):
        if not self.expanded or not self.vbar.winfo_ismapped() or not self._pointer_over_results():
            return
        if getattr(event, "num", None) == 4 or getattr(event, "delta", 0) > 0:
            self.scroll.yview_scroll(-1, "units")
        else:
            self.scroll.yview_scroll(1, "units")
        return "break"

    def cancel_animation(self):
        if self.animation_timer is not None:
            self.root.after_cancel(self.animation_timer)
            self.animation_timer = None

    def finish_transition(self):
        self.canvas.itemconfigure(self.compact_id, state="hidden" if self.expanded else "normal")
        self.canvas.itemconfigure(self.body_id, state="normal" if self.expanded else "hidden")
        if self.expanded:
            self.entry.focus_set()

    def looks_like_status(self, text):
        text = (text or "").strip()
        return not text or any(text.startswith(prefix) for prefix in (
            "Local answer", "Local chat", "Local reply", "Loading ", "Searching ", "Reading ",
            "Ready ·", "An operation is running",
        ))

    def sync_status_strip(self):
        """Keep long status lines out of the strip once the thread carries the story."""
        if self.busy:
            return
        if self.thread or self.pending_chat:
            if not self._status_error:
                self.status.configure(text="", fg=MUTED)
            return
        if self.scope == "Chat" and not self.thread:
            return

    def chat_context(self):
        turns = []
        for turn in self.thread:
            if turn["role"] in ("user", "assistant"):
                turns.append({"role": turn["role"], "content": turn["text"]})
        return turns[-6:]

    def paint_bubble(self, parent, role, text, *, fg=None, bg=None):
        row = tk.Frame(parent, bg=BG)
        row.pack(fill="x", pady=(8 if role == "user" else 4, 2))
        side = "e" if role == "user" else "w"
        margin = (36, 10) if role == "user" else (10, 36)
        wrap = tk.Frame(row, bg=BG)
        wrap.pack(anchor=side, padx=margin)
        bubble_bg = bg or (ACCENT if role == "user" else BUBBLE_ASSIST)
        bubble_fg = fg or ("#ffffff" if role == "user" else TEXT)
        pad = 14
        font = ("Segoe UI", 11)
        outer = tk.Canvas(wrap, bg=BG, highlightthickness=0, borderwidth=0, height=28, width=64)
        outer.pack(anchor=side)
        text_id = bubble_text_id(outer, text, pad, bubble_fg, font)

        class BubbleHandle:
            def __init__(self, canvas, tid, fill, app, padding):
                self.canvas = canvas
                self.text_id = tid
                self.fill = fill
                self.app = app
                self.pad = padding

            def winfo_exists(self):
                try:
                    return self.canvas.winfo_exists()
                except tk.TclError:
                    return False

            def configure(self, *, text=None, fg=None):
                if text is not None:
                    self.canvas.itemconfigure(self.text_id, text=text)
                if fg is not None:
                    self.canvas.itemconfigure(self.text_id, fill=fg)
                self._draw()

            def _draw(self):
                self.canvas.update_idletasks()
                box = self.canvas.bbox(self.text_id)
                if not box:
                    return
                x1, y1, x2, y2 = box
                w = max(int(x2 - x1 + self.pad * 2), 32)
                h = max(int(y2 - y1 + self.pad * 2), 24)
                self.canvas.configure(width=w, height=h)
                self.canvas.delete("bubble")
                photo = smooth_round_photo(self.canvas, w, h, BUBBLE_RADIUS, self.fill)
                if photo is not None:
                    self.canvas._loft_photo = photo
                    self.app._bubble_photos.append(photo)
                    self.canvas.create_image(0, 0, anchor="nw", image=photo, tags="bubble")
                else:
                    canvas_round_rect(self.canvas, 0, 0, w, h, BUBBLE_RADIUS, self.fill, tags="bubble")
                self.canvas.coords(self.text_id, self.pad, self.pad)
                self.canvas.tag_raise(self.text_id)

        handle = BubbleHandle(outer, text_id, bubble_bg, self, pad)
        handle._draw()
        return handle

    def paint_sources(self, parent, hits, *, indent=True):
        if not hits:
            return
        pad = (18, 0) if indent else (0, 0)
        block = tk.Frame(parent, bg=BG)
        block.pack(fill="x", pady=(0, 6), padx=pad)
        self.sources_detail = block
        self.sources_open = True
        for hit in hits:
            self.source_card(block, hit)

    def reset_results_view(self):
        self.answer_card = None
        self.answer_text = None
        self.support_note = None
        self.answer_heading = None
        self.answer_body = None
        self.caution_frame = None
        self.caution_label = None
        self.sources_wrap = None
        self.progress_track = None
        self.answer_skeleton = None
        self.live_assistant_label = None
        previous = self.results
        self.results = tk.Frame(self.scroll_inner, bg=BG, height=1)
        self.results.pack(fill="x", before=previous)
        previous.destroy()

    def scroll_thread_to_end(self):
        if not getattr(self, "scroll", None):
            return
        try:
            self.root.update_idletasks()
            self.scroll.yview_moveto(1)
        except tk.TclError:
            pass

    def clear_results(self):
        self.answer_buffer = ""
        self.support_gaps = []
        self.reset_results_view()

    def render_thread(self):
        self._bubble_photos = []
        self.reset_results_view()
        if self.scope == "Chat" and not self.thread and not self.pending_chat and not self.busy:
            tk.Label(self.results, text="Ask anything — this chat is not grounded in your indexed files.",
                     bg=BG, fg=MUTED, wraplength=460, justify="left", anchor="w",
                     font=("Segoe UI", 10)).pack(anchor="w", pady=(8, 12))
        for turn in self.thread[-10:]:
            if turn["role"] == "assistant" and self.looks_like_status(turn.get("text", "")):
                continue
            if turn["role"] == "user":
                self.paint_bubble(self.results, "user", turn["text"])
            else:
                group = tk.Frame(self.results, bg=BG)
                group.pack(fill="x")
                self.paint_bubble(group, "assistant", turn["text"])
                if turn.get("hits"):
                    self.paint_sources(group, turn["hits"])
                if turn.get("gaps"):
                    tk.Label(group, text=self.support_line_for(turn["gaps"]), bg=BG, fg=MUTED,
                             wraplength=BUBBLE_WRAP + 40, justify="left", anchor="w",
                             font=("Segoe UI", 9)).pack(anchor="w", padx=(18, 36), pady=(2, 6))
        if self.scope == "Chat" and self.pending_chat:
            self.paint_bubble(self.results, "user", self.pending_chat)
        if self.busy and (self.job_kind == "ask" or (self.job_kind == "chat" and self.pending_chat)):
            live = tk.Frame(self.results, bg=BG)
            live.pack(fill="x")
            assistant = self.answer_buffer.strip()
            if not assistant:
                assistant = self.phase_text if self.job_kind == "chat" else "Thinking…"
            self.live_assistant_label = self.paint_bubble(
                live, "assistant", assistant,
                fg=MUTED if not self.answer_buffer.strip() else TEXT)
            if self.last_hits and self.job_kind == "ask":
                self.paint_sources(live, self.last_hits)
            if self.support_gaps and self.live_assistant_label:
                tk.Label(live, text=self.support_line(), bg=BG, fg=MUTED, wraplength=BUBBLE_WRAP + 40,
                         justify="left", anchor="w", font=("Segoe UI", 9)).pack(anchor="w", padx=(18, 36), pady=(2, 4))
        elif self.awaiting_empty_note and self.phase_text and not self.looks_like_status(self.phase_text):
            self.paint_bubble(self.results, "assistant", self.phase_text, fg=MUTED)
        if self.answer_buffer.strip() and self.busy:
            self.button(self.results, "Read full answer", self.show_answer, muted=True).pack(anchor="w", padx=(18, 0), pady=(0, 6))
        self.fit_content()
        self.sync_status_strip()
        self.sync_conversation_layout()
        self.root.after_idle(self.scroll_thread_to_end)

    def assistant_turn_text(self):
        text = self.answer_buffer.strip()
        if text and not self.looks_like_status(text):
            return text
        if self.awaiting_empty_note or self._status_error:
            phase = (self.phase_text or "").strip()
            if phase and not self.looks_like_status(phase):
                return phase
            return self.phase_text or "No matching files."
        return ""

    def finalize_file_turn(self):
        note = self.assistant_turn_text()
        if self.looks_like_status(note):
            note = ""
        if note or self.last_hits:
            self.thread.append({
                "role": "assistant",
                "text": note or ("See the sources below." if self.last_hits else "No matching files."),
                "hits": list(self.last_hits),
                "gaps": list(self.support_gaps),
            })
        self.awaiting_empty_note = False
        self.live_assistant_label = None
        self.answer_card = None
        self.answer_text = None
        self.render_thread()
        self.sync_status_strip()
        if self.scope != "Chat":
            self.file_thread = list(self.thread)
        self.sync_conversation_layout()

    def transition(self, width, height):
        self.cancel_animation()
        if not self.motion_ready or not motion_enabled():
            self.layout(width, height)
            self.finish_transition()
            return
        start_width, start_height = self.root.winfo_width(), self.root.winfo_height()
        started = time.perf_counter()
        self.canvas.itemconfigure(self.body_id, state="hidden")
        self.canvas.itemconfigure(self.compact_id, state="hidden")
        def frame():
            self.animation_timer = None
            progress = min(1.0, (time.perf_counter()-started)/0.24)
            eased = 1 - (1-progress)**3
            self.layout(round(start_width+(width-start_width)*eased),
                        round(start_height+(height-start_height)*eased))
            if progress < 1:
                self.animation_timer = self.root.after(16, frame)
            else:
                self.finish_transition()
        frame()

    def expand(self):
        if self.expanded:
            return
        self.expanded = True
        self.canvas.itemconfigure(self.body_id, width=512)
        self.root.update_idletasks()
        self.sync_conversation_layout()
        self.size_results_viewport()
        self.root.update_idletasks()
        if self.conversation_active() and self._pinned_height:
            target = self._pinned_height
        else:
            target = self.body.winfo_reqheight() + 42
        self.transition(560, target)
        self.root.lift()

    def collapse(self):
        self.expanded = False
        self.transition(285, 48)

    def present_card(self, caption, text):
        card = tk.Frame(self.results, bg=HOVER, padx=12, pady=8)
        card.pack(fill="x")
        tk.Label(card, text=caption, bg=HOVER, fg=ACCENT, anchor="w",
                 font=("Segoe UI", 9)).pack(anchor="w")
        self.mount_rich(card, text)

    def present_streaming(self):
        self.answer_card = tk.Frame(self.results, bg=CARD, highlightthickness=1, highlightbackground=GLOW_SOFT)
        self.answer_card.pack(fill="x", pady=(0, 6))
        self.answer_heading = tk.Label(self.answer_card, text="Answer", bg=CARD, fg=TEXT, anchor="w",
                                       font=("Segoe UI", 10, "bold"))
        self.answer_heading.pack(anchor="w", padx=12, pady=(10, 0))
        self.answer_body = tk.Frame(self.answer_card, bg=CARD)
        self.answer_body.pack(fill="x", padx=12, pady=(4, 0))
        self.answer_buffer = ""
        self.answer_text = tk.Label(self.answer_body, text="Writing an answer…", bg=CARD, fg=MUTED, wraplength=460,
                                    justify="left", anchor="w", font=("Segoe UI", 11), cursor="hand2")
        self.answer_text.pack(anchor="w", fill="x")
        self.answer_text.bind("<Button-1>", lambda _event: self.show_answer())
        self.progress_track = tk.Canvas(self.answer_body, height=8, bg=CARD, highlightthickness=0)
        self.progress_track.pack(fill="x", pady=(10, 6))
        self.progress_track.bind("<Configure>", lambda _event: self.draw_answer_progress())
        self.answer_skeleton = tk.Frame(self.answer_body, bg=CARD)
        self.answer_skeleton.pack(fill="x", pady=(0, 4))
        for width in (260, 200, 140):
            bar = tk.Frame(self.answer_skeleton, bg=HOVER, height=7, width=width)
            bar.pack(anchor="w", pady=3)

    def ensure_caution_banner(self):
        line = self.support_line()
        if not line:
            if self.caution_frame is not None and self.caution_frame.winfo_exists():
                self.caution_frame.pack_forget()
            return
        if self.caution_frame is None or not self.caution_frame.winfo_exists():
            self.caution_frame = tk.Frame(self.results, bg="#2a2218", highlightthickness=1,
                                          highlightbackground="#5c4030")
            tk.Label(self.caution_frame, text="!", bg="#2a2218", fg=ERROR,
                     font=("Segoe UI", 10, "bold"), width=2).pack(side="left", padx=(8, 0), pady=8)
            self.caution_label = tk.Label(self.caution_frame, text=line, bg="#2a2218", fg=ERROR, wraplength=440,
                                            justify="left", anchor="w", font=("Segoe UI", 9))
            self.caution_label.pack(side="left", fill="x", expand=True, padx=(4, 10), pady=8)
        else:
            self.caution_label.configure(text=line)
        if self.sources_wrap is not None and self.sources_wrap.winfo_exists():
            self.caution_frame.pack(fill="x", pady=(0, 6), before=self.sources_wrap)
        elif self.answer_card is not None and self.answer_card.winfo_exists():
            self.caution_frame.pack(fill="x", pady=(0, 6), after=self.answer_card)

    def support_line(self):
        if not self.support_gaps:
            return ""
        return self.support_line_for(self.support_gaps)

    def support_line_for(self, gaps):
        if not gaps:
            return ""
        if len(gaps) > 4:
            return f"Some terms aren’t supported by the cited passage ({len(gaps)} flagged)."
        return "Not in the cited passage: " + ", ".join(gaps)

    def mark_unsupported(self, box, answer):
        if not self.support_gaps or not answer:
            return
        box.tag_configure("gap", foreground=ERROR, underline=True)
        wanted = {gap.lower() for gap in self.support_gaps}
        for match in re.finditer(r"\w+", answer):
            if match.group().lower() in wanted:
                box.tag_add("gap", f"1.0+{match.start()}c", f"1.0+{match.end()}c")

    def show_support_flag(self):
        line = self.support_line()
        if not line:
            return
        body = self.answer_body
        card = self.answer_card
        if body is not None and body.winfo_exists():
            current = self.answer_text
            if isinstance(current, tk.Label) and current.winfo_exists():
                current.destroy()
                self.answer_text = self.mount_flagged_answer(body, self.answer_buffer)
        self.ensure_caution_banner()
        if self.support_note is not None and self.support_note.winfo_exists():
            self.support_note.destroy()
            self.support_note = None
        window = self.answer_window
        if window is not None and window.winfo_exists() and getattr(self, "full_answer", None):
            window.title("Loft · Local answer · not fully supported")
            self.full_answer.configure(state="normal")
            self.mark_unsupported(self.full_answer, self.answer_buffer)
            self.full_answer.configure(state="disabled")
            if not getattr(self, "answer_gap_note", None) or not self.answer_gap_note.winfo_exists():
                self.answer_gap_note = tk.Label(window, text=line, bg=BG, fg=ERROR, wraplength=640,
                                                 justify="left", anchor="w", font=("Segoe UI", 10), padx=16, pady=8)
                self.answer_gap_note.pack(anchor="w", before=self.full_answer)
            else:
                self.answer_gap_note.configure(text=line)

    def mount_flagged_answer(self, parent, answer):
        lines = max(1, answer.count("\n") + (len(answer) // 55) + 1)
        box = tk.Text(parent, height=min(lines, 8), wrap="word", bg=CARD, fg=TEXT, relief="flat",
                      font=("Segoe UI", 11), highlightthickness=0, takefocus=0, cursor="hand2", padx=0, pady=0)
        box.pack(anchor="w", fill="x", pady=(4, 0))
        box.insert("1.0", answer)
        self.mark_unsupported(box, answer)
        box.configure(state="disabled")
        box.bind("<Button-1>", lambda _event: self.show_answer())
        return box

    def finish_answer(self):
        card = self.answer_card
        if card is None or not card.winfo_exists() or not self.answer_buffer:
            return
        if "```" in self.answer_buffer:
            for child in list(self.answer_body.winfo_children()):
                child.destroy()
            self.mount_rich(self.answer_body, self.answer_buffer)
            self.answer_text = None
            self.support_note = None
        self.show_support_flag()

    def parts(self, text):
        if "```" not in text:
            if re.match(r"\s*(def |class )", text):
                return [("code", text, "python")]
            return [("text", text)]
        pattern = re.compile(r"```([^\n`]*)\n?(.*?)```", re.S)
        chunks, last = [], 0
        for match in pattern.finditer(text):
            if match.start() > last:
                chunks.append(("text", text[last:match.start()]))
            chunks.append(("code", match.group(2), match.group(1).strip() or "code"))
            last = match.end()
        if last < len(text):
            chunks.append(("text", text[last:]))
        return chunks or [("text", text)]

    def mount_rich(self, parent, text):
        surface = parent.cget("bg")
        for kind, *rest in self.parts(text):
            if kind == "text":
                body = rest[0].strip()
                if body:
                    tk.Label(parent, text=body, bg=surface, fg=TEXT, wraplength=460, justify="left",
                             anchor="w", font=("Segoe UI", 11)).pack(anchor="w", fill="x", pady=(4, 0))
            else:
                self.add_code_block(parent, rest[0], rest[1])

    def source_preview(self, parent, hit):
        text = hit["text"].strip()
        path = Path(hit["path"])
        if path.suffix.lower() in CODE or re.match(r"\s*(def |class |import |#|//)", text):
            self.add_code_block(parent, text, path.suffix.lstrip(".") or "code")
            return
        preview = text if len(text) <= 320 else text[:317] + "…"
        tk.Label(parent, text=preview, bg=BG, fg=MUTED, wraplength=460, justify="left",
                 anchor="w", font=("Segoe UI", 9)).pack(anchor="w")

    def source_card(self, parent, hit):
        row = tk.Frame(parent, bg=BG)
        row.pack(fill="x", pady=(2, 0))
        tk.Label(row, text=hit["citation"], bg=ACCENT, fg=BG, font=("Segoe UI", 7, "bold"),
                 padx=4, pady=1).pack(side="left", padx=(12, 0))
        tk.Label(row, text=Path(hit["path"]).name, bg=BG, fg=TEXT,
                 font=("Segoe UI", 9)).pack(side="left", padx=(8, 0))
        tk.Label(row, text=f"· {hit['start']}–{hit['end']}", bg=BG, fg=MUTED,
                 font=("Segoe UI", 8)).pack(side="left", padx=(4, 0))
        actions = tk.Frame(row, bg=BG)
        actions.pack(side="right", padx=(0, 12))
        preview_body = tk.Frame(row, bg=BG)
        shown = {"open": False}

        def toggle_preview(button):
            if shown["open"]:
                preview_body.pack_forget()
                shown["open"] = False
                button.configure(text="Show me")
            else:
                preview_body.pack(fill="x", padx=(36, 12), pady=(4, 2))
                shown["open"] = True
                button.configure(text="Hide")
            self.fit_content()
            self.root.after_idle(self.scroll_thread_to_end)

        show = self.button(actions, "Show me", lambda: None, muted=True)
        show.configure(command=lambda: toggle_preview(show))
        show.pack(side="left", padx=(0, 6))
        self.button(actions, "Open", lambda h=hit: self.open_source(h), muted=True).pack(side="left")
        self.source_preview(preview_body, hit)

    def present_sources(self, hits):
        self.sources_open = True
        self.sources_wrap = tk.Frame(self.results, bg=BG)
        self.sources_wrap.pack(fill="x", pady=(0, 4))
        header = tk.Frame(self.sources_wrap, bg=BG)
        header.pack(fill="x")
        tk.Label(header, text="Sources", bg=BG, fg=TEXT, font=("Segoe UI", 10, "bold")).pack(side="left")
        tk.Label(header, text=str(len(hits)), bg=HOVER, fg=MUTED, font=("Segoe UI", 8),
                 padx=6, pady=1).pack(side="left", padx=(6, 0))
        self.sources_toggle = self.button(header, "Hide", self.toggle_sources, muted=True)
        self.sources_toggle.pack(side="right")
        self.sources_detail = tk.Frame(self.sources_wrap, bg=BG)
        self.sources_detail.pack(fill="x", pady=(6, 0))
        for hit in hits:
            self.source_card(self.sources_detail, hit)
        if self.support_gaps:
            self.ensure_caution_banner()

    def add_code_block(self, parent, code, language):
        code = code.strip("\n")
        block = tk.Frame(parent, bg=CARD)
        block.pack(fill="x", pady=(6, 0))
        bar = tk.Frame(block, bg=CARD)
        bar.pack(fill="x")
        tk.Label(bar, text=language or "code", bg=CARD, fg=MUTED,
                 font=("Segoe UI", 8)).pack(side="left", padx=(8, 0))
        copy = self.button(bar, "Copy", lambda: None, muted=True)
        copy.configure(command=lambda c=code, b=copy: self.copy_code(c, b))
        copy.pack(side="right")
        lines = code.splitlines() or [""]
        box = tk.Text(block, height=min(len(lines), 6), width=48, bg=BG, fg=TEXT, relief="flat",
                      bd=0, padx=8, pady=4, wrap="word", font=("Consolas", 10),
                      takefocus=0, highlightthickness=0)
        box.pack(fill="x", padx=8, pady=(0, 6))
        self.highlight(box, code)
        box.configure(state="disabled")

    def highlight(self, box, code):
        box.insert("1.0", code)
        box.tag_configure("kw", foreground="#8ab4f8")
        box.tag_configure("str", foreground="#9ece6a")
        box.tag_configure("com", foreground=MUTED)
        for line_no, line in enumerate(code.splitlines(), 1):
            for match in re.finditer(r"#.*$", line):
                box.tag_add("com", f"{line_no}.{match.start()}", f"{line_no}.{match.end()}")
            for match in re.finditer(r"(\"(?:\\.|[^\"\\])*\"|'(?:\\.|[^'\\])*')", line):
                box.tag_add("str", f"{line_no}.{match.start()}", f"{line_no}.{match.end()}")
            for match in re.finditer(r"\b[A-Za-z_][A-Za-z0-9_]*\b", line):
                if match.group() in CODE_WORDS:
                    box.tag_add("kw", f"{line_no}.{match.start()}", f"{line_no}.{match.end()}")

    def copy_code(self, code, button):
        try:
            self.root.clipboard_clear()
            self.root.clipboard_append(code)
            self.root.update_idletasks()
        except tk.TclError:
            self.status.configure(text="Could not copy to the clipboard.", fg=ERROR)
            return
        button.configure(text="Copied")
        self.root.after(1200, lambda: button.configure(text="Copy") if button.winfo_exists() else None)

    def toggle_sources(self):
        self.sources_open = not self.sources_open
        if self.sources_open:
            self.sources_detail.pack(fill="x", pady=(6, 0))
            self.sources_toggle.configure(text="Hide")
        else:
            self.sources_detail.pack_forget()
            self.sources_toggle.configure(text=f"Show {len(self.last_hits)}")
        self.fit_content()

    def show_welcome(self):
        self.in_settings = False
        self._pinned_height = None
        self._input_at_bottom = False
        self.thread = []
        self.file_thread = []
        self.chat_thread = []
        self.clear_results()
        if not self.demo:
            self._status_error = False
            self.status.configure(
                text="Your files, ready when you are." if self.engine.count() else "Add a folder to get started.",
                fg=MUTED)
        else:
            self.status.configure(text="Try a sample question or pick a quick action.", fg=MUTED)
        self.section_row(self.results, "Quick Actions")
        grid = tk.Frame(self.results, bg=BG)
        grid.pack(fill="x")
        grid.columnconfigure(0, weight=1)
        grid.columnconfigure(1, weight=1)
        actions = [
            ("📄", "Summarise", "Get a quick summary",
             lambda: self.quick_prompt("Summarise ", "Documents")),
            ("</>", "Explain code", "Ask about functions",
             lambda: self.quick_prompt("Explain ", "Code")),
            ("🔎", "Search files", "Find in your files",
             lambda: self.quick_prompt("", "All")),
            ("📁", "Explore folders", "Browse and understand", self.manage_library),
        ]
        if self.demo:
            actions[0] = ("📄", "Summarise", "Get a quick summary",
                          lambda: self.ask("Summarise the parking receipt."))
            actions[1] = ("</>", "Explain code", "Ask about functions",
                          lambda: self.ask("Where is login handled?"))
            actions[2] = ("🔎", "Search files", "Find in your files",
                          lambda: self.ask("Have I paid for parking today?"))
            actions[3] = ("📁", "Explore folders", "Browse sample files",
                          lambda: self.ask("Find the invoice for my laptop."))
        for index, (icon, title, subtitle, command) in enumerate(actions):
            card = self.quick_action_card(grid, icon, title, subtitle, command)
            card.grid(row=index // 2, column=index % 2, sticky="nsew",
                      padx=(0 if index % 2 == 0 else 3, 3 if index % 2 == 0 else 0),
                      pady=(0, 4))
        self.present_home_models()
        self.fit_content()

    def quick_prompt(self, prefix, scope):
        if self.busy and not self.demo:
            self.note_phase("An operation is running. Use Stop, then try again.")
            return
        self.question.set("")
        self.select_scope(scope)
        self.question.set(prefix)
        self.entry.focus_set()
        self.update_placeholder()
        if prefix:
            self.entry.icursor("end")

    def ask(self, question):
        self.question.set(question)
        self.search()

    def select_scope(self, scope):
        if scope == self.scope:
            return
        if self.busy and not self.demo:
            self.note_phase("An operation is running. Use Stop, then change mode.")
            return
        previous = self.scope
        self.scope = scope
        self.style_scopes()
        self.placeholder.configure(text="Ask anything..." if scope == "Chat" else "Ask your files...")
        if scope == "Chat":
            if previous != "Chat":
                self.file_thread = list(self.thread)
            self.thread = list(self.chat_thread)
            self._status_error = False
            self.pending_chat = ""
            self.answer_buffer = ""
            self.pin_window_height()
            self.sync_conversation_layout()
            self.render_thread()
            self.note_phase("Chat doesn't use your files. Press Enter to send.")
            self.fit_content()
            self.sync_compose_bar()
            return
        if previous == "Chat":
            self.chat_thread = list(self.thread)
            self.thread = list(self.file_thread)
            self.render_thread()
            self.fit_content()
            self.sync_compose_bar()
        if self.question.get().strip():
            self.search()

    def search(self):
        if not self.demo:
            if self.busy:
                self.note_phase("An operation is running. Use Stop, then try again.")
                return
            question = self.question.get().strip()
            if not question:
                self.show_welcome()
                return
            if self.answer_window and self.answer_window.winfo_exists():
                self.answer_window.destroy()
            scope = self.scope
            if scope == "Chat":
                self.pending_chat = question
                self.question.set("")
                self.update_placeholder()
                self.render_thread()
                history = self.chat_context()
                operation = lambda cancel, emit, history=history: self.engine.chat_question(question, cancel, emit, history)
                kind, phase = "chat", "Loading the answer model…"
            else:
                self.thread.append({"role": "user", "text": question})
                self.question.set("")
                self.update_placeholder()
                self.answer_buffer = ""
                self.last_hits = []
                self.support_gaps = []
                self.render_thread()
                operation = lambda cancel, emit: self.engine.ask(question, scope, cancel, emit)
                kind, phase = "ask", "Loading the search model…"
            if self.start_job(operation, kind=kind):
                self.show_stop()
                self.note_phase(phase)
            return
        self.root.after_idle(self.fit_content)
        question = self.question.get().strip()
        if not question:
            self.last_hits = []
            self.show_welcome()
            return
        self.clear_results()
        try:
            self.last_hits = self.searcher.search(question, self.scope)
        except (ValueError, RuntimeError) as exc:
            self.status.configure(text=f"Search could not finish: {exc}")
            return
        if not self.last_hits:
            self.status.configure(text="Nothing here yet.")
            tk.Label(self.results, text="Try parking, an invoice, or login code.\nThis preview searches five sample files.",
                     bg=BG, fg=MUTED, font=("Segoe UI", 11), justify="left").pack(anchor="w", pady=(4, 12))
            return
        self.status.configure(text="From your sample library")
        lead = self.last_hits[0]
        self.present_card(f"{Path(lead['path']).name} · lines {lead['start']}–{lead['end']}", lead["text"])
        self.present_sources(self.last_hits)

    def open_source(self, hit):
        if self.source_window and self.source_window.winfo_exists():
            self.source_window.destroy()
        window = self.source_window = tk.Toplevel(self.root)
        window.title(f"Loft · {Path(hit['path']).name} · {'sample' if self.demo else 'indexed'} passage")
        window.configure(bg=BG)
        window.attributes("-topmost", True)
        self.place_dialog(window, 680, 380)
        tk.Label(window, text=f"[{hit['citation']}]  {hit['path']}  ·  lines {hit['start']}–{hit['end']}",
                 bg=BG, fg=TEXT, font=("Segoe UI", 12), padx=16, pady=14).pack(anchor="w")
        tk.Label(window, text="Invented fixture · this is not an original file on your computer." if self.demo else hit["path"],
                 bg=BG, fg=ACCENT, padx=16).pack(anchor="w")
        text = scrolledtext.ScrolledText(window, bg=PANEL, fg=TEXT, wrap="word", relief="flat",
                                        padx=14, pady=14, font=("Consolas", 11))
        text.pack(fill="both", expand=True, padx=16, pady=16)
        text.insert("1.0", hit["text"])
        text.configure(state="disabled")
        if not self.demo:
            self.button(window, "Show original in folder", lambda: self.reveal_source(hit)).pack(anchor="w", padx=16, pady=(0, 12))
        window.bind("<Escape>", lambda _: window.destroy())

    def reveal_source(self, hit):
        path = Path(hit["path"])
        if not path.exists():
            self.status.configure(text="That source no longer exists. Refresh its folder.")
            return
        try:
            if sys.platform == "win32":
                subprocess.Popen(["explorer.exe", "/select,", str(path)])
            elif sys.platform == "darwin":
                subprocess.Popen(["open", "-R", str(path)])
            else:
                subprocess.Popen(["xdg-open", str(path.parent)])
        except OSError as exc:
            self.status.configure(text=f"Could not open the folder: {exc}", fg=ERROR)

    def render_chat_transcript(self):
        tk.Label(self.results, text="Chat · not based on your files", bg=BG, fg=ACCENT,
                 font=("Segoe UI", 9)).pack(anchor="w", pady=(4, 0))
        turns = list(self.chat_history)
        if self.pending_chat:
            turns.append({"role": "user", "content": self.pending_chat})
        if not turns:
            tk.Label(self.results, text="Press Enter to send. Earlier file answers are not part of this chat.",
                     bg=BG, fg=MUTED, wraplength=480, justify="left", anchor="w",
                     font=("Segoe UI", 10)).pack(anchor="w", pady=(6, 4))
            return
        for turn in turns[-6:]:
            who = "You" if turn["role"] == "user" else "Loft"
            tk.Label(self.results, text=f"{who}\n{turn['content']}", bg=BG, fg=TEXT, wraplength=480,
                     justify="left", anchor="w", font=("Segoe UI", 11)).pack(anchor="w", pady=(6, 0))

    def _watched_folder_changed(self, root):
        if self.engine is not None:
            self.engine.note_root_changed(root)
        self.events.put(("stale", root))

    def sync_watcher(self):
        if self.watcher is not None and self.engine is not None:
            self.watcher.set_roots(self.engine.roots())

    def refresh_stale_markers(self):
        if self.demo or self.busy or not (self.library_window and self.library_window.winfo_exists()):
            return
        now = time.monotonic()
        if now - self._stale_checked < 3:
            return
        self._stale_checked = now
        self.refresh_library_list()

    def close(self):
        self.closed = True
        self.cancel_work.set()
        for sequence in ("<MouseWheel>", "<Button-4>", "<Button-5>"):
            self.root.unbind_all(sequence)
        if self.event_timer is not None:
            self.root.after_cancel(self.event_timer)
        self.cancel_animation()
        self.stop_busy_fx()
        if self.theme_timer is not None:
            self.root.after_cancel(self.theme_timer)
        if self.watcher is not None:
            self.watcher.close()
        if self.searcher:
            self.searcher.close()
        if self.worker and self.worker.is_alive():
            self.worker.join(timeout=3)
        self.root.destroy()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="Exercise UI flows and exit")
    parser.add_argument("--expanded", action="store_true")
    parser.add_argument("--demo", action="store_true", help="Use invented fixtures instead of your library")
    args = parser.parse_args()
    enable_dpi_awareness()
    root = tk.Tk()
    app = LoftPreview(root, demo=args.demo or args.check, use_settings=not args.check)
    if args.expanded:
        app.expand()
    if args.check:
        def settle():
            done = tk.BooleanVar(root, False)
            root.after(300, lambda: done.set(True))
            root.wait_variable(done)
        app.expand()
        settle()
        root.update()
        app.ask("parking")
        root.update()
        assert app.last_hits[0]["path"] == "parking.txt"
        assert root.winfo_height() < 520, "Single result must not leave a large empty panel"
        assert app.body.winfo_height() >= app.body.winfo_reqheight(), "Content is clipped"
        app.open_source(app.last_hits[0])
        root.update()
        from loft_theme import palette
        for colour in ("#f3f3f3", "#1d5778", "#202020"):
            app.apply_theme(palette(colour))
            root.update()
            assert app.body.cget("bg") == palette(colour)[0]
            assert app.source_window.cget("bg") == palette(colour)[0]
            assert app.entry.get() == "parking"
        app.select_scope("Code")
        assert app.last_hits == []
        app.ask("login")
        assert app.last_hits[0]["path"] == "src/login.py"
        app.ask("!!!")
        root.update()
        assert root.winfo_height() < 380, "Empty state must shrink to its content"
        assert app.last_hits == []
        app.ask("")
        app.collapse()
        settle()
        root.update()
        assert not app.expanded
        assert root.winfo_height() == 48
        app.expand()
        root.after(40, app.collapse)
        root.after(80, app.expand)
        settle()
        settle()
        assert app.expanded and app.animation_timer is None
        assert app.canvas.itemcget(app.body_id, "state") == "normal"
        assert app.question.get() == ""
        from unittest.mock import patch
        with patch(__name__ + ".motion_enabled", return_value=False):
            app.collapse()
            root.update()
            assert app.animation_timer is None and root.winfo_height() == 48
            app.expand()
            assert app.animation_timer is None
        app.close()
        print("UI check passed: search, citations, sizing, transitions, reversal, reduced motion, close")
    else:
        root.mainloop()


if __name__ == "__main__":
    main()

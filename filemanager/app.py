"""Stage-one native Windows-friendly file browser with metadata-only history."""
from __future__ import annotations

import os
import queue
import subprocess
import sys
import threading
import tkinter as tk
from datetime import datetime
from pathlib import Path
from tkinter import filedialog, messagebox, ttk

from .history import HistoryStore, default_database


def open_file(path: Path) -> None:
    if sys.platform == "win32":
        os.startfile(str(path))  # type: ignore[attr-defined]
    elif sys.platform == "darwin":
        subprocess.Popen(["open", str(path)])
    else:
        subprocess.Popen(["xdg-open", str(path)])


def local_time(iso: str) -> str:
    return datetime.fromisoformat(iso).astimezone().strftime("%d.%m.%Y %H:%M:%S")


def size_label(size: int) -> str:
    n = float(size)
    for suffix in ("Б", "КБ", "МБ", "ГБ", "ТБ"):
        if n < 1024 or suffix == "ТБ":
            return f"{n:.0f} {suffix}" if suffix == "Б" else f"{n:.1f} {suffix}"
        n /= 1024
    return ""


class FileTab(ttk.Frame):
    def __init__(self, parent, app: "FileManager", folder: Path):
        super().__init__(parent)
        self.app = app
        self.folder = folder
        self.back_paths: list[Path] = []
        self.forward_paths: list[Path] = []
        self.files: dict[str, Path] = {}
        self.history_ids: dict[str, int] = {}
        self.history_rows: dict[int, dict] = {}
        self.selected: Path | None = None

        bar = ttk.Frame(self)
        bar.pack(fill="x", padx=8, pady=7)
        ttk.Button(bar, text="←", width=3, command=self.back).pack(side="left")
        ttk.Button(bar, text="→", width=3, command=self.forward).pack(side="left")
        ttk.Button(bar, text="↑", width=3, command=self.up).pack(side="left")
        self.path_var = tk.StringVar(value=str(folder))
        self.address = ttk.Entry(bar, textvariable=self.path_var)
        self.address.pack(side="left", fill="x", expand=True, padx=8)
        self.address.bind("<Return>", lambda _: self.navigate(self.path_var.get()))
        ttk.Button(bar, text="Перейти", command=lambda: self.navigate(self.path_var.get())).pack(side="left")
        ttk.Button(bar, text="↻", command=self.refresh).pack(side="left", padx=4)

        filter_row = ttk.Frame(self)
        filter_row.pack(fill="x", padx=8)
        ttk.Label(filter_row, text="Поиск по имени:").pack(side="left")
        self.filter_var = tk.StringVar()
        self.filter_var.trace_add("write", lambda *_: self.refresh())
        ttk.Entry(filter_row, textvariable=self.filter_var).pack(fill="x", expand=True, padx=8, pady=6)

        split = ttk.Panedwindow(self, orient="horizontal")
        split.pack(fill="both", expand=True, padx=8, pady=8)
        left = ttk.Frame(split)
        right = ttk.Frame(split)
        split.add(left, weight=3)
        split.add(right, weight=2)

        self.files_tree = ttk.Treeview(left, columns=("name", "size", "date"), show="headings", selectmode="browse")
        for key, title, width in (("name", "Файл / папка", 350), ("size", "Размер", 90), ("date", "Изменён", 150)):
            self.files_tree.heading(key, text=title)
            self.files_tree.column(key, width=width, minwidth=65, stretch=key == "name")
        self.files_tree.pack(side="left", fill="both", expand=True)
        scroll = ttk.Scrollbar(left, command=self.files_tree.yview)
        scroll.pack(side="right", fill="y")
        self.files_tree.configure(yscrollcommand=scroll.set)
        self.files_tree.bind("<<TreeviewSelect>>", self.select_file)
        self.files_tree.bind("<Double-1>", self.open_selected)
        self.files_tree.bind("<Return>", self.open_selected)

        ttk.Label(right, text="История файла", font=("Segoe UI", 12, "bold")).pack(anchor="w", pady=6)
        self.filename = ttk.Label(right, text="Выберите файл")
        self.filename.pack(anchor="w", pady=6)
        self.events_tree = ttk.Treeview(right, columns=("date", "kind"), show="headings", height=10)
        self.events_tree.heading("date", text="Обнаружено")
        self.events_tree.heading("kind", text="Событие")
        self.events_tree.column("date", width=160)
        self.events_tree.column("kind", width=95)
        self.events_tree.pack(fill="both", expand=True)
        self.events_tree.bind("<<TreeviewSelect>>", self.select_event)

        ttk.Label(right, text="Автор (вручную, если известен)").pack(anchor="w", pady=(10, 2))
        self.author = tk.StringVar()
        ttk.Entry(right, textvariable=self.author).pack(fill="x")
        ttk.Label(right, text="Комментарий к сохранению").pack(anchor="w", pady=(8, 2))
        self.comment = tk.Text(right, height=4, background="#252932", foreground="#f0f0f0",
                               insertbackground="#f0f0f0", wrap="word")
        self.comment.pack(fill="x")
        ttk.Button(right, text="Сохранить комментарий", command=self.save_annotation).pack(anchor="e", pady=7)
        self.refresh()

    def navigate(self, target: str | Path, remember: bool = True) -> None:
        destination = Path(target).expanduser().absolute()
        if not destination.is_dir():
            messagebox.showerror("Папка не найдена", str(destination))
            return
        if remember and destination != self.folder:
            self.back_paths.append(self.folder)
            self.forward_paths.clear()
        self.folder = destination
        self.path_var.set(str(destination))
        self.filter_var.set("")
        self.refresh()
        self.app.tabs.tab(self, text=destination.name or str(destination))

    def back(self) -> None:
        if self.back_paths:
            self.forward_paths.append(self.folder)
            self.navigate(self.back_paths.pop(), remember=False)

    def forward(self) -> None:
        if self.forward_paths:
            self.back_paths.append(self.folder)
            self.navigate(self.forward_paths.pop(), remember=False)

    def up(self) -> None:
        if self.folder.parent != self.folder:
            self.navigate(self.folder.parent)

    def refresh(self) -> None:
        self.files_tree.delete(*self.files_tree.get_children())
        self.files.clear()
        try:
            entries = sorted(os.scandir(self.folder), key=lambda e: (
                not e.is_dir(follow_symlinks=False), e.name.casefold()
            ))
        except OSError as exc:
            self.app.set_status(f"Ошибка чтения: {exc}")
            return
        query = self.filter_var.get().casefold()
        for entry in entries:
            if query not in entry.name.casefold():
                continue
            try:
                st = entry.stat(follow_symlinks=False)
                directory = entry.is_dir(follow_symlinks=False)
            except OSError:
                continue
            row = self.files_tree.insert("", "end", values=(
                ("▸ " if directory else "   ") + entry.name,
                "—" if directory else size_label(st.st_size),
                datetime.fromtimestamp(st.st_mtime).strftime("%d.%m.%Y %H:%M"),
            ))
            self.files[row] = Path(entry.path)
        self.app.set_status(f"{self.folder} · {len(self.files)} объектов")

    def select_file(self, _event=None) -> None:
        current = self.files_tree.selection()
        self.selected = self.files.get(current[0]) if current else None
        self.show_history()

    def open_selected(self, _event=None) -> None:
        current = self.files_tree.selection()
        path = self.files.get(current[0]) if current else None
        if not path:
            return
        if path.is_dir():
            self.navigate(path)
        else:
            try:
                open_file(path)
            except OSError as exc:
                messagebox.showerror("Файл не открывается", str(exc))

    def show_history(self) -> None:
        self.events_tree.delete(*self.events_tree.get_children())
        self.history_ids.clear()
        self.author.set("")
        self.comment.delete("1.0", "end")
        self.filename.configure(text=self.selected.name if self.selected else "Выберите файл")
        self.history_rows = {}
        if not self.selected or self.selected.is_dir():
            return
        rows = self.app.store.events(self.selected)
        self.history_rows = {row["id"]: row for row in rows}
        names = {"baseline": "Обнаружен", "modified": "Изменён", "missing": "Удалён"}
        for item in rows:
            row_id = self.events_tree.insert("", "end", values=(
                local_time(item["observed_at"]), names.get(item["kind"], item["kind"])
            ))
            self.history_ids[row_id] = item["id"]

    def select_event(self, _event=None) -> None:
        current = self.events_tree.selection()
        if not current:
            return
        row = self.history_rows[self.history_ids[current[0]]]
        self.author.set(row["author"] or "")
        self.comment.delete("1.0", "end")
        self.comment.insert("1.0", row["comment"])
        self.app.set_status(f"Зафиксировано пользователем {row['recorded_by']}; фактический автор неизвестен")

    def save_annotation(self) -> None:
        current = self.events_tree.selection()
        if not current:
            messagebox.showinfo("История", "Сначала выберите событие")
            return
        try:
            updated = self.app.store.annotate(
                self.history_ids[current[0]], self.comment.get("1.0", "end-1c"), self.author.get()
            )
        except ValueError as exc:
            messagebox.showerror("Слишком длинный текст", str(exc))
            return
        if updated:
            self.show_history()
            self.app.set_status("Комментарий сохранён")


class FileManager(tk.Tk):
    def __init__(self):
        super().__init__()
        self.title("Filemanager — прототип")
        self.geometry("1220x760")
        self.minsize(800, 520)
        self.configure(background="#171b22")
        self.store = HistoryStore(default_database())
        self.stop_signal = threading.Event()
        self.messages: queue.Queue[str] = queue.Queue()
        self.worker: threading.Thread | None = None
        self.status = tk.StringVar(value="Файловые операции пока только для чтения")
        self.setup_style()

        top = ttk.Frame(self)
        top.pack(fill="x", padx=12, pady=8)
        ttk.Label(top, text="FILEMANAGER", foreground="#76b1fa", font=("Segoe UI", 14, "bold")).pack(side="left")
        ttk.Button(top, text="+ Вкладка", command=self.new_tab).pack(side="left", padx=12)
        ttk.Button(top, text="Открыть папку…", command=self.pick_folder).pack(side="left")
        ttk.Button(top, text="Наблюдать за папкой…", command=self.start_watch).pack(side="right")
        ttk.Button(top, text="Стоп", command=self.stop_watch).pack(side="right", padx=5)

        self.tabs = ttk.Notebook(self)
        self.tabs.pack(fill="both", expand=True, padx=12, pady=6)
        self.new_tab()
        ttk.Label(self, textvariable=self.status, foreground="#afb6c2").pack(anchor="w", padx=16, pady=7)
        self.after(500, self.drain_messages)
        self.protocol("WM_DELETE_WINDOW", self.close)

    def setup_style(self) -> None:
        style = ttk.Style(self)
        if "clam" in style.theme_names():
            style.theme_use("clam")
        style.configure(".", background="#171b22", foreground="#edf0f5",
                        fieldbackground="#262b35", font=("Segoe UI", 10))
        style.configure("TFrame", background="#171b22")
        style.configure("TLabel", background="#171b22", foreground="#edf0f5")
        style.configure("TButton", background="#2c3440", foreground="#edf0f5", padding=(8, 6))
        style.configure("TEntry", fieldbackground="#262b35", foreground="#edf0f5")
        style.configure("Treeview", background="#242a33", fieldbackground="#242a33",
                        foreground="#f2f3f7", rowheight=27)
        style.map("Treeview", background=[("selected", "#30557b")])
        style.configure("Treeview.Heading", background="#303845", foreground="#eef0f5")
        style.configure("TNotebook", background="#171b22")
        style.configure("TNotebook.Tab", background="#262b35", foreground="#edf0f5", padding=(15, 8))
        style.map("TNotebook.Tab", background=[("selected", "#30557b")])

    def new_tab(self, folder: Path | None = None) -> None:
        root = folder or Path.home()
        tab = FileTab(self.tabs, self, root)
        self.tabs.add(tab, text=root.name or str(root))
        self.tabs.select(tab)

    def current_tab(self) -> FileTab:
        return self.nametowidget(self.tabs.select())  # type: ignore[return-value]

    def pick_folder(self) -> None:
        chosen = filedialog.askdirectory(initialdir=str(self.current_tab().folder))
        if chosen:
            self.current_tab().navigate(chosen)

    def start_watch(self) -> None:
        chosen = filedialog.askdirectory(title="Выберите папку для журнала")
        if not chosen:
            return
        self.stop_watch()
        stop = threading.Event()
        self.stop_signal = stop
        self.worker = threading.Thread(target=self.watch_loop, args=(Path(chosen), stop), daemon=True)
        self.worker.start()
        self.set_status("Наблюдение за папкой: " + chosen)

    def watch_loop(self, folder: Path, stop: threading.Event) -> None:
        while not stop.is_set():
            try:
                changes = self.store.scan_once(folder)
                if changes:
                    self.messages.put(f"Записано событий: {changes} · {folder}")
            except Exception as exc:
                self.messages.put(f"Наблюдение остановлено: {exc}")
                return
            if stop.wait(4):
                return

    def stop_watch(self) -> None:
        self.stop_signal.set()
        self.set_status("Наблюдение остановлено")

    def drain_messages(self) -> None:
        try:
            while True:
                self.set_status(self.messages.get_nowait())
                self.current_tab().show_history()
        except queue.Empty:
            pass
        self.after(600, self.drain_messages)

    def set_status(self, text: str) -> None:
        self.status.set(text)

    def close(self) -> None:
        self.stop_signal.set()
        self.destroy()


def main() -> None:
    FileManager().mainloop()


if __name__ == "__main__":
    main()

"""Metadata-only file activity journal. Never reads or stores file contents."""
from __future__ import annotations

import getpass
import os
import sqlite3
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path


@dataclass(frozen=True)
class Snapshot:
    size: int
    mtime_ns: int
    identity: str


def now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def default_database() -> Path:
    base = os.environ.get("LOCALAPPDATA")
    if base:
        return Path(base) / "Filemanager" / "history.sqlite3"
    return Path.home() / ".filemanager" / "history.sqlite3"


class HistoryStore:
    """SQLite records only path, timestamps, event kind and human annotations."""

    def __init__(self, database: Path | str):
        self.database = Path(database).expanduser().absolute()
        self.database.parent.mkdir(parents=True, exist_ok=True)
        with self._connect() as db:
            db.executescript("""
                CREATE TABLE IF NOT EXISTS tracked (
                    root TEXT NOT NULL,
                    path TEXT NOT NULL,
                    size INTEGER NOT NULL,
                    mtime_ns INTEGER NOT NULL,
                    identity TEXT NOT NULL,
                    missing_scans INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (root, path)
                );
                CREATE TABLE IF NOT EXISTS events (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    path TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    observed_at TEXT NOT NULL,
                    file_mtime_ns INTEGER,
                    size INTEGER,
                    recorded_by TEXT NOT NULL,
                    author TEXT,
                    comment TEXT NOT NULL DEFAULT ''
                );
                CREATE INDEX IF NOT EXISTS events_path_idx ON events(path, id DESC);
            """)

    def _connect(self) -> sqlite3.Connection:
        db = sqlite3.connect(self.database, timeout=15)
        db.execute("PRAGMA busy_timeout = 15000")
        return db

    def events(self, path: Path | str, limit: int = 100) -> list[dict]:
        with self._connect() as db:
            db.row_factory = sqlite3.Row
            rows = db.execute(
                "SELECT * FROM events WHERE path = ? ORDER BY id DESC LIMIT ?",
                (str(Path(path).absolute()), limit),
            ).fetchall()
        return [dict(r) for r in rows]

    def annotate(self, event_id: int, comment: str, author: str = "") -> bool:
        if len(comment) > 5000 or len(author) > 200:
            raise ValueError("Comment or author is too long")
        with self._connect() as db:
            result = db.execute(
                "UPDATE events SET comment = ?, author = ? WHERE id = ?",
                (comment, author.strip() or None, event_id),
            )
            return result.rowcount == 1

    def scan_once(self, root: Path | str, max_files: int = 50000) -> int:
        """Scan monitored tree; return count of added journal events.

        An initial scan records 'baseline' (not a claim that a file was created).
        Deletions need two complete scans, reducing transient rename false positives.
        """
        root = Path(root).expanduser().resolve(strict=True)
        if not root.is_dir():
            raise NotADirectoryError(root)
        if max_files < 1:
            raise ValueError("max_files must be positive")
        excludes = {str(self.database), str(self.database) + "-wal", str(self.database) + "-shm"}
        seen: dict[str, Snapshot] = {}
        pending = [root]
        while pending:
            folder = pending.pop()
            with os.scandir(folder) as entries:
                for entry in entries:
                    if entry.is_symlink():
                        continue
                    if entry.is_dir(follow_symlinks=False):
                        if entry.name not in {".git", ".svn", ".hg", "__pycache__"}:
                            pending.append(Path(entry.path))
                        continue
                    if entry.path in excludes or entry.name.startswith("~$"):
                        continue
                    if not entry.is_file(follow_symlinks=False):
                        continue
                    try:
                        st = entry.stat(follow_symlinks=False)
                    except (FileNotFoundError, PermissionError):
                        continue
                    seen[str(Path(entry.path).absolute())] = Snapshot(
                        st.st_size, st.st_mtime_ns, f"{st.st_dev}:{st.st_ino}"
                    )
                    if len(seen) > max_files:
                        raise RuntimeError(f"More than {max_files} files; narrow the monitored folder")

        root_key = str(root)
        stamp = now()
        observer = getpass.getuser()
        changes = 0
        with self._connect() as db:
            old = {
                path: (size, mtime, identity, missing)
                for path, size, mtime, identity, missing in db.execute(
                    "SELECT path, size, mtime_ns, identity, missing_scans FROM tracked WHERE root = ?",
                    (root_key,),
                )
            }
            for path, data in seen.items():
                previous = old.pop(path, None)
                if previous is None:
                    kind = "baseline"
                elif previous[:3] != (data.size, data.mtime_ns, data.identity):
                    kind = "modified"
                else:
                    kind = ""
                if kind:
                    db.execute(
                        "INSERT INTO events(path,kind,observed_at,file_mtime_ns,size,recorded_by) VALUES(?,?,?,?,?,?)",
                        (path, kind, stamp, data.mtime_ns, data.size, observer),
                    )
                    changes += 1
                db.execute(
                    "INSERT INTO tracked(root,path,size,mtime_ns,identity,missing_scans) VALUES(?,?,?,?,?,0) "
                    "ON CONFLICT(root,path) DO UPDATE SET size=excluded.size, mtime_ns=excluded.mtime_ns, "
                    "identity=excluded.identity, missing_scans=0",
                    (root_key, path, data.size, data.mtime_ns, data.identity),
                )
            for path, (size, mtime, _identity, missing) in old.items():
                if missing >= 1:
                    db.execute(
                        "INSERT INTO events(path,kind,observed_at,file_mtime_ns,size,recorded_by) VALUES(?,?,?,?,?,?)",
                        (path, "missing", stamp, mtime, size, observer),
                    )
                    db.execute("DELETE FROM tracked WHERE root=? AND path=?", (root_key, path))
                    changes += 1
                else:
                    db.execute(
                        "UPDATE tracked SET missing_scans=1 WHERE root=? AND path=?", (root_key, path)
                    )
        return changes

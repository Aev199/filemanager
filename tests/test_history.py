import tempfile
import unittest
from pathlib import Path

from filemanager.history import HistoryStore


class HistoryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "work"
        self.root.mkdir()
        self.store = HistoryStore(Path(self.temp.name) / "metadata" / "history.db")

    def test_initial_scan_and_repeat_do_not_duplicate(self):
        path = self.root / "document.txt"
        path.write_text("old", encoding="utf8")
        self.assertEqual(self.store.scan_once(self.root), 1)
        self.assertEqual(self.store.scan_once(self.root), 0)
        events = self.store.events(path)
        self.assertEqual([e["kind"] for e in events], ["baseline"])
        self.assertIsNone(events[0]["author"])

    def test_modified_file_records_event_without_content(self):
        path = self.root / "report.txt"
        path.write_text("draft", encoding="utf8")
        self.store.scan_once(self.root)
        path.write_text("final result", encoding="utf8")
        self.assertEqual(self.store.scan_once(self.root), 1)
        self.assertEqual([e["kind"] for e in self.store.events(path)], ["modified", "baseline"])
        database_bytes = self.store.database.read_bytes()
        self.assertNotIn(b"final result", database_bytes)
        self.assertNotIn(b"draft", database_bytes)

    def test_comment_and_author_are_editable(self):
        path = self.root / "one.txt"
        path.write_text("a")
        self.store.scan_once(self.root)
        event = self.store.events(path)[0]
        self.assertTrue(self.store.annotate(event["id"], "Changed geometry", "Engineer"))
        updated = self.store.events(path)[0]
        self.assertEqual((updated["comment"], updated["author"]), ("Changed geometry", "Engineer"))

    def test_missing_requires_two_scans(self):
        path = self.root / "x.txt"
        path.write_text("a")
        self.store.scan_once(self.root)
        path.unlink()
        self.assertEqual(self.store.scan_once(self.root), 0)
        self.assertEqual(self.store.scan_once(self.root), 1)
        self.assertEqual(self.store.events(path)[0]["kind"], "missing")

    def test_atomic_replacement_is_modification(self):
        path = self.root / "sheet.xlsx"
        path.write_bytes(b"a")
        self.store.scan_once(self.root)
        temporary = self.root / "replacement.tmp"
        temporary.write_bytes(b"new data")
        temporary.replace(path)
        self.store.scan_once(self.root)
        self.assertEqual(self.store.events(path)[0]["kind"], "modified")

    def test_inaccessible_root_does_not_mark_files_deleted(self):
        path = self.root / "a.txt"
        path.write_text("a")
        self.store.scan_once(self.root)
        with self.assertRaises(FileNotFoundError):
            self.store.scan_once(self.root / "does_not_exist")
        self.assertEqual(len(self.store.events(path)), 1)

    def test_scan_limit_fails_without_writing_partial_events(self):
        (self.root / "a").write_text("a")
        (self.root / "b").write_text("b")
        with self.assertRaises(RuntimeError):
            self.store.scan_once(self.root, max_files=1)
        self.assertEqual(self.store.events(self.root / "a"), [])

    def test_symlinks_not_followed(self):
        real = self.root / "real.txt"
        real.write_text("test")
        link = self.root / "link.txt"
        try:
            link.symlink_to(real)
        except (OSError, NotImplementedError):
            self.skipTest("Symlink creation requires privileges")
        self.assertEqual(self.store.scan_once(self.root), 1)
        self.assertEqual(self.store.events(link), [])


if __name__ == "__main__":
    unittest.main()

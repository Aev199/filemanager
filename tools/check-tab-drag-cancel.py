"""Run the Escape handler's drag cancellation prelude without starting a GUI."""
from pathlib import Path
import subprocess
import tempfile

repo = Path(__file__).resolve().parents[1]
source = (repo / "crates/desktop/src/keys.rs").read_text()
prelude = source.split("pub(crate) fn key_dismiss(", 1)[1].split("{", 1)[1].split("if let Some(resize)", 1)[0]
with tempfile.TemporaryDirectory(prefix="filemanager-drag-cancel-") as folder:
    path = Path(folder)
    (path / "test.rs").write_text("""
struct State { dock_hover: Option<()>, selection: usize }
struct Window;
struct Context { active_drag: bool, notified: bool }
impl Context {
    fn stop_active_drag(&mut self, _: &mut Window) -> bool {
        let was_active = self.active_drag; self.active_drag = false; was_active
    }
    fn notify(&mut self) { self.notified = true; }
}
impl State {
    fn escape(&mut self, window: &mut Window, cx: &mut Context) {
""" + prelude + """
    }
}
fn main() {
    for hover in [None, Some(())] {
        let mut state = State { dock_hover: hover, selection: 7 };
        let mut cx = Context { active_drag: true, notified: false };
        state.escape(&mut Window, &mut cx);
        assert!(!cx.active_drag, "Escape must stop drag before mouse release can drop");
        assert!(state.dock_hover.is_none());
        assert_eq!(state.selection, 7);
        assert!(cx.notified);
    }
}
""")
    subprocess.run(["rustc", "--edition=2024", str(path / "test.rs"), "-o", str(path / "test")], check=True)
    subprocess.run([str(path / "test")], check=True)
print("Escape cancels an active drag with and without a dock overlay")

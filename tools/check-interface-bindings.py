"""Exercise repository bindings with the installed GPUI 0.2.2 predicate parser.
No GUI is started. Cargo dependencies must already be cached.
"""
from pathlib import Path
import os
import re
import subprocess
import tempfile

repo = Path(__file__).resolve().parents[1]
registry = Path(os.environ['CARGO_HOME']) / 'registry' / 'src'
source = next(registry.glob('*/gpui-ce-0.2.2/src/keymap/context.rs'))
# Execute the production parser; omit GPUI's unrelated action-macro unit tests.
parser = source.read_text().split('#[cfg(test)]', 1)[0]
keys = (repo / 'crates/desktop/src/keys.rs').read_text()
cases = []
for key, action in [('up', 'SelectPrev'), ('down', 'SelectNext'), ('enter', 'PaletteEnter'), ('escape', 'DismissOverlay')]:
    predicates = re.findall(r'KeyBinding::new\("' + re.escape(key) + r'",\s*' + action + r',\s*Some\("([^\"]*CommandPalette[^\"]*)"\)\)', keys)
    assert len(predicates) == 1, (key, predicates)
    predicate = predicates[0]
    cases.append(f'''let predicate=KeyBindingContextPredicate::parse("{predicate}").unwrap();
assert_eq!(predicate.depth_of(&palette),Some(3),"{key} must match focused palette Input");
assert_eq!(predicate.depth_of(&ordinary),None,"{key} must not change ordinary Input");''')
# Run the actual opener's early-return predicate with existing-dialog fixtures.
interface = (repo / 'crates/desktop/src/interface.rs').read_text()
open_body = interface.split('pub(crate) fn open_settings(', 1)[1]
guard = re.search(r'\bif\s+(.*?)\s*\{\s*return;', open_body, re.S).group(1)
cases.append(f"""struct State {{ palette: Option<()>, settings: Option<bool> }}
impl State {{ fn can_open_interface(&self)->bool {{ true }} fn blocked(&self)->bool {{ {guard} }} }}
assert!(State{{palette:None,settings:Some(true)}}.blocked(),"pending save must retain its dialog");
assert!(State{{palette:None,settings:Some(false)}}.blocked(),"open draft must retain its dialog");
assert!(!State{{palette:None,settings:None}}.blocked(),"closed settings may open");""")
with tempfile.TemporaryDirectory(prefix='filemanager-binding-check-') as task_dir:
    task = Path(task_dir)
    (task/'src').mkdir()
    (task/'Cargo.toml').write_text('[package]\nname="filemanager-binding-check"\nversion="0.0.0"\nedition="2024"\n[dependencies]\nanyhow="1"\n[workspace]\n')
    (task/'src/context.rs').write_text(parser)
    # SharedString is only immutable text storage here, represented by Arc<str>.
    # The parser/evaluator itself is unchanged production GPUI code.
    (task/'src/main.rs').write_text('''#![allow(dead_code)]
type SharedString=std::sync::Arc<str>;
mod context;
use context::{KeyContext,KeyBindingContextPredicate};
fn main() {
 let palette=vec![KeyContext::parse("Filemanager").unwrap(),KeyContext::parse("CommandPalette").unwrap(),KeyContext::parse("Input").unwrap()];
 let ordinary=vec![KeyContext::parse("Filemanager").unwrap(),KeyContext::parse("Input").unwrap()];
'''+'\n'.join(cases)+'\nprintln!("4 palette bindings match and remain scoped");\n}\n')
    cargo = Path(os.environ['CARGO_HOME'])/'bin/cargo'
    subprocess.run([str(cargo),'run','--offline','--manifest-path',str(task/'Cargo.toml')],check=True)

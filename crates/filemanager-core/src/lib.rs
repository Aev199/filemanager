//! Platform-independent model of the file manager.
//!
//! Everything here is plain Rust without UI code so it can be unit-tested on
//! any OS. The GPUI front end in `filemanager-desktop` renders this state.

pub mod entry;
pub mod format;
pub mod nav;
pub mod paths;
pub mod sort;

pub use entry::{read_dir, Entry, EntryKind, ListOptions};
pub use sort::{SortKey, SortSpec};

//! Native filesystem core for Filemanager. No GUI dependencies.
pub mod directory_watch;
pub mod browser;
pub mod history;
pub mod versions;
pub mod operations;
pub mod bulk_rename;
pub mod search;

pub mod workspace;

pub mod folder_copy;

pub mod persistent_index;

pub mod index_watch;

pub mod path_utils;

pub mod operation_journal;

pub mod sort;

pub mod format;

pub mod places;

pub mod selection;

pub mod clipboard;

pub mod recycle_bin;

pub mod thumbnail;
pub mod preview_host;
#[cfg(windows)]
mod document_preview;

mod undo_snapshot;
mod native_copy;
mod copy_verification;
mod cross_volume_move;
mod replacement;

pub mod interface;

pub mod panes;

pub mod projects;

#[cfg(windows)]
pub mod shell_drag;

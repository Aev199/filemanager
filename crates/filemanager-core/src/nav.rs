//! Back/forward history of one tab.

use std::path::{Path, PathBuf};

const MAX_HISTORY: usize = 200;

#[derive(Clone, Debug)]
pub struct NavHistory {
    back: Vec<PathBuf>,
    current: PathBuf,
    forward: Vec<PathBuf>,
}

impl NavHistory {
    pub fn new(current: PathBuf) -> Self {
        Self { back: Vec::new(), current, forward: Vec::new() }
    }

    pub fn current(&self) -> &Path {
        &self.current
    }

    /// Returns false when `path` is already current.
    pub fn navigate(&mut self, path: PathBuf) -> bool {
        if path == self.current {
            return false;
        }
        self.back.push(std::mem::replace(&mut self.current, path));
        if self.back.len() > MAX_HISTORY {
            self.back.remove(0);
        }
        self.forward.clear();
        true
    }

    pub fn go_back(&mut self) -> bool {
        let Some(previous) = self.back.pop() else { return false };
        self.forward.push(std::mem::replace(&mut self.current, previous));
        true
    }

    pub fn go_forward(&mut self) -> bool {
        let Some(next) = self.forward.pop() else { return false };
        self.back.push(std::mem::replace(&mut self.current, next));
        true
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_and_forward() {
        let mut nav = NavHistory::new("a".into());
        assert!(!nav.navigate("a".into()));
        assert!(nav.navigate("b".into()));
        assert!(nav.navigate("c".into()));
        assert!(nav.go_back());
        assert_eq!(nav.current(), Path::new("b"));
        assert!(nav.can_go_forward());
        assert!(nav.go_back());
        assert!(!nav.go_back());
        assert!(nav.go_forward());
        assert_eq!(nav.current(), Path::new("b"));
        // A new navigation drops the forward stack.
        assert!(nav.navigate("d".into()));
        assert!(!nav.can_go_forward());
        assert!(nav.go_back());
        assert_eq!(nav.current(), Path::new("b"));
    }

    #[test]
    fn history_is_bounded() {
        let mut nav = NavHistory::new("0".into());
        for i in 1..=(MAX_HISTORY + 50) {
            nav.navigate(i.to_string().into());
        }
        let mut steps = 0;
        while nav.go_back() {
            steps += 1;
        }
        assert_eq!(steps, MAX_HISTORY);
    }
}

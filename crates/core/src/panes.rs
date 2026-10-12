//! Independent tab groups and a bounded split tree. Only directory paths are stored.
use crate::browser::{Pane, display_name};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GroupId(pub u64);
#[derive(Clone, Debug)]
pub struct Tab { pub pinned: bool, pub title: String, pub pane: Pane }
impl Tab {
    pub fn new(path: impl AsRef<Path>) -> io::Result<Self> {
        let pane = Pane::new(path)?;
        Ok(Self { title: display_name(&pane.path), pane, pinned: false })
    }
    pub fn active(&self) -> &Pane { &self.pane }
    pub fn active_mut(&mut self) -> &mut Pane { &mut self.pane }
    pub fn navigate(&mut self, path: impl AsRef<Path>) -> io::Result<()> {
        self.pane.navigate(path)?; self.title = display_name(&self.pane.path); Ok(())
    }
}
#[derive(Clone, Debug)]
/// `miller`: columns view, otherwise a list; each pane chooses its own.
pub struct Group { pub id: GroupId, pub tabs: Vec<Tab>, pub active_tab: usize, pub miller: bool }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis { Horizontal, Vertical }
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Leaf(GroupId),
    Split { axis: Axis, ratio: f32, first: Box<Node>, second: Box<Node> },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge { Left, Right, Top, Bottom }
pub const MAX_GROUPS: usize = 4;
pub const MAX_TABS: usize = 100;
/// Old tabs each held two folders; restoration must preserve both.
pub const MAX_RESTORED_TABS: usize = MAX_TABS * 2;
pub const CLOSED_TABS_REMEMBERED: usize = 20;

impl Node {
    pub fn encode(&self) -> String {
        match self {
            Self::Leaf(id) => id.0.to_string(),
            Self::Split { axis, ratio, first, second } => format!("{}({},{},{})",
                if *axis == Axis::Horizontal { "h" } else { "v" }, ratio, first.encode(), second.encode()),
        }
    }
    pub fn decode(input: &str) -> Option<Self> {
        fn parse(s: &[u8], pos: &mut usize, depth: usize) -> Option<Node> {
            if depth > 8 { return None; }
            let start = *pos;
            match *s.get(*pos)? {
                b'h' | b'v' => {
                    let axis = if s[*pos] == b'h' { Axis::Horizontal } else { Axis::Vertical };
                    *pos += 1;
                    if s.get(*pos) != Some(&b'(') { return None; } *pos += 1;
                    let begin = *pos;
                    while s.get(*pos).is_some_and(|c| *c != b',') { *pos += 1; }
                    let ratio: f32 = std::str::from_utf8(&s[begin..*pos]).ok()?.parse().ok()?;
                    if !ratio.is_finite() || s.get(*pos) != Some(&b',') { return None; } *pos += 1;
                    let first = Box::new(parse(s, pos, depth + 1)?);
                    if s.get(*pos) != Some(&b',') { return None; } *pos += 1;
                    let second = Box::new(parse(s, pos, depth + 1)?);
                    if s.get(*pos) != Some(&b')') { return None; } *pos += 1;
                    Some(Node::Split { axis, ratio: ratio.clamp(0.15, 0.85), first, second })
                }
                b'0'..=b'9' => {
                    while s.get(*pos).is_some_and(u8::is_ascii_digit) { *pos += 1; }
                    Some(Node::Leaf(GroupId(std::str::from_utf8(&s[start..*pos]).ok()?.parse().ok()?)))
                }
                _ => None,
            }
        }
        if input.len() > 4096 { return None; }
        let mut pos = 0;
        let node = parse(input.as_bytes(), &mut pos, 0)?;
        (pos == input.len()).then_some(node)
    }
    fn leaves(&self, out: &mut Vec<GroupId>) {
        match self { Self::Leaf(id) => out.push(*id), Self::Split { first, second, .. } => { first.leaves(out); second.leaves(out); } }
    }
    pub fn retain(&self, ids: &[GroupId]) -> Option<Node> {
        match self {
            Self::Leaf(id) => ids.contains(id).then(|| self.clone()),
            Self::Split { axis, ratio, first, second } => match (first.retain(ids), second.retain(ids)) {
                (Some(a), Some(b)) => Some(Self::Split { axis: *axis, ratio: *ratio, first: Box::new(a), second: Box::new(b) }),
                (Some(a), None) | (None, Some(a)) => Some(a), _ => None,
            },
        }
    }
    fn replace(&mut self, id: GroupId, node: &Node) -> bool {
        match self {
            Self::Leaf(old) if *old == id => { *self = node.clone(); true }
            Self::Split { first, second, .. } => first.replace(id, node) || second.replace(id, node), _ => false,
        }
    }
    pub fn signature(&self) -> String {
        match self { Self::Leaf(id) => id.0.to_string(), Self::Split { axis, first, second, .. } => format!("{}({},{})", if *axis == Axis::Horizontal { "h" } else { "v" }, first.signature(), second.signature()) }
    }
}

#[derive(Clone, Debug)]
pub struct Workspace {
    pub groups: Vec<Group>, pub root: Node, pub active: GroupId,
    next_id: u64, closed: Vec<(GroupId, PathBuf)>, recent: Vec<GroupId>,
}
fn invalid(message: &str) -> io::Error { io::Error::new(io::ErrorKind::InvalidInput, message) }
impl Workspace {
    pub fn new(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::from_parts(vec![Group { id: GroupId(1), tabs: vec![Tab::new(path)?], active_tab: 0, miller: true }], Node::Leaf(GroupId(1)), GroupId(1))
    }
    pub fn from_parts(groups: Vec<Group>, root: Node, active: GroupId) -> io::Result<Self> {
        let mut ids = Vec::new(); root.leaves(&mut ids);
        if groups.is_empty() || groups.len() > MAX_GROUPS || groups.iter().map(|g|g.tabs.len()).sum::<usize>() > MAX_RESTORED_TABS
            || ids.len() != groups.len() || groups.iter().any(|g| g.tabs.is_empty() || g.active_tab >= g.tabs.len() || ids.iter().filter(|id| **id == g.id).count() != 1)
            || !groups.iter().any(|g|g.id == active) { return Err(invalid("Повреждённая раскладка")); }
        let next_id = groups.iter().map(|g| g.id.0).max().unwrap().checked_add(1).ok_or_else(|| invalid("Неверный идентификатор панели"))?;
        Ok(Self { groups, root, active, next_id, closed: Vec::new(), recent: vec![active] })
    }
    pub fn group(&self, id: GroupId) -> Option<&Group> { self.groups.iter().find(|g|g.id == id) }
    pub fn group_mut(&mut self, id: GroupId) -> Option<&mut Group> { self.groups.iter_mut().find(|g|g.id == id) }
    pub fn active_group(&self) -> &Group { self.group(self.active).unwrap() }
    pub fn active_group_mut(&mut self) -> &mut Group { self.group_mut(self.active).unwrap() }
    pub fn active_tab(&self) -> &Tab { let g = self.active_group(); &g.tabs[g.active_tab] }
    pub fn active_pane(&self) -> &Pane { &self.active_tab().pane }
    pub fn active_pane_mut(&mut self) -> &mut Pane { &mut self.active_mut().pane }
    pub fn active(&self) -> &Tab { self.active_tab() }
    pub fn active_mut(&mut self) -> &mut Tab { let g = self.active_group_mut(); &mut g.tabs[g.active_tab] }
    pub fn focus(&mut self, id: GroupId) {
        if self.group(id).is_none() { return; }
        self.active = id; self.recent.retain(|old| *old != id); self.recent.push(id); self.check();
    }
    pub fn tab_count(&self) -> usize { self.groups.iter().map(|g|g.tabs.len()).sum() }
    pub fn new_tab(&mut self, id: GroupId, path: impl AsRef<Path>) -> io::Result<()> {
        if self.tab_count() >= MAX_TABS { return Err(invalid("Не больше ста вкладок")); }
        let tab = Tab::new(path)?;
        let g = self.group_mut(id).ok_or_else(|| invalid("Панель закрыта"))?;
        g.active_tab += 1; g.tabs.insert(g.active_tab, tab); self.focus(id); self.check(); Ok(())
    }
    pub fn toggle_pin(&mut self, id: GroupId, index: usize) {
        if let Some(tab) = self.group_mut(id).and_then(|g|g.tabs.get_mut(index)) { tab.pinned = !tab.pinned; }
    }
    fn remember(&mut self, id: GroupId, tab: &Tab) {
        self.closed.push((id, tab.pane.path.clone()));
        if self.closed.len() > CLOSED_TABS_REMEMBERED { self.closed.remove(0); }
    }
    fn remove_tab(&mut self, id: GroupId, index: usize) -> Tab {
        let g = self.group_mut(id).unwrap(); let tab = g.tabs.remove(index);
        g.active_tab = if index < g.active_tab { g.active_tab - 1 } else { g.active_tab.min(g.tabs.len().saturating_sub(1)) }; tab
    }
    pub fn close_tab(&mut self, id: GroupId, index: usize) {
        let Some(g) = self.group(id) else { return; };
        if index >= g.tabs.len() || g.tabs[index].pinned || (g.tabs.len() == 1 && self.groups.len() == 1) { return; }
        let tab = self.remove_tab(id,index); self.remember(id,&tab);
        if self.group(id).unwrap().tabs.is_empty() { self.close_group(id); } self.check();
    }
    pub fn close_other_tabs(&mut self, id: GroupId, keep: usize) {
        let Some(g) = self.group(id) else { return; };
        if keep >= g.tabs.len() { return; }
        let active = g.tabs[..keep].iter().filter(|t|t.pinned).count();
        let mut tabs = std::mem::take(&mut self.group_mut(id).unwrap().tabs);
        let mut retained = Vec::new();
        for (i, tab) in tabs.drain(..).enumerate() { if i == keep || tab.pinned { retained.push(tab); } else { self.remember(id,&tab); } }
        let g = self.group_mut(id).unwrap(); g.tabs = retained; g.active_tab = active; self.check();
    }
    pub fn reopen_closed(&mut self) -> bool {
        if self.tab_count() >= MAX_TABS { return false; }
        while let Some((id,path)) = self.closed.pop() {
            let target = if self.group(id).is_some() { id } else { self.active };
            if self.new_tab(target, path).is_ok() { return true; }
        } false
    }
    pub fn move_tab_within(&mut self, id: GroupId, from: usize, to: usize) {
        let Some(g) = self.group_mut(id) else { return; };
        if from >= g.tabs.len() { return; } let to = to.min(g.tabs.len()-1);
        let active = g.active_tab; let tab = g.tabs.remove(from); g.tabs.insert(to,tab);
        g.active_tab = if active == from { to } else if from < active && to >= active { active-1 } else if from > active && to <= active { active+1 } else { active }; self.check();
    }
    pub fn move_tab(&mut self, from: GroupId, index: usize, to: GroupId, position: usize) -> io::Result<()> {
        if self.group(from).is_none_or(|g| index >= g.tabs.len()) || self.group(to).is_none() { return Err(invalid("Вкладка или панель закрыта")); }
        if from == to { self.move_tab_within(from,index,position); return Ok(()); }
        let tab = self.remove_tab(from,index);
        let g = self.group_mut(to).unwrap(); let pos = position.min(g.tabs.len()); g.tabs.insert(pos,tab); g.active_tab = pos;
        if self.group(from).unwrap().tabs.is_empty() { self.close_group(from); }
        self.focus(to); self.check(); Ok(())
    }
    pub fn split(&mut self, id: GroupId, edge: Edge, source: Option<(GroupId, usize)>) -> io::Result<GroupId> {
        if self.groups.len() >= MAX_GROUPS { return Err(invalid("Не больше четырёх панелей")); }
        let g = self.group(id).ok_or_else(|| invalid("Панель закрыта"))?;
        let miller = g.miller;
        if source == Some((id,0)) && g.tabs.len() == 1 { return Err(invalid("Нельзя разделить единственную вкладку переносом")); }
        let copy = if let Some((src,index)) = source {
            self.group(src).and_then(|g|g.tabs.get(index)).ok_or_else(|| invalid("Вкладка закрыта"))?.clone()
        } else {
            if self.tab_count() >= MAX_TABS { return Err(invalid("Не больше ста вкладок")); }
            Tab::new(&g.tabs[g.active_tab].pane.path)?
        };
        let new = GroupId(self.next_id); self.next_id = self.next_id.checked_add(1).ok_or_else(||invalid("Неверный идентификатор панели"))?;
        let (first,second) = if matches!(edge,Edge::Left|Edge::Top) { (Node::Leaf(new),Node::Leaf(id)) } else { (Node::Leaf(id),Node::Leaf(new)) };
        self.root.replace(id,&Node::Split { axis: if matches!(edge,Edge::Left|Edge::Right) { Axis::Horizontal } else { Axis::Vertical }, ratio: 0.5, first: Box::new(first), second: Box::new(second) });
        self.groups.push(Group { id: new, tabs: vec![copy], active_tab: 0, miller });
        if let Some((src,index)) = source {
            self.remove_tab(src,index);
            if self.group(src).unwrap().tabs.is_empty() { self.close_group(src); }
        } self.focus(new); self.check(); Ok(new)
    }
    pub fn close_group(&mut self, id: GroupId) {
        if self.groups.len() <= 1 || self.group(id).is_none() { return; }
        let leaves = self.leaves(); let pos = leaves.iter().position(|leaf|*leaf == id).unwrap();
        let next = if pos > 0 { leaves[pos-1] } else { leaves[pos+1] };
        self.groups.retain(|g|g.id != id); self.recent.retain(|old| *old != id);
        let ids: Vec<_> = self.groups.iter().map(|g|g.id).collect(); self.root = self.root.retain(&ids).unwrap();
        if self.active == id { self.focus(next); } self.check();
    }
    pub fn set_ratio(&mut self, path: &[bool], ratio: f32) {
        if !ratio.is_finite() { return; } let mut node = &mut self.root;
        for second_child in path { match node { Node::Split { first,second,.. } => node = if *second_child { second } else { first }, _ => return } }
        if let Node::Split { ratio: old,.. } = node { *old = ratio.clamp(0.15,0.85); } self.check();
    }
    pub fn leaves(&self) -> Vec<GroupId> { let mut ids = Vec::new(); self.root.leaves(&mut ids); ids }
    pub fn signature(&self) -> String { self.root.signature() }
    pub fn other_group(&self, id: GroupId) -> Option<GroupId> {
        self.recent.iter().rev().copied().find(|old| *old != id && self.group(*old).is_some()).or_else(||self.leaves().into_iter().find(|old| *old != id))
    }
    pub fn neighbour(&self, id: GroupId, edge: Edge) -> Option<GroupId> {
        fn rects(node: &Node, rect: [f32;4], out: &mut Vec<(GroupId,[f32;4])>) {
            match node {
                Node::Leaf(id) => out.push((*id,rect)),
                Node::Split { axis,ratio,first,second } => {
                    let [x,y,w,h] = rect;
                    if *axis == Axis::Horizontal { rects(first,[x,y,w*ratio,h],out); rects(second,[x+w*ratio,y,w*(1.-ratio),h],out); }
                    else { rects(first,[x,y,w,h*ratio],out); rects(second,[x,y+h*ratio,w,h*(1.-ratio)],out); }
                }
            }
        }
        let mut boxes = Vec::new(); rects(&self.root,[0.,0.,1.,1.],&mut boxes);
        let a = boxes.iter().find(|(group,_)| *group == id)?.1;
        boxes.iter().filter(|(group,b)| *group != id && match edge {
            Edge::Left => b[0]+b[2] <= a[0]+0.0001 && b[1] < a[1]+a[3] && b[1]+b[3] > a[1],
            Edge::Right => b[0] >= a[0]+a[2]-0.0001 && b[1] < a[1]+a[3] && b[1]+b[3] > a[1],
            Edge::Top => b[1]+b[3] <= a[1]+0.0001 && b[0] < a[0]+a[2] && b[0]+b[2] > a[0],
            Edge::Bottom => b[1] >= a[1]+a[3]-0.0001 && b[0] < a[0]+a[2] && b[0]+b[2] > a[0],
        }).min_by(|(_,b),(_,c)| {
            let distance = |r: &[f32;4]| (r[0]+r[2]/2.-a[0]-a[2]/2.).powi(2)+(r[1]+r[3]/2.-a[1]-a[3]/2.).powi(2);
            distance(b).total_cmp(&distance(c))
        }).map(|(group,_)| *group)
    }
    fn check(&self) {
        debug_assert!((1..=MAX_GROUPS).contains(&self.groups.len()));
        let ids = self.leaves(); debug_assert_eq!(ids.len(),self.groups.len());
        debug_assert!(self.group(self.active).is_some());
        debug_assert!(self.tab_count() <= MAX_RESTORED_TABS);
        debug_assert!(self.groups.iter().all(|g| !g.tabs.is_empty() && g.active_tab < g.tabs.len() && ids.iter().filter(|id| **id == g.id).count() == 1));
    }
}

pub fn tab_titles(paths: &[PathBuf]) -> Vec<String> {
    let names: Vec<_> = paths.iter().map(|p| display_name(p)).collect();
    names.iter().enumerate().map(|(i,name)| {
        // Only different folders with one name need the parent; two tabs
        // of the same folder would just repeat it.
        if names.iter().zip(paths).any(|(other, path)| other == name && path != &paths[i]) {
            if let Some(parent) = paths[i].parent() { return format!("{name} · {}",display_name(parent)); }
        } name.clone()
    }).collect()
}

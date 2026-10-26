use crate::render::Rect;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Window,
    TabList,
    Tab,
    ToolBar,
    Button,
    Group,
    Link,
    Edit,
    Tree,
    TreeItem,
    List,
    ListItem,
    DataGrid,
    DataItem,
    Header,
    HeaderItem,
    Pane,
    Dialog,
    Text,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: u64,
    pub role: Role,
    pub name: String,
    pub help: String,
    pub value: String,
    pub bounds: Rect,
    pub parent: usize,
    pub children: Vec<usize>,
    pub selected: bool,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub focus: Option<usize>,
}

pub const ROOT_ID: u64 = 1;

impl Tree {
    pub fn new(name: impl Into<String>, bounds: Rect) -> Self {
        let root = Node { id: ROOT_ID, role: Role::Window, name: name.into(), help: String::new(), value: String::new(), bounds, parent: 0, children: Vec::new(), selected: false, enabled: true };
        Self { nodes: vec![root], focus: None }
    }

    pub fn add(&mut self, parent: usize, id: u64, role: Role, name: impl Into<String>, bounds: Rect) -> usize {
        let k = self.nodes.len();
        self.nodes.push(Node { id, role, name: name.into(), help: String::new(), value: String::new(), bounds, parent, children: Vec::new(), selected: false, enabled: true });
        self.nodes[parent].children.push(k);
        k
    }

    pub fn find(&self, id: u64) -> Option<usize> {
        self.nodes.iter().position(|n| n.id == id)
    }

    pub fn sibling(&self, k: usize, step: isize) -> Option<usize> {
        if k == 0 {
            return None;
        }
        let kids = &self.nodes[self.nodes[k].parent].children;
        let at = kids.iter().position(|&c| c == k)? as isize + step;
        usize::try_from(at).ok().and_then(|i| kids.get(i).copied())
    }

    pub fn at(&self, x: f32, y: f32) -> usize {
        let mut k = 0;
        while let Some(&c) = self.nodes[k].children.iter().rev().find(|&&c| self.nodes[c].bounds.contains(x, y)) {
            k = c;
        }
        k
    }

    pub fn by_name(&self, name: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.name == name)
    }
}

#[cfg(test)]
#[path = "../test/a11y.rs"]
mod tests;

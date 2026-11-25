//! The namespace filter: the tree of the namespaces of the image, those of
//! its subjects and of its event types, a box each. A namespace unticked
//! hides its subjects' lines, its types' rows, its subjects' statistics, and
//! the events that name nothing ticked. Ticking or unticking a node does its
//! whole subtree; a node part ticked shows a box neither ticked nor not. The
//! global namespace is a node of its own, the first, parent of none.

use crate::names;
use eframe::egui;
use std::collections::BTreeSet;

/// The name the tree gives the global namespace.
const GLOBAL: &str = "(global)";
/// The line that stands for the global namespace in the state saved.
const SAVED_GLOBAL: &str = "::";

struct Node {
    /// The namespace in full, empty for the global one.
    path: String,
    /// Its last scope, as the tree shows it.
    name: String,
    /// Whether a subject or a type is in it, not only in its nested ones.
    members: bool,
    /// The index after its subtree, the nodes being in the tree's order.
    end: usize,
}

#[derive(Default)]
pub struct Filter {
    /// Depth first, each namespace's nested ones in order after it.
    nodes: Vec<Node>,
    /// The namespaces unticked, those with members only.
    hidden: BTreeSet<String>,
    /// Counts the changes, for what is computed from the filter to know it
    /// is stale.
    generation: u64,
}

impl Filter {
    /// The tree of `namespaces`, as `names::namespace` gives them, and of the
    /// namespaces they nest in; all ticked.
    pub fn new<'a>(namespaces: impl IntoIterator<Item = &'a str>) -> Self {
        let members: BTreeSet<Vec<&str>> = namespaces
            .into_iter()
            .map(|path| {
                if path.is_empty() {
                    Vec::new()
                } else {
                    names::scopes(path)
                }
            })
            .collect();
        let mut all = members.clone();
        for scopes in &members {
            all.extend((1..scopes.len()).map(|n| scopes[..n].to_vec()));
        }
        // In order, a namespace comes right before its nested ones.
        let all: Vec<Vec<&str>> = all.into_iter().collect();
        let nodes = all
            .iter()
            .enumerate()
            .map(|(n, scopes)| Node {
                path: scopes.join("::"),
                name: scopes.last().map_or(GLOBAL, |name| *name).to_owned(),
                members: members.contains(scopes),
                end: match scopes.is_empty() {
                    true => n + 1,
                    false => {
                        n + 1
                            + all[n + 1..]
                                .iter()
                                .take_while(|other| other.starts_with(scopes))
                                .count()
                    }
                },
            })
            .collect();
        Self {
            nodes,
            hidden: BTreeSet::new(),
            generation: 0,
        }
    }

    /// Whether the namespace `path` is ticked; one the tree lacks is.
    pub fn shown(&self, path: &str) -> bool {
        !self.hidden.contains(path)
    }

    /// Whether anything is unticked.
    pub fn active(&self) -> bool {
        !self.hidden.is_empty()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Node `n`'s box: ticked when every namespace with members in its
    /// subtree is, unticked when none is, None between.
    fn state(&self, n: usize) -> Option<bool> {
        let mut members = self.nodes[n..self.nodes[n].end].iter().filter(|node| node.members);
        let first = self.shown(&members.next()?.path);
        members.all(|node| self.shown(&node.path) == first).then_some(first)
    }

    /// Ticks or unticks node `n` and its subtree.
    fn set(&mut self, n: usize, shown: bool) {
        for node in self.nodes[n..self.nodes[n].end].iter().filter(|node| node.members) {
            if shown {
                self.hidden.remove(&node.path);
            } else {
                self.hidden.insert(node.path.clone());
            }
        }
        self.generation += 1;
    }

    /// A click on node `n`'s box: unticks its subtree when all ticked, else
    /// ticks it.
    pub fn toggle(&mut self, n: usize) {
        self.set(n, self.state(n) != Some(true));
    }

    /// Ticks node `n` and its subtree, and unticks every other node.
    pub fn isolate(&mut self, n: usize) {
        self.hidden = self
            .nodes
            .iter()
            .filter(|node| node.members)
            .map(|node| node.path.clone())
            .collect();
        self.set(n, true);
    }

    /// Ticks every node.
    pub fn all(&mut self) {
        self.hidden.clear();
        self.generation += 1;
    }

    /// The node of the namespace `path`.
    #[cfg(test)]
    pub fn node(&self, path: &str) -> Option<usize> {
        self.nodes.iter().position(|node| node.path == path)
    }

    /// The namespaces unticked, a line each, to be restored.
    pub fn saved(&self) -> String {
        let line = |path: &String| if path.is_empty() { SAVED_GLOBAL } else { path.as_str() }.to_owned();
        self.hidden.iter().map(line).collect::<Vec<_>>().join("\n")
    }

    /// Unticks the namespaces `saved` gives, those the tree has with members;
    /// the others are forgotten.
    pub fn restore(&mut self, saved: &str) {
        self.hidden = saved
            .lines()
            .map(|line| if line == SAVED_GLOBAL { "" } else { line })
            .filter(|path| self.nodes.iter().any(|node| node.members && node.path == *path))
            .map(str::to_owned)
            .collect();
        self.generation += 1;
    }

    /// The tree, a box per node, each with a button to show it alone.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Namespaces");
            if ui
                .add_enabled(self.active(), egui::Button::new("All"))
                .on_hover_text("Tick every namespace")
                .clicked()
            {
                self.all();
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let mut n = 0;
            while n < self.nodes.len() {
                n = self.subtree(ui, n);
            }
        });
    }

    /// Node `n` and, under it, its nested namespaces; the index after them.
    fn subtree(&mut self, ui: &mut egui::Ui, n: usize) -> usize {
        let end = self.nodes[n].end;
        if end == n + 1 {
            ui.horizontal(|ui| self.header(ui, n));
            return end;
        }
        let id = ui.make_persistent_id(("namespace", &self.nodes[n].path));
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
            .show_header(ui, |ui| self.header(ui, n))
            .body(|ui| {
                let mut m = n + 1;
                while m < end {
                    m = self.subtree(ui, m);
                }
            });
        end
    }

    /// Node `n`'s box and its button to show it alone.
    fn header(&mut self, ui: &mut egui::Ui, n: usize) {
        let state = self.state(n);
        let mut ticked = state == Some(true);
        let node = &self.nodes[n];
        let name = egui::RichText::new(&node.name);
        let name = if node.members { name } else { name.weak() };
        let full = if node.path.is_empty() {
            GLOBAL
        } else {
            node.path.as_str()
        }
        .to_owned();
        let toggled = ui
            .add(egui::Checkbox::new(&mut ticked, name).indeterminate(state.is_none()))
            .on_hover_text(full)
            .clicked();
        let isolated = ui
            .small_button("only")
            .on_hover_text("Show this namespace alone, with its nested ones")
            .clicked();
        if toggled {
            self.toggle(n);
        }
        if isolated {
            self.isolate(n);
        }
    }
}

#[cfg(test)]
mod tests;

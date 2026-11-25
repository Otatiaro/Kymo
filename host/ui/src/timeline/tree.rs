//! The column of names as a tree. Within a section, the lines of a namespace
//! that holds two or more of them sit under a heading at its name; each line
//! is joined to its heading by a guide and named without it. The namespace
//! every line of the section shares is left out, so a section of a single
//! namespace has no heading.

/// An entry of the column, in order: a heading or a line of the section.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Node {
    pub(super) kind: Kind,
    /// What the column shows: the name without the headings above it.
    pub(super) text: String,
    /// How many headings it is under.
    pub(super) depth: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Kind {
    /// A namespace: its path, whether it is folded, and the lines under it.
    Heading { path: String, folded: bool, count: usize },
    /// The line of the section's `n`th name.
    Line(usize),
}

/// The tree of a section whose lines are named `names`, as scopes, the
/// namespaces in order: a namespace's own lines before its nested ones'.
/// The lines under a heading `folded` says, by path, are left out.
pub(super) fn tree(names: &[Vec<&str>], folded: impl Fn(&str) -> bool) -> Vec<Node> {
    let space = |n: usize| &names[n][..names[n].len() - 1];
    let Some(first) = names.first() else {
        return Vec::new();
    };
    let first = &first[..first.len() - 1];
    let shared = (0..names.len())
        .map(|n| space(n).iter().zip(first).take_while(|(a, b)| a == b).count())
        .min()
        .unwrap_or(0);
    // A namespace has a heading when it holds two lines or more, and they
    // are not all in one namespace nested in it, which has it instead.
    let heading = |prefix: &[&str]| {
        let members: Vec<usize> = (0..names.len()).filter(|&n| space(n).starts_with(prefix)).collect();
        let below = |n: usize| space(n).get(prefix.len());
        let one =
            members.first().and_then(|&n| below(n)).is_some() && members.iter().all(|&n| below(n) == below(members[0]));
        (members.len() >= 2 && !one).then_some(members.len())
    };
    let mut nodes = Vec::new();
    // The headings the current line is under, and whether each is folded.
    let mut open: Vec<(&[&str], bool)> = Vec::new();
    for (n, name) in names.iter().enumerate() {
        let space = space(n);
        while open.last().is_some_and(|(prefix, _)| !space.starts_with(prefix)) {
            open.pop();
        }
        let from = |open: &[(&[&str], bool)]| open.last().map_or(shared, |(prefix, _)| prefix.len());
        for len in from(&open) + 1..=space.len() {
            let prefix = &space[..len];
            if let Some(count) = heading(prefix) {
                let path = prefix.join("::");
                let shut = folded(&path);
                if !open.iter().any(|(_, folded)| *folded) {
                    nodes.push(Node {
                        text: prefix[from(&open)..].join("::"),
                        depth: open.len(),
                        kind: Kind::Heading {
                            path,
                            folded: shut,
                            count,
                        },
                    });
                }
                open.push((prefix, shut));
            }
        }
        if !open.iter().any(|(_, folded)| *folded) {
            nodes.push(Node {
                kind: Kind::Line(n),
                text: name[from(&open)..].join("::"),
                depth: open.len(),
            });
        }
    }
    nodes
}

/// The guides of an entry under a heading: for each heading above its own,
/// outermost first, whether a guide runs down past it, and whether it is
/// the last under its heading.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Guide {
    pub(super) through: Vec<bool>,
    pub(super) last: bool,
}

/// The guides of entries at `depths`, in order.
pub(super) fn guides(depths: &[usize]) -> Vec<Guide> {
    (0..depths.len())
        .map(|n| {
            // Whether an entry at `depth` follows before one shallower.
            let follows = |depth: usize| {
                depths[n + 1..]
                    .iter()
                    .take_while(|&&other| other >= depth)
                    .any(|&other| other == depth)
            };
            let depth = depths[n];
            if depth == 0 {
                return Guide::default();
            }
            Guide {
                through: (1..depth).map(follows).collect(),
                last: !follows(depth),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;

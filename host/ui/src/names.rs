//! The names the rows show: a subject's name from the image, without what
//! every name repeats. The hover gives the name in full.

use kymo::meaning::Subject;

use std::collections::HashMap;

const ANONYMOUS: &str = "(anonymous namespace)";

/// The names shown for each list of `subjects`, in its order: without the
/// anonymous namespaces nor the leading ones all subjects share, each name
/// keeping its last component; names alike within a list take their address.
pub fn shown(subjects: &[&[Subject]]) -> Vec<Vec<String>> {
    let components: Vec<Vec<Vec<&str>>> = subjects
        .iter()
        .map(|list| list.iter().map(|subject| scopes(&subject.name)).collect())
        .collect();
    let all = || components.iter().flatten();
    let shortest = all().map(|name| name.len() - 1).min().unwrap_or(0);
    let shared = (0..shortest)
        .take_while(|&n| all().all(|name| name[n] == all().next().expect("a name")[n]))
        .count();
    subjects
        .iter()
        .zip(&components)
        .map(|(list, names)| {
            let names: Vec<String> = names.iter().map(|name| name[shared..].join("::")).collect();
            let mut alike: HashMap<&str, usize> = HashMap::new();
            for name in &names {
                *alike.entry(name).or_default() += 1;
            }
            names
                .iter()
                .zip(*list)
                .map(|(name, subject)| match alike[name.as_str()] {
                    1 => name.clone(),
                    _ => format!("{name} @0x{:08x}", subject.address),
                })
                .collect()
        })
        .collect()
}

/// The names shown for event `types`, in their order: each type's last
/// component, the anonymous namespaces left out; types alike in it take
/// scopes from the left until they differ.
pub fn types(types: &[&str]) -> Vec<String> {
    let scopes: Vec<Vec<&str>> = types.iter().map(|name| scopes(name)).collect();
    let mut kept = vec![1; types.len()];
    loop {
        let shown: Vec<&[&str]> = scopes
            .iter()
            .zip(&kept)
            .map(|(scopes, &kept)| &scopes[scopes.len() - kept..])
            .collect();
        let alike: Vec<usize> = (0..shown.len())
            .filter(|&n| kept[n] < scopes[n].len() && (0..shown.len()).any(|m| m != n && shown[m] == shown[n]))
            .collect();
        if alike.is_empty() {
            return shown.iter().map(|scopes| scopes.join("::")).collect();
        }
        for n in alike {
            kept[n] += 1;
        }
    }
}

/// The indices of `subjects` grouped by namespace: namespaces compared scope
/// by scope, the anonymous ones left out, a namespace's own members before
/// its nested namespaces'; within one, by name, then by address.
pub fn by_namespace(subjects: &[Subject]) -> Vec<usize> {
    let scopes: Vec<Vec<&str>> = subjects.iter().map(|subject| scopes(&subject.name)).collect();
    let mut order: Vec<usize> = (0..subjects.len()).collect();
    order.sort_by(|&a, &b| {
        let (a_scopes, b_scopes) = (&scopes[a], &scopes[b]);
        let (a_space, a_name) = a_scopes.split_at(a_scopes.len() - 1);
        let (b_space, b_name) = b_scopes.split_at(b_scopes.len() - 1);
        a_space
            .cmp(b_space)
            .then_with(|| a_name.cmp(b_name))
            .then_with(|| subjects[a].address.cmp(&subjects[b].address))
    });
    order
}

/// The namespace of a qualified `name`: its scopes but the last, the
/// anonymous namespaces left out, joined by `::`; empty for the global
/// namespace. A handler's interrupt, after its name, is part of the last.
pub fn namespace(name: &str) -> String {
    let scopes = scopes(name);
    scopes[..scopes.len() - 1].join("::")
}

/// The scopes of a qualified `name`, the anonymous namespaces left out: a
/// `::` inside template arguments or a parameter list separates none.
pub fn scopes(name: &str) -> Vec<&str> {
    let mut scopes = Vec::new();
    let (mut depth, mut start) = (0i32, 0);
    let bytes = name.as_bytes();
    let mut n = 0;
    while n < bytes.len() {
        match bytes[n] {
            b'<' | b'(' | b'[' => depth += 1,
            b'>' | b')' | b']' => depth -= 1,
            b':' if depth == 0 && bytes.get(n + 1) == Some(&b':') => {
                scopes.push(&name[start..n]);
                start = n + 2;
                n += 1;
            }
            _ => {}
        }
        n += 1;
    }
    scopes.push(&name[start..]);
    scopes.retain(|scope| *scope != ANONYMOUS);
    if scopes.is_empty() {
        scopes.push(name);
    }
    scopes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject(name: &str, address: u64) -> Subject {
        Subject {
            address,
            name: name.to_owned(),
            priority: None,
            priorities: Vec::new(),
        }
    }

    fn names(lists: &[&[(&str, u64)]]) -> Vec<Vec<String>> {
        let lists: Vec<Vec<Subject>> = lists
            .iter()
            .map(|list| list.iter().map(|&(name, address)| subject(name, address)).collect())
            .collect();
        let slices: Vec<&[Subject]> = lists.iter().map(Vec::as_slice).collect();
        shown(&slices)
    }

    /// The namespaces every subject shares, anonymous ones aside, are left
    /// out, across tasks, handlers and mutexes alike.
    #[test]
    fn the_namespaces_all_share_are_left_out() {
        assert_eq!(
            names(&[
                &[
                    ("app::(anonymous namespace)::g_logger", 1),
                    ("app::(anonymous namespace)::g_sampler", 2)
                ],
                &[("app::(anonymous namespace)::timer_isr", 3)],
                &[("app::(anonymous namespace)::g_mutex", 4)],
            ]),
            [vec!["g_logger", "g_sampler"], vec!["timer_isr"], vec!["g_mutex"]]
        );
        assert_eq!(
            names(&[&[("app::io::uart", 1), ("app::io::spi", 2), ("app::main_task", 3)]]),
            [vec!["io::uart", "io::spi", "main_task"]]
        );
    }

    /// A namespace some subject lacks stays; so does a lone name's last
    /// component, and a name from an address.
    #[test]
    fn what_not_all_share_stays() {
        assert_eq!(names(&[&[("app::a", 1), ("drv::b", 2)]]), [vec!["app::a", "drv::b"]]);
        assert_eq!(names(&[&[("app::a", 1), ("b", 2)]]), [vec!["app::a", "b"]]);
        assert_eq!(
            names(&[&[("app::a", 1)], &[("0x20000010", 2)]]),
            [vec!["app::a"], vec!["0x20000010"]]
        );
        assert_eq!(names(&[&[("app::a", 1)]]), [vec!["a"]]);
    }

    /// A handler's interrupt, after its name, keeps the namespaces it shares
    /// with the other subjects.
    #[test]
    fn a_handler_named_with_its_interrupt_shares_the_namespaces() {
        assert_eq!(
            names(&[
                &[("app::(anonymous namespace)::g_logger", 1)],
                &[("app::(anonymous namespace)::timer_isr (IRQ45)", 2)],
            ]),
            [vec!["g_logger"], vec!["timer_isr (IRQ45)"]]
        );
    }

    /// A type shows its last component, without the anonymous namespaces.
    #[test]
    fn a_type_shows_its_last_component() {
        assert_eq!(
            types(&["kymo::opsy::task_started", "app::(anonymous namespace)::sample", "tick"]),
            ["task_started", "sample", "tick"]
        );
    }

    /// Types alike in their last component take scopes until they differ;
    /// the others keep theirs alone.
    #[test]
    fn types_alike_take_scopes_until_they_differ() {
        assert_eq!(
            types(&[
                "app::sensors::sample_ready",
                "app::comms::sample_ready",
                "kymo::opsy::task_ready"
            ]),
            ["sensors::sample_ready", "comms::sample_ready", "task_ready"]
        );
        assert_eq!(
            types(&["a::x::io::done", "b::x::io::done", "c::y::done"]),
            ["a::x::io::done", "b::x::io::done", "y::done"]
        );
    }

    /// Types alike in full, as two translation units' anonymous namespaces
    /// give, keep all their scopes.
    #[test]
    fn types_alike_in_full_keep_all_their_scopes() {
        assert_eq!(
            types(&[
                "app::(anonymous namespace)::sample",
                "app::(anonymous namespace)::sample",
                "tick"
            ]),
            ["app::sample", "app::sample", "tick"]
        );
    }

    /// A `::` inside template arguments is no scope of the type.
    #[test]
    fn template_arguments_are_no_scope_of_a_type() {
        assert_eq!(types(&["app::event<app::x>", "app::other"]), ["event<app::x>", "other"]);
    }

    /// A `::` inside template arguments or a parameter list is no namespace.
    #[test]
    fn template_arguments_are_no_namespace() {
        assert_eq!(
            names(&[&[("app::pool<app::x, 4>::slot", 1), ("app::b", 2)]]),
            [vec!["pool<app::x, 4>::slot", "b"]]
        );
        assert_eq!(
            names(&[&[("app::f(app::x)::s", 1), ("app::b", 2)]]),
            [vec!["f(app::x)::s", "b"]]
        );
    }

    /// Subjects group by namespace, a global name first, a namespace's own
    /// members before its nested namespaces', the anonymous ones left out;
    /// within one by name, then by address.
    #[test]
    fn subjects_group_by_namespace() {
        let subjects: Vec<Subject> = [
            ("drv::spi::g_bus", 1),
            ("app::(anonymous namespace)::g_tick", 2),
            ("app::sub::g_a", 3),
            ("g_global", 4),
            ("app::g_z", 0x30),
            ("drv::g_dma", 6),
            ("app::(anonymous namespace)::g_z", 0x10),
            ("app::g_b", 8),
            ("app::z_last", 9),
        ]
        .iter()
        .map(|&(name, address)| subject(name, address))
        .collect();
        let order: Vec<(&str, u64)> = by_namespace(&subjects)
            .into_iter()
            .map(|n| (subjects[n].name.as_str(), subjects[n].address))
            .collect();
        assert_eq!(
            order,
            [
                ("g_global", 4),
                ("app::g_b", 8),
                ("app::(anonymous namespace)::g_tick", 2),
                ("app::(anonymous namespace)::g_z", 0x10),
                ("app::g_z", 0x30),
                ("app::z_last", 9),
                ("app::sub::g_a", 3),
                ("drv::g_dma", 6),
                ("drv::spi::g_bus", 1),
            ]
        );
    }

    /// A namespace is the scopes but the last, without the anonymous ones;
    /// a name without one, or an address, is in the global namespace.
    #[test]
    fn a_namespace_is_the_scopes_but_the_last() {
        assert_eq!(
            namespace("app::sensors::(anonymous namespace)::g_sampler"),
            "app::sensors"
        );
        assert_eq!(namespace("app::sensors::timer_isr (IRQ45)"), "app::sensors");
        assert_eq!(namespace("app::pool<drv::x>::slot"), "app::pool<drv::x>");
        assert_eq!(namespace("(anonymous namespace)::g_x"), "");
        assert_eq!(namespace("g_console"), "");
        assert_eq!(namespace("0x20000010"), "");
    }

    /// Names alike once shortened, as statics of two translation units in
    /// anonymous namespaces, take their address.
    #[test]
    fn names_alike_take_their_address() {
        assert_eq!(
            names(&[&[
                ("app::(anonymous namespace)::g_mutex", 0x2000_0010),
                ("app::(anonymous namespace)::g_mutex", 0x2000_0400),
                ("app::g_other", 0x2000_0800),
            ]]),
            [vec!["g_mutex @0x20000010", "g_mutex @0x20000400", "g_other"]]
        );
    }
}

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

/// The scopes of a qualified `name`, the anonymous namespaces left out: a
/// `::` inside template arguments or a parameter list separates none.
fn scopes(name: &str) -> Vec<&str> {
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

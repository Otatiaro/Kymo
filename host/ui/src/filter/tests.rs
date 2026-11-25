use super::*;

/// The namespaces of `modules.cpp`'s subjects, and of OpSy's event types.
fn filter() -> Filter {
    Filter::new([
        "app::sensors",
        "app::sensors::imu",
        "app::comms",
        "app::storage",
        "drv",
        "",
        "kymo::opsy",
        "app::sensors",
    ])
}

fn paths(filter: &Filter) -> Vec<(&str, bool, usize)> {
    filter
        .nodes
        .iter()
        .map(|node| (node.path.as_str(), node.members, node.end))
        .collect()
}

fn node(filter: &Filter, path: &str) -> usize {
    filter.node(path).expect("a node of the tree")
}

fn hidden(filter: &Filter) -> Vec<&str> {
    filter.hidden.iter().map(String::as_str).collect()
}

/// The global namespace first and parent of none, each namespace right
/// before its nested ones, the namespaces they nest in added without members.
#[test]
fn the_tree_nests_each_namespace_under_its_parent() {
    assert_eq!(
        paths(&filter()),
        [
            ("", true, 1),
            ("app", false, 6),
            ("app::comms", true, 3),
            ("app::sensors", true, 5),
            ("app::sensors::imu", true, 5),
            ("app::storage", true, 6),
            ("drv", true, 7),
            ("kymo", false, 9),
            ("kymo::opsy", true, 9),
        ]
    );
}

/// Unticking a node hides its nested namespaces too; its parent is then
/// part ticked, and ticking it again shows them all.
#[test]
fn a_node_unticked_hides_its_nested_namespaces() {
    let mut filter = filter();
    assert!(!filter.active());
    filter.toggle(node(&filter, "app::sensors"));
    assert_eq!(hidden(&filter), ["app::sensors", "app::sensors::imu"]);
    assert!(!filter.shown("app::sensors::imu"));
    assert!(filter.shown("app::comms") && filter.shown("") && filter.shown("drv"));
    assert_eq!(filter.state(node(&filter, "app")), None);
    assert_eq!(filter.state(node(&filter, "app::sensors")), Some(false));
    // The global namespace is no parent of the others.
    assert_eq!(filter.state(node(&filter, "")), Some(true));
    filter.toggle(node(&filter, "app"));
    assert!(!filter.active());
    filter.toggle(node(&filter, "app"));
    assert_eq!(
        hidden(&filter),
        ["app::comms", "app::sensors", "app::sensors::imu", "app::storage"]
    );
    filter.toggle(node(&filter, "app::sensors::imu"));
    assert_eq!(filter.state(node(&filter, "app::sensors")), None);
}

/// Unticking the global namespace hides only its own members.
#[test]
fn the_global_namespace_hides_only_its_own() {
    let mut filter = filter();
    filter.toggle(node(&filter, ""));
    assert_eq!(hidden(&filter), [""]);
}

/// Showing a node alone ticks its subtree and unticks every other node.
#[test]
fn a_node_alone_unticks_every_other() {
    let mut filter = filter();
    filter.isolate(node(&filter, "app::sensors"));
    assert_eq!(hidden(&filter), ["", "app::comms", "app::storage", "drv", "kymo::opsy"]);
    filter.all();
    assert!(!filter.active());
}

/// The state saved restores in a tree of the same namespaces; a namespace
/// the next tree lacks is forgotten.
#[test]
fn a_namespace_the_next_capture_lacks_is_forgotten() {
    let mut filter = filter();
    filter.toggle(node(&filter, ""));
    filter.toggle(node(&filter, "app::comms"));
    filter.toggle(node(&filter, "kymo"));
    let saved = filter.saved();
    let mut again = super::tests::filter();
    let before = again.generation();
    again.restore(&saved);
    assert_eq!(hidden(&again), ["", "app::comms", "kymo::opsy"]);
    assert!(again.generation() > before);
    let mut other = Filter::new(["app::comms", "drv"]);
    other.restore(&saved);
    assert_eq!(hidden(&other), ["app::comms"]);
    // A namespace without members of its own is never saved.
    other.restore("app\nnone");
    assert!(!other.active());
}

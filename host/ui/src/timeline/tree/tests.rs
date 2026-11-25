use super::*;

fn names(names: &[&'static str]) -> Vec<Vec<&'static str>> {
    names.iter().map(|name| name.split("::").collect()).collect()
}

/// Each entry as its depth in spaces, then a heading's text after `>` or a
/// line's.
fn shown(nodes: &[Node]) -> Vec<String> {
    nodes
        .iter()
        .map(|node| {
            let indent = " ".repeat(node.depth);
            match &node.kind {
                Kind::Heading { folded, count, .. } => {
                    format!(
                        "{indent}>{}{}",
                        node.text,
                        if *folded { format!(" ({count})") } else { String::new() }
                    )
                }
                Kind::Line(_) => format!("{indent}{}", node.text),
            }
        })
        .collect()
}

const TASKS: [&str; 5] = [
    "app::comms::g_uplink",
    "app::sensors::g_sampler",
    "app::sensors::imu::g_fusion",
    "app::sensors::imu::g_filter",
    "app::storage::g_logger",
];

#[test]
fn a_namespace_of_two_lines_or_more_heads_them() {
    let nodes = tree(&names(&TASKS), |_| false);
    assert_eq!(
        shown(&nodes),
        [
            "comms::g_uplink",
            ">sensors",
            " g_sampler",
            " >imu",
            "  g_fusion",
            "  g_filter",
            "storage::g_logger",
        ]
    );
    assert_eq!(
        nodes[1].kind,
        Kind::Heading {
            path: "app::sensors".to_owned(),
            folded: false,
            count: 3
        }
    );
    assert_eq!(nodes[2].kind, Kind::Line(1));
}

#[test]
fn a_section_of_one_namespace_has_no_heading() {
    let nodes = tree(&names(&["drv::g_spi", "drv::g_i2c"]), |_| false);
    assert_eq!(shown(&nodes), ["g_spi", "g_i2c"]);
}

#[test]
fn a_namespace_whose_lines_are_all_nested_gives_its_heading_to_theirs() {
    let nodes = tree(&names(&["g_console", "app::imu::g_a", "app::imu::g_b"]), |_| false);
    assert_eq!(shown(&nodes), ["g_console", ">app::imu", " g_a", " g_b"]);
}

#[test]
fn a_heading_folded_leaves_out_what_is_under_it() {
    let nodes = tree(&names(&TASKS), |path| path == "app::sensors");
    assert_eq!(shown(&nodes), ["comms::g_uplink", ">sensors (3)", "storage::g_logger"]);
}

#[test]
fn the_guides_run_down_to_the_last_under_each_heading() {
    // >sensors, g_sampler, >imu, g_fusion, g_filter, storage
    let guides = guides(&[0, 1, 1, 2, 2, 0]);
    let at = |through: &[bool], last| Guide {
        through: through.to_vec(),
        last,
    };
    assert_eq!(
        guides,
        [
            Guide::default(),
            at(&[], false),
            at(&[], true),
            at(&[false], false),
            at(&[false], true),
            Guide::default(),
        ]
    );
    assert_eq!(super::guides(&[0, 1, 2, 1])[2], at(&[true], true));
    // Two headings side by side: the first's guide stops at its last line.
    assert_eq!(super::guides(&[0, 1, 0, 1])[1], at(&[], true));
}

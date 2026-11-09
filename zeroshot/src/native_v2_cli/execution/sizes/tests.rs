use openengine_cluster_protocol::{RunSize, RunSizeDescriptor};
use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

fn catalog() -> TargetRunSizesDiscovery {
    let id = RunSize::new("lab-custom-48").assert_value();
    TargetRunSizesDiscovery::new(
        Some(id.clone()),
        vec![RunSizeDescriptor::new(id, "48 vCPU, 192 GiB RAM, 1024 GiB disk").assert_value()],
    )
    .assert_value()
}

fn rendered(catalog: &TargetRunSizesDiscovery, width: usize) -> String {
    let mut output = Vec::new();
    write_table(&mut output, catalog, width).assert_value();
    String::from_utf8(output).assert_value()
}

#[test]
fn table_displays_target_owned_identifiers_and_descriptions() {
    let text = rendered(&catalog(), 80);
    assert_eq!(
        text.lines()
            .next()
            .assert_value()
            .split_whitespace()
            .collect::<Vec<_>>(),
        ["Identifier", "Description"]
    );
    assert!(text.contains("lab-custom-48  48 vCPU, 192 GiB RAM, 1024 GiB disk"));
    assert!(!text.contains("medium"));
}

#[test]
fn narrow_table_wraps_descriptions_and_long_identifiers_without_losing_text() {
    let mut catalog = catalog();
    let id = "one-target-owned-identifier-with-many-segments";
    catalog.default = Some(RunSize::new(id).assert_value());
    catalog.sizes[0].id = RunSize::new(id).assert_value();
    let text = rendered(&catalog, 40);
    assert!(text.lines().all(|line| line.chars().count() <= 40));
    assert_eq!(wrap_identifier(id, 13).concat(), id);
    assert_eq!(
        wrap(&catalog.sizes[0].description, 25).join(" "),
        catalog.sizes[0].description,
    );
}

#[test]
fn identifiers_with_whitespace_are_quoted_and_preserved() {
    for value in ["a  box", " box ", "  "] {
        let id = RunSize::new(value).assert_value();
        let catalog = TargetRunSizesDiscovery::new(
            Some(id.clone()),
            vec![RunSizeDescriptor::new(id, "Target-owned description").assert_value()],
        )
        .assert_value();
        let text = rendered(&catalog, 80);
        assert!(text.contains(&serde_json::to_string(value).assert_value()));
        assert_eq!(wrap_identifier(value, 2).concat(), value);
    }
}

#[test]
fn unicode_columns_use_terminal_width() {
    let id = RunSize::new("大きい箱").assert_value();
    let catalog = TargetRunSizesDiscovery::new(
        Some(id.clone()),
        vec![
            RunSizeDescriptor::new(id, "一二三四五六七八九十 repeated description words")
                .assert_value(),
        ],
    )
    .assert_value();
    let text = rendered(&catalog, 40);
    assert!(text.lines().all(|line| line.width() <= 40));
    assert_eq!(wrap_identifier("大きい箱", 3).concat(), "大きい箱");
}

#[test]
fn empty_catalog_does_not_invent_choices() {
    let mut output = Vec::new();
    write_table(
        &mut output,
        &TargetRunSizesDiscovery::new(None, vec![]).assert_value(),
        80,
    )
    .assert_value();
    assert_eq!(
        String::from_utf8(output).assert_value(),
        "No sizes are currently offered by this target.\n"
    );
}

pub(super) fn script() -> String {
    openengine_cluster_testkit::fixture::python3_script(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/harnesses/adapters/core/copilot/tests/runtime.py"
        ))
        .unwrap(),
    )
}

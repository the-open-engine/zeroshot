use super::release;

#[test]
fn version_output_reads_only_a_numeric_leading_release() {
    assert_eq!(release("2.1.310 (Claude Code)"), Some([2, 1, 310]));
    assert_eq!(
        release("2.1.318 (Claude Code) dev-build"),
        Some([2, 1, 318])
    );
    assert_eq!(release(" 10.0.1\n"), Some([10, 0, 1]));
    for unsupported in [
        "",
        "Claude Code",
        "v2.1.318",
        "2.1",
        "2.1.310.1",
        "2.1.+310",
        "2.1.310-beta",
    ] {
        assert_eq!(release(unsupported), None, "{unsupported:?}");
    }
}

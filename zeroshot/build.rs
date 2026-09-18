fn main() {
    if std::env::var_os("CARGO_FEATURE_UI").is_some() {
        println!("cargo:rerun-if-changed=../ui/dist");
        let assets = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo supplies its manifest directory"),
        )
        .join("../ui/dist/index.html");
        assert!(
            assets.is_file(),
            "UI assets are missing. Run `npm --prefix ui ci && npm --prefix ui run build` before building with --features ui."
        );
    }
}

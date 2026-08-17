use super::*;

#[test]
fn static_main_uses_available_viewport_width() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let base_css = fs::read_to_string(static_root.join("css/base.css")).unwrap();
    let main_start = base_css
        .find("\nmain {\n")
        .expect("shared main layout should exist");
    let main_end = main_start
        + base_css[main_start..]
            .find("\n}\n")
            .expect("shared main layout should close")
        + "\n}\n".len();
    let main = &base_css[main_start..main_end];

    assert!(main.contains("padding: 24px 32px 48px;"));
    assert!(main.contains("width: 100%;"));
    assert!(main.contains("flex: 1 1 auto;"));
    assert!(main.contains("min-height: 0;"));
    assert!(main.contains("overflow-y: auto;"));
    assert!(
        !main.contains("max-width:"),
        "shared main layout should not cap wide viewports"
    );
    assert!(base_css.contains("@media (max-width: 700px)"));
    assert!(base_css.contains("  main {\n    padding: 24px 32px 48px;\n  }"));
}

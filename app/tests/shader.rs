//! Validates the WGSL on the host. A shader error would otherwise only show up as a
//! crash on the phone the moment the app starts.

const SRC: &str = include_str!("../src/shader.wgsl");

#[test]
fn shader_parses_and_validates() {
    let module = naga::front::wgsl::parse_str(SRC).unwrap_or_else(|e| panic!("{}", e.emit_to_string(SRC)));
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
        .validate(&module)
        .expect("WGSL validation failed");
}

#[test]
fn shader_has_all_entry_points() {
    let module = naga::front::wgsl::parse_str(SRC).expect("parse");
    let names: Vec<String> = module.entry_points.iter().map(|e| e.name.clone()).collect();
    for want in ["vs_main", "fs_main", "vs_lod", "vs_sky", "fs_sky", "vs_cloud", "fs_cloud", "fs_mask", "fs_rays", "vs_char", "fs_char"] {
        assert!(names.iter().any(|n| n == want), "missing entry point {want}");
    }
}

#[test]
fn title_shader_parses_and_validates() {
    const TITLE: &str = include_str!("../src/title.wgsl");
    let module = naga::front::wgsl::parse_str(TITLE).unwrap_or_else(|e| panic!("{}", e.emit_to_string(TITLE)));
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
        .validate(&module)
        .expect("title WGSL validation failed");
    let names: Vec<String> = module.entry_points.iter().map(|e| e.name.clone()).collect();
    assert!(names.contains(&"vs_title".to_string()) && names.contains(&"fs_title".to_string()));
}

#[test]
fn ui_shader_parses_and_validates() {
    const UI: &str = include_str!("../src/ui.wgsl");
    let module = naga::front::wgsl::parse_str(UI).unwrap_or_else(|e| panic!("{}", e.emit_to_string(UI)));
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
        .validate(&module)
        .expect("UI WGSL validation failed");
    let names: Vec<String> = module.entry_points.iter().map(|e| e.name.clone()).collect();
    assert!(names.contains(&"vs_ui".to_string()) && names.contains(&"fs_ui".to_string()));
}

//! Compiles the markup. The fluent style supplies `Palette` (the desktop colour
//! scheme) and `AboutSlint`; the controls themselves are drawn in `ui/look/breeze`.

fn main() -> Result<(), slint_build::CompileError> {
    // Debug builds keep element ids, so the tests can walk the accessible tree.
    let debug = std::env::var("PROFILE").is_ok_and(|p| p == "debug");
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent".to_owned())
        .with_debug_info(debug);
    slint_build::compile_with_config("ui/app.slint", config)
}

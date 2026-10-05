//! Compiles the markup. The fluent style supplies `Palette` (the desktop colour
//! scheme) and `AboutSlint`; the controls themselves are drawn in `ui/look/breeze`.

fn main() -> Result<(), slint_build::CompileError> {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".to_owned());
    slint_build::compile_with_config("ui/app.slint", config)
}

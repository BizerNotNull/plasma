fn main() {
    slint_build::compile_with_config(
        "ui/main.slint",
        slint_build::CompilerConfiguration::new().with_style("cosmic-dark".into()),
    )
    .expect("failed to compile the PLASMA interface");
}

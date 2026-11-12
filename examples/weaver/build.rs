fn main() -> Result<(), Box<dyn std::error::Error>> {
    slogr_build::Builder::new("semconv")
        .group("registry.slogr_example")
        .generate()?;
    Ok(())
}

use spirv_builder::SpirvBuilder;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let shader_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("shader");

    let mut builder = SpirvBuilder::new(shader_path, "spirv-unknown-vulkan1.1");
    builder.build_script.defaults = true;
    builder.build_script.env_shader_spv_path = Some(true);
    builder.build()?;

    Ok(())
}

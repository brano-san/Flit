fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=shaders/ui.wgsl");
    #[cfg(target_os = "linux")]
    glsl::generate()?;
    Ok(())
}

#[cfg(target_os = "linux")]
mod glsl {
    use naga::back::glsl::{Options, PipelineOptions, Version, Writer, WriterFlags};
    use naga::valid::{Capabilities, ValidationFlags, Validator};
    use naga::ShaderStage;
    use std::fmt::Write;
    use std::{env, fs, path::PathBuf};

    pub fn generate() -> Result<(), Box<dyn std::error::Error>> {
        let src = fs::read_to_string("shaders/ui.wgsl")?;
        let module = naga::front::wgsl::parse_str(&src).map_err(|e| e.emit_to_string(&src))?;
        let info = Validator::new(ValidationFlags::all(), Capabilities::DUAL_SOURCE_BLENDING).validate(&module).map_err(|e| e.emit_to_string(&src))?;
        let options = Options { version: Version::new_gles(300), writer_flags: WriterFlags::empty(), binding_map: Default::default(), zero_initialize_workgroup_memory: false };
        let mut rust = String::new();
        let mut blocks = Vec::new();
        let mut samplers = Vec::new();
        for (stage, entry, name) in [(ShaderStage::Vertex, "vs_main", "VERTEX"), (ShaderStage::Fragment, "fs_main", "FRAGMENT"), (ShaderStage::Fragment, "fs_lcd", "FRAGMENT_LCD")] {
            let pipeline = PipelineOptions { shader_stage: stage, entry_point: entry.into(), multiview: None };
            let mut glsl = String::new();
            let reflection = Writer::new(&mut glsl, &module, &info, &options, &pipeline, Default::default()).and_then(|mut w| w.write()).map_err(|e| format!("{entry}: {e}"))?;
            blocks.extend(reflection.uniforms.into_values());
            for (uniform, mapping) in reflection.texture_mapping {
                let binding = module.global_variables[mapping.texture].binding.as_ref().ok_or_else(|| format!("{entry}: {uniform} has no binding"))?.binding;
                samplers.push((uniform, binding - 1));
            }
            writeln!(rust, "pub const {name}: &str = {glsl:?};")?;
        }
        blocks.sort();
        blocks.dedup();
        samplers.sort();
        samplers.dedup();
        writeln!(rust, "pub const UNIFORM_BLOCKS: &[&str] = &{blocks:?};")?;
        writeln!(rust, "pub const SAMPLERS: &[(&str, u32)] = &{samplers:?};")?;
        fs::write(PathBuf::from(env::var("OUT_DIR")?).join("ui_glsl.rs"), rust)?;
        Ok(())
    }
}

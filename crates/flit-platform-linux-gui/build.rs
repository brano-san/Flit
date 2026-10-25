fn main() {
    println!("cargo:rerun-if-changed=shaders/ui.wgsl");
    #[cfg(target_os = "linux")]
    glsl::generate();
}

#[cfg(target_os = "linux")]
mod glsl {
    use naga::back::glsl::{Options, PipelineOptions, Version, Writer, WriterFlags};
    use naga::valid::{Capabilities, ValidationFlags, Validator};
    use naga::ShaderStage;
    use std::fmt::Write;
    use std::{env, fs, path::PathBuf};

    pub fn generate() {
        let src = fs::read_to_string("shaders/ui.wgsl").unwrap();
        let module = naga::front::wgsl::parse_str(&src).unwrap_or_else(|e| panic!("{}", e.emit_to_string(&src)));
        let info = Validator::new(ValidationFlags::all(), Capabilities::empty())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&src)));
        let options = Options {
            version: Version::new_gles(300),
            writer_flags: WriterFlags::empty(),
            binding_map: Default::default(),
            zero_initialize_workgroup_memory: false,
        };
        let mut rust = String::new();
        let mut blocks = Vec::new();
        let mut samplers = Vec::new();
        for (stage, entry, name) in [(ShaderStage::Vertex, "vs_main", "VERTEX"), (ShaderStage::Fragment, "fs_main", "FRAGMENT")] {
            let pipeline = PipelineOptions { shader_stage: stage, entry_point: entry.into(), multiview: None };
            let mut glsl = String::new();
            let reflection = Writer::new(&mut glsl, &module, &info, &options, &pipeline, Default::default())
                .and_then(|mut w| w.write())
                .unwrap_or_else(|e| panic!("{entry}: {e}"));
            blocks.extend(reflection.uniforms.into_values());
            for (uniform, mapping) in reflection.texture_mapping {
                let binding = module.global_variables[mapping.texture].binding.as_ref().unwrap().binding;
                samplers.push((uniform, binding - 1));
            }
            writeln!(rust, "pub const {name}: &str = {glsl:?};").unwrap();
        }
        blocks.sort();
        samplers.sort();
        writeln!(rust, "pub const UNIFORM_BLOCKS: &[&str] = &{blocks:?};").unwrap();
        writeln!(rust, "pub const SAMPLERS: &[(&str, u32)] = &{samplers:?};").unwrap();
        fs::write(PathBuf::from(env::var("OUT_DIR").unwrap()).join("ui_glsl.rs"), rust).unwrap();
    }
}

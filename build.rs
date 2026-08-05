use std::env;
use std::fs;
use std::path::PathBuf;
use syntect::parsing::SyntaxSetBuilder;

fn main() {
    println!("cargo:rerun-if-changed=assets/syntaxes");
    println!("cargo:rerun-if-changed=assets/shaders/egui.wgsl");
    let mut builder = SyntaxSetBuilder::new();
    if let Err(error) = builder.add_from_folder("assets/syntaxes", true) {
        panic!("failed to compile built-in syntaxes: {error}");
    }
    let syntax_set = builder.build();
    let output =
        PathBuf::from(env::var_os("OUT_DIR").unwrap_or_default()).join("default_syntaxes.packdump");
    if let Err(error) = syntect::dumps::dump_to_file(&syntax_set, output) {
        panic!("failed to write built-in syntax dump: {error}");
    }

    let source = include_str!("assets/shaders/egui.wgsl");
    let module = naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|error| panic!("failed to parse Vulkan shader: {error}"));
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("failed to validate Vulkan shader: {error}"));
    let output_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap_or_default());
    for (name, stage) in [
        ("vertex", naga::ShaderStage::Vertex),
        ("fragment", naga::ShaderStage::Fragment),
    ] {
        let words = naga::back::spv::write_vec(
            &module,
            &info,
            &naga::back::spv::Options::default(),
            Some(&naga::back::spv::PipelineOptions {
                shader_stage: stage,
                entry_point: format!("{name}_main"),
            }),
        )
        .unwrap_or_else(|error| panic!("failed to compile Vulkan {name} shader: {error}"));
        let bytes: Vec<u8> = words.into_iter().flat_map(u32::to_le_bytes).collect();
        fs::write(output_dir.join(format!("egui_{name}.spv")), bytes)
            .unwrap_or_else(|error| panic!("failed to write Vulkan {name} shader: {error}"));
    }
}

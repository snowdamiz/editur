use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use syntect::parsing::SyntaxSetBuilder;

fn main() {
    println!("cargo:rerun-if-changed=assets/syntaxes");
    println!("cargo:rerun-if-changed=assets/shaders/egui.wgsl");
    println!("cargo:rerun-if-changed=assets/shaders/egui.metal");
    println!("cargo:rerun-if-env-changed=EDITUR_REQUIRE_PRECOMPILED_METAL");
    println!("cargo:rustc-check-cfg=cfg(editur_precompiled_metal)");
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

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        compile_metal_library(&output_dir);
    }
}

fn compile_metal_library(output_dir: &std::path::Path) {
    let air = output_dir.join("egui.air");
    let library = output_dir.join("egui.metallib");
    let compile = Command::new("xcrun")
        .args([
            "-sdk",
            "macosx",
            "metal",
            "-c",
            "assets/shaders/egui.metal",
            "-o",
        ])
        .arg(&air)
        .output();
    let Ok(compile) = compile else {
        metal_fallback("Apple Metal compiler is unavailable");
        return;
    };
    if !compile.status.success() {
        metal_fallback(&format!(
            "Apple Metal compiler is unavailable: {}",
            String::from_utf8_lossy(&compile.stderr).trim()
        ));
        return;
    }
    let link = Command::new("xcrun")
        .args(["-sdk", "macosx", "metallib"])
        .arg(&air)
        .arg("-o")
        .arg(&library)
        .output();
    let Ok(link) = link else {
        metal_fallback("Apple Metal linker is unavailable");
        return;
    };
    if link.status.success() {
        println!("cargo:rustc-cfg=editur_precompiled_metal");
    } else {
        metal_fallback(&format!(
            "Apple Metal linker is unavailable: {}",
            String::from_utf8_lossy(&link.stderr).trim()
        ));
    }
}

fn metal_fallback(message: &str) {
    if env::var("EDITUR_REQUIRE_PRECOMPILED_METAL").as_deref() == Ok("1") {
        panic!("{message}");
    }
    println!("cargo:warning={message}; using runtime shader fallback");
}

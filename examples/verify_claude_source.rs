use std::{env, fs, path::Path, path::PathBuf, process::Command};

const ADAPTER_COMMIT: &str = "6b405138fc82be947964612fac04e56654827b66";
const ADAPTER_VERSION: &str = "0.66.0";
const SDK_VERSION: &str = "0.3.220";
const SDK_INTEGRITY: &str = "sha512-glc7SdwPkOkLw8oxwLo9PKTdLJGqW/PIR4urWXFoRtX9YllwozsEVc5Tc1+EvLSkfrsxPJqQWqOgpjUOQXf1oA==";
const NATIVE_VERSION: &str = "2.1.220 (Claude Code)";
const NODE_VERSION: &str = "v22.22.0";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: verify_claude_source SOURCE")?;
    check_output(
        Command::new("git")
            .args(["-C"])
            .arg(&root)
            .args(["rev-parse", "HEAD"]),
        ADAPTER_COMMIT,
        "Claude adapter commit",
    )?;
    check_output(
        Command::new("node").arg("--version"),
        NODE_VERSION,
        "Node runtime",
    )?;

    let package = read_json(root.join("package.json"))?;
    check(
        package["version"] == ADAPTER_VERSION,
        "Claude adapter package version does not match the release pin",
    )?;
    let lock = read_json(root.join("package-lock.json"))?;
    check_lock(&lock, "@anthropic-ai/claude-agent-sdk", SDK_INTEGRITY)?;
    let (native, native_integrity) = native_package(env::consts::OS, env::consts::ARCH)
        .ok_or("unsupported Claude release target")?;
    check_lock(&lock, native, native_integrity)?;

    let sdk_root = root.join("node_modules/@anthropic-ai/claude-agent-sdk");
    check_package(&sdk_root, "@anthropic-ai/claude-agent-sdk")?;
    let native_root = root.join("node_modules").join(native);
    check_package(&native_root, native)?;
    for path in [
        root.join("dist/index.js"),
        root.join("README.md"),
        root.join("LICENSE"),
        sdk_root.join("README.md"),
        sdk_root.join("LICENSE.md"),
        native_root.join("README.md"),
        native_root.join("LICENSE.md"),
        native_root.join(if env::consts::OS == "windows" {
            "claude.exe"
        } else {
            "claude"
        }),
    ] {
        check(
            path.is_file(),
            &format!("required package file is missing: {}", path.display()),
        )?;
    }
    let installed_native = fs::read_dir(root.join("node_modules/@anthropic-ai"))?
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with("claude-agent-sdk-"))
        .collect::<Vec<_>>();
    check(
        installed_native == [native.trim_start_matches("@anthropic-ai/")],
        "production tree contains the wrong Claude native package set",
    )?;

    let entrypoint = root.join("dist/index.js");
    check_output(
        &mut sanitized_node(&entrypoint, ["--version"]),
        ADAPTER_VERSION,
        "Claude adapter version",
    )?;
    check_output(
        &mut sanitized_node(&entrypoint, ["--cli", "--version"]),
        NATIVE_VERSION,
        "Claude native binary version",
    )
}

fn read_json(path: PathBuf) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn check_lock(
    lock: &serde_json::Value,
    name: &str,
    integrity: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let package = &lock["packages"][format!("node_modules/{name}")];
    check(
        package["version"] == SDK_VERSION && package["integrity"] == integrity,
        &format!("{name} lock does not match the release pin"),
    )
}

fn check_package(root: &Path, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let package = read_json(root.join("package.json"))?;
    check(
        package["name"] == name && package["version"] == SDK_VERSION,
        &format!("installed {name} package does not match the release pin"),
    )
}

fn sanitized_node(entrypoint: &Path, args: impl IntoIterator<Item = &'static str>) -> Command {
    let mut command = Command::new("node");
    command.arg(entrypoint).args(args);
    for name in [
        "CLAUDE_CODE_EXECUTABLE",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "NODE_OPTIONS",
        "NODE_PATH",
    ] {
        command.env_remove(name);
    }
    command
}

fn check_output(
    command: &mut Command,
    expected: &str,
    name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let output = command.output()?;
    let reported = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    check(
        output.status.success() && reported.lines().any(|line| line.trim() == expected),
        &format!("{name} does not match the release pin"),
    )
}

fn check(condition: bool, message: &str) -> Result<(), Box<dyn std::error::Error>> {
    condition.then_some(()).ok_or_else(|| message.into())
}

fn native_package(os: &str, architecture: &str) -> Option<(&'static str, &'static str)> {
    match (os, architecture) {
        ("macos", "aarch64") => Some((
            "@anthropic-ai/claude-agent-sdk-darwin-arm64",
            "sha512-7VxlbEosK7DODiOnsjoVd0DSJzbnaPrM2jelMHI0y8zx1UnLS3WC6EFUXbvy74F2sXqEznh2tzn7EKWInaRN6Q==",
        )),
        ("macos", "x86_64") => Some((
            "@anthropic-ai/claude-agent-sdk-darwin-x64",
            "sha512-X9RwDsSmbF6ultKZroaip+DL8WRgC64gHbrAwrRlAFSPNZV7zmJyP2ur8rW7KrxqmtuehdMMkw8+SAC/6hD2PA==",
        )),
        ("linux", "x86_64") => Some((
            "@anthropic-ai/claude-agent-sdk-linux-x64",
            "sha512-tkTJFnpR9VifvWX2fmkCAPkT6+8Wk/gVu8B5jsVekKZPiZoWRHmMXO30BnZn+f0TZhgYP+82PSX3S8crH1kn+w==",
        )),
        ("windows", "x86_64") => Some((
            "@anthropic-ai/claude-agent-sdk-win32-x64",
            "sha512-MuOuXhbr66HlGaWXD2f3w0k2PsvmnbkwcUZ0dAe2poFLdl72GC2dapwwOBefxm9QmoNqk9+jmv/dSKGOVWyvLw==",
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::native_package;

    #[test]
    fn native_package_matrix_is_exactly_the_four_release_targets() {
        for target in [
            ("macos", "aarch64"),
            ("macos", "x86_64"),
            ("linux", "x86_64"),
            ("windows", "x86_64"),
        ] {
            assert!(native_package(target.0, target.1).is_some());
        }
        assert!(native_package("linux", "aarch64").is_none());
    }
}

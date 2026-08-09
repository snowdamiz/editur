use std::{env, fs, path::PathBuf, process::Command};

const ADAPTER_COMMIT: &str = "5faefec5d55ded33c54b68ffec93def4f6c547f5";
const ADAPTER_VERSION: &str = "1.1.14";
const CODEX_VERSION: &str = "0.147.0";
const CODEX_INTEGRITY: &str = "sha512-EQLEXecAG2ptxI7UpBMo2TR/ga5596/c/OsYF/0LoUDh5JANZ7IoGqlzBEWbuEVQ76JePIbtTW/ihCkp1a7Z3w==";
const NODE_VERSION: &str = "v22.22.0";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: verify_codex_source SOURCE")?;
    let commit = Command::new("git")
        .args([
            "-C",
            root.to_str().ok_or("source path is not UTF-8")?,
            "rev-parse",
            "HEAD",
        ])
        .output()?;
    check(
        commit.status.success(),
        "cannot inspect Codex adapter commit",
    )?;
    check(
        String::from_utf8(commit.stdout)?.trim() == ADAPTER_COMMIT,
        "Codex adapter commit does not match the release pin",
    )?;
    let node = Command::new("node").arg("--version").output()?;
    check(
        node.status.success(),
        "cannot inspect private Node runtime version",
    )?;
    check(
        String::from_utf8(node.stdout)?.trim() == NODE_VERSION,
        "Node runtime does not match the release pin",
    )?;

    let package: serde_json::Value = serde_json::from_slice(&fs::read(root.join("package.json"))?)?;
    check(
        package["version"] == ADAPTER_VERSION,
        "Codex adapter package version does not match the release pin",
    )?;
    let lock: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("package-lock.json"))?)?;
    let codex = &lock["packages"]["node_modules/@openai/codex"];
    check(
        codex["version"] == CODEX_VERSION && codex["integrity"] == CODEX_INTEGRITY,
        "Codex dependency lock does not match the release pin",
    )?;
    let installed: serde_json::Value = serde_json::from_slice(&fs::read(
        root.join("node_modules/@openai/codex/package.json"),
    )?)?;
    check(
        installed["version"] == CODEX_VERSION,
        "installed Codex dependency does not match the release pin",
    )?;
    check(
        root.join("dist/index.js").is_file(),
        "Codex adapter build output is missing",
    )?;
    Ok(())
}

fn check(condition: bool, message: &str) -> Result<(), Box<dyn std::error::Error>> {
    condition.then_some(()).ok_or_else(|| message.into())
}

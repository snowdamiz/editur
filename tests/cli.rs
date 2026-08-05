use std::process::Command;

#[test]
fn help_version_and_invalid_arguments_have_stable_exit_behavior() {
    let binary = env!("CARGO_BIN_EXE_editur");

    let help = Command::new(binary).arg("--help").output().unwrap();
    assert!(help.status.success());
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.contains("editur syntax install"));
    assert!(help.contains("editur update"));

    let version = Command::new(binary).arg("--version").output().unwrap();
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8_lossy(&version.stdout).trim(),
        concat!("editur ", env!("CARGO_PKG_VERSION"))
    );

    let invalid = Command::new(binary)
        .args(["syntax", "remove", "Not Valid"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).starts_with("editur: "));
}

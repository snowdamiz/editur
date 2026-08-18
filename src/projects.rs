//! The list of recently opened project roots, persisted in the application
//! data directory so every window (and the project switcher) shares it.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use serde::Deserialize;

const PROJECTS_FILE: &str = "projects.json";
/// A switcher longer than this stops being a shortcut, so older roots fall off.
pub const MAX_RECENT_PROJECTS: usize = 10;
/// Ten paths cannot legitimately need more than this; anything bigger is not
/// our file.
const MAX_PROJECTS_BYTES: u64 = 64 * 1024;
const MAX_GIT_METADATA_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GitWorkspaceStatus {
    pub branch: String,
    pub pull_request: Option<PullRequestStatus>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PullRequestStatus {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub is_draft: bool,
    #[serde(default)]
    pub review_decision: String,
    #[serde(default)]
    pub merge_state_status: String,
}

impl PullRequestStatus {
    pub(crate) fn label(&self) -> String {
        let status = if self.is_draft {
            "Draft"
        } else if self.state == "MERGED" {
            "Merged"
        } else if self.state == "CLOSED" {
            "Closed"
        } else if self.review_decision == "CHANGES_REQUESTED" {
            "Changes requested"
        } else if self.merge_state_status == "DIRTY" {
            "Conflicts"
        } else if self.review_decision == "APPROVED" {
            "Ready"
        } else {
            "Open"
        };
        format!("PR #{} · {status}", self.number)
    }
}

pub(crate) fn inspect_git_workspace(root: &Path) -> Option<GitWorkspaceStatus> {
    let mut git = Command::new("git");
    git.args(["-C"])
        .arg(root)
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"]);
    let output = command_output(&mut git)?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8(output.stdout)
        .ok()
        .map(|branch| branch.trim().to_owned())
        .filter(|branch| !branch.is_empty())?;
    let mut gh = Command::new("gh");
    gh.args([
        "pr",
        "view",
        "--json",
        "number,title,url,state,isDraft,reviewDecision,mergeStateStatus",
    ])
    .current_dir(root)
    .env("GH_PROMPT_DISABLED", "1");
    let pull_request = command_output(&mut gh)
        .filter(|output| output.status.success())
        .and_then(|output| parse_pull_request(&output.stdout));
    Some(GitWorkspaceStatus {
        branch,
        pull_request,
    })
}

fn command_output(command: &mut Command) -> Option<std::process::Output> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.output().ok()
}

fn parse_pull_request(bytes: &[u8]) -> Option<PullRequestStatus> {
    if bytes.len() > MAX_GIT_METADATA_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

/// Recently opened project roots, most recent first. Roots that no longer
/// exist as directories are skipped so the switcher never offers a dead
/// folder.
pub fn load(data_dir: &Path) -> Vec<PathBuf> {
    read_small_file(&data_dir.join(PROJECTS_FILE))
        .and_then(|bytes| serde_json::from_slice::<Vec<PathBuf>>(&bytes).ok())
        .map(|roots| {
            let mut roots: Vec<PathBuf> = roots.into_iter().filter(|root| root.is_dir()).collect();
            roots.dedup();
            roots.truncate(MAX_RECENT_PROJECTS);
            roots
        })
        .unwrap_or_default()
}

/// Records `root` as the most recently opened project.
pub fn remember(data_dir: &Path, root: &Path) -> Result<(), String> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut roots = load(data_dir);
    roots.retain(|known| known != &root);
    roots.insert(0, root);
    roots.truncate(MAX_RECENT_PROJECTS);
    let bytes = serde_json::to_vec(&roots)
        .map_err(|error| format!("cannot encode recent projects: {error}"))?;
    write_atomic(data_dir, PROJECTS_FILE, &bytes)
}

fn read_small_file(path: &Path) -> Option<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).ok()?;
    (metadata.is_file() && metadata.len() <= MAX_PROJECTS_BYTES)
        .then(|| fs::read(path).ok())
        .flatten()
}

fn write_atomic(data_dir: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    fs::create_dir_all(data_dir)
        .map_err(|error| format!("cannot create application data directory: {error}"))?;
    let mut staged = tempfile::NamedTempFile::new_in(data_dir)
        .map_err(|error| format!("cannot stage recent projects: {error}"))?;
    staged
        .write_all(bytes)
        .and_then(|()| staged.flush())
        .and_then(|()| staged.as_file().sync_all())
        .map_err(|error| format!("cannot write recent projects: {error}"))?;
    staged
        .persist(data_dir.join(name))
        .map_err(|error| format!("cannot save recent projects: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{MAX_RECENT_PROJECTS, inspect_git_workspace, load, parse_pull_request, remember};
    use std::{fs, process::Command};

    #[test]
    fn git_workspace_inspection_reports_the_branch_and_skips_plain_folders() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let plain = temp.path().join("plain");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&plain).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "--quiet", "--initial-branch", "agent-workspace"])
                .current_dir(&workspace)
                .status()
                .unwrap()
                .success()
        );

        assert_eq!(
            inspect_git_workspace(&workspace)
                .as_ref()
                .map(|workspace| workspace.branch.as_str()),
            Some("agent-workspace")
        );
        assert_eq!(inspect_git_workspace(&plain), None);
    }

    #[test]
    fn pull_request_status_uses_ghs_current_branch_json() {
        let pull_request = parse_pull_request(
            br#"{
                "number":42,
                "title":"Attach sessions to worktrees",
                "url":"https://github.com/editur/editur/pull/42",
                "state":"OPEN",
                "isDraft":false,
                "reviewDecision":"APPROVED",
                "mergeStateStatus":"CLEAN"
            }"#,
        )
        .unwrap();

        assert_eq!(pull_request.number, 42);
        assert_eq!(pull_request.label(), "PR #42 · Ready");
        assert_eq!(pull_request.url, "https://github.com/editur/editur/pull/42");
    }

    #[test]
    fn remember_orders_most_recent_first_and_dedups() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();

        remember(&data, &first).unwrap();
        remember(&data, &second).unwrap();
        remember(&data, &first).unwrap();

        let roots = load(&data);
        assert_eq!(
            roots,
            vec![
                first.canonicalize().unwrap(),
                second.canonicalize().unwrap()
            ]
        );
    }

    #[test]
    fn load_skips_roots_that_no_longer_exist() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let kept = temp.path().join("kept");
        let gone = temp.path().join("gone");
        fs::create_dir_all(&kept).unwrap();
        fs::create_dir_all(&gone).unwrap();

        remember(&data, &kept).unwrap();
        remember(&data, &gone).unwrap();
        fs::remove_dir_all(&gone).unwrap();

        assert_eq!(load(&data), vec![kept.canonicalize().unwrap()]);
    }

    #[test]
    fn remember_caps_the_list() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        for index in 0..MAX_RECENT_PROJECTS + 3 {
            let root = temp.path().join(format!("project-{index}"));
            fs::create_dir_all(&root).unwrap();
            remember(&data, &root).unwrap();
        }
        assert_eq!(load(&data).len(), MAX_RECENT_PROJECTS);
    }

    #[test]
    fn load_tolerates_a_corrupt_file() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().to_path_buf();
        fs::write(data.join("projects.json"), b"not json").unwrap();
        assert!(load(&data).is_empty());
    }
}

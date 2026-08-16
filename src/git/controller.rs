use std::{
    ffi::OsString,
    fs,
    io::{Read, Write as _},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread,
    time::Duration,
};

use super::status::{
    BranchInfo, ChangeKind, GitStatusSnapshot, RepoInfo, RepositoryStatus, parse_status,
};

const COMMAND_CAPACITY: usize = 64;
const EVENT_CAPACITY: usize = 64;
const DIFF_LIMIT: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum DiffArea {
    Staged,
    Worktree,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitCommand {
    Refresh,
    LoadDiff {
        repository: PathBuf,
        path: PathBuf,
        area: DiffArea,
    },
    Stage {
        repository: PathBuf,
        paths: Vec<PathBuf>,
    },
    Unstage {
        repository: PathBuf,
        paths: Vec<PathBuf>,
    },
    Discard {
        repository: PathBuf,
        paths: Vec<PathBuf>,
    },
    Commit {
        repository: PathBuf,
        message: String,
    },
    Init,
    Shutdown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitEvent {
    Status(GitStatusSnapshot),
    Diff {
        repository: PathBuf,
        path: PathBuf,
        area: DiffArea,
        old: Option<String>,
        new: String,
        generation: u64,
    },
    RefreshFailed(String),
    OperationFailed {
        op: &'static str,
        message: String,
    },
    Committed {
        repository: PathBuf,
        short_hash: String,
        subject: String,
    },
    WorktreeMutated {
        repository: PathBuf,
        paths: Vec<PathBuf>,
    },
    GitUnavailable,
}

pub struct GitController {
    commands: SyncSender<GitCommand>,
    events: Receiver<GitEvent>,
    worker: Option<thread::JoinHandle<()>>,
}

impl GitController {
    pub fn start(workspace_root: PathBuf, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let (command_tx, command_rx) = mpsc::sync_channel(COMMAND_CAPACITY);
        let (event_tx, event_rx) = mpsc::sync_channel(EVENT_CAPACITY);
        let wake = Arc::new(wake);
        let worker = thread::Builder::new()
            .name("editur-git".into())
            .spawn(move || Worker::new(workspace_root, event_tx, wake).run(command_rx))
            .expect("failed to start Editur Git controller thread");
        Self {
            commands: command_tx,
            events: event_rx,
            worker: Some(worker),
        }
    }

    pub fn send(&self, command: GitCommand) -> Result<(), String> {
        self.commands
            .try_send(command)
            .map_err(|_| "Git command could not be sent".to_owned())
    }

    pub fn events(&self) -> &Receiver<GitEvent> {
        &self.events
    }
}

impl Drop for GitController {
    fn drop(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        loop {
            match self.commands.try_send(GitCommand::Shutdown) {
                Ok(()) | Err(TrySendError::Disconnected(_)) => break,
                Err(TrySendError::Full(_)) => {
                    self.events.try_iter().for_each(drop);
                    if worker.is_finished() {
                        break;
                    }
                    thread::park_timeout(Duration::from_millis(1));
                }
            }
        }
        while !worker.is_finished() {
            self.events.try_iter().for_each(drop);
            thread::park_timeout(Duration::from_millis(1));
        }
        let _ = worker.join();
    }
}

struct Worker {
    workspace_root: PathBuf,
    events: SyncSender<GitEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
    generation: u64,
    repositories: Vec<RepositoryStatus>,
}

impl Worker {
    fn new(
        workspace_root: PathBuf,
        events: SyncSender<GitEvent>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            workspace_root,
            events,
            wake,
            generation: 0,
            repositories: Vec::new(),
        }
    }

    fn run(mut self, commands: Receiver<GitCommand>) {
        while let Ok(command) = commands.recv() {
            match command {
                GitCommand::Refresh => self.refresh(),
                GitCommand::Init => {
                    self.mutate("initialize", |worker| worker.initialize());
                }
                GitCommand::Stage { repository, paths } => {
                    self.mutate("stage", |worker| worker.stage(&repository, &paths));
                }
                GitCommand::Unstage { repository, paths } => {
                    self.mutate("unstage", |worker| worker.unstage(&repository, &paths));
                }
                GitCommand::Discard { repository, paths } => {
                    let event_paths = paths.clone();
                    let result = self.discard(&repository, &paths);
                    if result.is_ok() {
                        self.emit(GitEvent::WorktreeMutated {
                            repository,
                            paths: event_paths,
                        });
                    } else if let Err(message) = result {
                        self.emit(GitEvent::OperationFailed {
                            op: "discard",
                            message,
                        });
                    }
                    self.refresh();
                }
                GitCommand::Commit {
                    repository,
                    message,
                } => {
                    let result = self.commit(&repository, &message);
                    match result {
                        Ok((short_hash, subject)) => self.emit(GitEvent::Committed {
                            repository,
                            short_hash,
                            subject,
                        }),
                        Err(message) => self.emit(GitEvent::OperationFailed {
                            op: "commit",
                            message,
                        }),
                    }
                    self.refresh();
                }
                GitCommand::LoadDiff {
                    repository,
                    path,
                    area,
                } => match self.load_diff(&repository, &path, area) {
                    Ok((old, new)) => self.emit(GitEvent::Diff {
                        repository,
                        path,
                        area,
                        old,
                        new,
                        generation: self.generation,
                    }),
                    Err(message) => self.emit(GitEvent::OperationFailed {
                        op: "diff",
                        message,
                    }),
                },
                GitCommand::Shutdown => return,
            }
        }
    }

    fn mutate(&mut self, op: &'static str, mutation: impl FnOnce(&mut Self) -> Result<(), String>) {
        if let Err(message) = mutation(self) {
            self.emit(GitEvent::OperationFailed { op, message });
        }
        self.refresh();
    }

    fn refresh(&mut self) {
        if Command::new("git")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
        {
            self.emit(GitEvent::GitUnavailable);
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        let result = discover_repositories(&self.workspace_root).and_then(|roots| {
            roots
                .into_iter()
                .map(|root| repository_status(&root))
                .collect::<Result<Vec<_>, _>>()
        });
        match result {
            Ok(repositories) => {
                self.repositories.clone_from(&repositories);
                self.emit(GitEvent::Status(GitStatusSnapshot {
                    generation: self.generation,
                    repositories,
                }));
            }
            Err(message) => self.emit(GitEvent::RefreshFailed(message)),
        }
    }

    fn initialize(&self) -> Result<(), String> {
        run_git(
            &self.workspace_root,
            &[OsString::from("init")],
            None,
            64 * 1024,
        )
        .map(drop)
    }

    fn stage(&self, repository: &Path, paths: &[PathBuf]) -> Result<(), String> {
        self.ensure_repository(repository)?;
        let mut args = vec![OsString::from("add"), OsString::from("--")];
        args.extend(paths.iter().map(|path| path.as_os_str().to_owned()));
        run_git(repository, &args, None, 64 * 1024).map(drop)
    }

    fn unstage(&self, repository: &Path, paths: &[PathBuf]) -> Result<(), String> {
        let snapshot = self.ensure_repository(repository)?;
        let unborn = matches!(snapshot.info.branch, BranchInfo::Named { unborn: true, .. });
        let mut args = if unborn {
            vec![
                OsString::from("rm"),
                OsString::from("--cached"),
                OsString::from("-r"),
                OsString::from("--"),
            ]
        } else {
            vec![
                OsString::from("restore"),
                OsString::from("--staged"),
                OsString::from("--"),
            ]
        };
        args.extend(paths.iter().map(|path| path.as_os_str().to_owned()));
        run_git(repository, &args, None, 64 * 1024).map(drop)
    }

    fn discard(&self, repository: &Path, paths: &[PathBuf]) -> Result<(), String> {
        let snapshot = self.ensure_repository(repository)?;
        let mut tracked = Vec::new();
        let mut untracked = Vec::new();
        for path in paths {
            let entry = snapshot
                .entries
                .iter()
                .find(|entry| &entry.path == path)
                .ok_or_else(|| format!("{} is no longer changed", path.display()))?;
            if entry.worktree == Some(ChangeKind::Untracked) {
                let target = repository.join(path);
                let metadata = fs::symlink_metadata(&target)
                    .map_err(|error| format!("cannot inspect {}: {error}", target.display()))?;
                if metadata.is_dir() && !metadata.file_type().is_symlink() {
                    return Err(format!(
                        "refusing to recursively delete {}",
                        target.display()
                    ));
                }
                untracked.push(target);
            } else {
                tracked.push(path);
            }
        }
        for target in untracked {
            fs::remove_file(&target)
                .map_err(|error| format!("cannot delete {}: {error}", target.display()))?;
        }
        if tracked.is_empty() {
            return Ok(());
        }
        let mut args = vec![
            OsString::from("restore"),
            OsString::from("--worktree"),
            OsString::from("--"),
        ];
        args.extend(tracked.into_iter().map(|path| path.as_os_str().to_owned()));
        run_git(repository, &args, None, 64 * 1024).map(drop)
    }

    fn commit(&self, repository: &Path, message: &str) -> Result<(String, String), String> {
        self.ensure_repository(repository)?;
        if message.trim().is_empty() {
            return Err("enter a commit message".into());
        }
        run_git(
            repository,
            &[OsString::from("commit"), OsString::from("--file=-")],
            Some(message.as_bytes()),
            64 * 1024,
        )?;
        last_commit(repository)?.ok_or_else(|| "commit succeeded without a new commit".to_owned())
    }

    fn load_diff(
        &self,
        repository: &Path,
        path: &Path,
        area: DiffArea,
    ) -> Result<(Option<String>, String), String> {
        let snapshot = self.ensure_repository(repository)?;
        let entry = snapshot
            .entries
            .iter()
            .find(|entry| entry.path == path)
            .ok_or_else(|| format!("{} is no longer changed", path.display()))?;
        let (old, new) = match area {
            DiffArea::Worktree => {
                let old = if entry.worktree == Some(ChangeKind::Untracked) {
                    None
                } else {
                    let old_path = entry
                        .orig_path
                        .as_deref()
                        .filter(|_| entry.worktree == Some(ChangeKind::Renamed))
                        .unwrap_or(path);
                    Some(git_show(repository, ":0:", old_path)?.unwrap_or_default())
                };
                let new = if entry.worktree == Some(ChangeKind::Deleted) {
                    String::new()
                } else {
                    read_diff_file(&repository.join(path))?
                };
                (old, new)
            }
            DiffArea::Staged => {
                let old = if entry.index == Some(ChangeKind::Added) {
                    None
                } else {
                    let old_path = entry
                        .orig_path
                        .as_deref()
                        .filter(|_| entry.index == Some(ChangeKind::Renamed))
                        .unwrap_or(path);
                    git_show(repository, "HEAD:", old_path)?
                };
                let new = if entry.index == Some(ChangeKind::Deleted) {
                    String::new()
                } else {
                    git_show(repository, ":0:", path)?.unwrap_or_default()
                };
                (old, new)
            }
        };
        if old.as_deref().is_some_and(is_diff_placeholder) || is_diff_placeholder(&new) {
            let placeholder = if old.as_deref() == Some("File too large to diff")
                || new == "File too large to diff"
            {
                "File too large to diff"
            } else {
                "Binary file"
            };
            return Ok((None, placeholder.into()));
        }
        Ok((old, new))
    }

    fn ensure_repository(&self, root: &Path) -> Result<&RepositoryStatus, String> {
        self.repositories
            .iter()
            .find(|repository| repository.root == root)
            .ok_or_else(|| format!("{} is not an active repository", root.display()))
    }

    fn emit(&self, event: GitEvent) {
        if self.events.send(event).is_ok() {
            (self.wake)();
        }
    }
}

fn git_show(repository: &Path, prefix: &str, path: &Path) -> Result<Option<String>, String> {
    let mut spec = OsString::from(prefix);
    spec.push(path.as_os_str());
    match run_git(
        repository,
        &[
            OsString::from("show"),
            OsString::from("--no-textconv"),
            spec,
        ],
        None,
        DIFF_LIMIT,
    ) {
        Ok(bytes) => Ok(Some(diff_text(bytes))),
        Err(message) if message == "git output was too large" => {
            Ok(Some("File too large to diff".into()))
        }
        Err(message) => Err(message),
    }
}

fn read_diff_file(path: &Path) -> Result<String, String> {
    let file =
        fs::File::open(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let (bytes, overflow) = read_bounded(file, DIFF_LIMIT)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if overflow {
        Ok("File too large to diff".into())
    } else {
        Ok(diff_text(bytes))
    }
}

fn diff_text(bytes: Vec<u8>) -> String {
    if bytes.contains(&0) {
        "Binary file".into()
    } else {
        String::from_utf8(bytes).unwrap_or_else(|_| "Binary file".into())
    }
}

fn is_diff_placeholder(text: &str) -> bool {
    matches!(text, "Binary file" | "File too large to diff")
}

fn repository_status(root: &Path) -> Result<RepositoryStatus, String> {
    let output = run_git(
        root,
        &[
            OsString::from("status"),
            OsString::from("--porcelain=v2"),
            OsString::from("--branch"),
            OsString::from("-z"),
            OsString::from("--untracked-files=all"),
        ],
        None,
        64 * 1024,
    )?;
    let parsed = parse_status(&output)?;
    let last_commit_subject = if matches!(parsed.branch, BranchInfo::Named { unborn: true, .. }) {
        None
    } else {
        last_commit(root)?.map(|(hash, subject)| format!("{hash} {subject}"))
    };
    Ok(RepositoryStatus {
        root: root.to_path_buf(),
        info: RepoInfo {
            branch: parsed.branch,
            last_commit_subject,
        },
        entries: parsed.entries,
    })
}

fn last_commit(root: &Path) -> Result<Option<(String, String)>, String> {
    let output = run_git(
        root,
        &[
            OsString::from("log"),
            OsString::from("-1"),
            OsString::from("--format=%h%x00%s"),
        ],
        None,
        64 * 1024,
    )?;
    if output.is_empty() {
        return Ok(None);
    }
    let mut fields = output.splitn(2, |byte| *byte == 0);
    let hash = String::from_utf8_lossy(fields.next().unwrap_or_default()).into_owned();
    let subject = String::from_utf8_lossy(fields.next().unwrap_or_default())
        .trim()
        .to_owned();
    Ok(Some((hash, subject)))
}

fn run_git(
    root: &Path,
    args: &[OsString],
    input: Option<&[u8]>,
    stdout_limit: usize,
) -> Result<Vec<u8>, String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run git: {error}"))?;
    if let Some(input) = input {
        child
            .stdin
            .take()
            .ok_or_else(|| "git stdin was not available".to_owned())?
            .write_all(input)
            .map_err(|error| format!("cannot write to git: {error}"))?;
    }
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "git stdout was not available".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "git stderr was not available".to_owned())?;
    let stdout = thread::spawn(move || read_bounded(stdout, stdout_limit));
    let stderr = thread::spawn(move || read_bounded(stderr, 64 * 1024));
    let status = child
        .wait()
        .map_err(|error| format!("cannot wait for git: {error}"))?;
    let (stdout, stdout_overflow) = stdout
        .join()
        .map_err(|_| "git stdout reader stopped".to_owned())?
        .map_err(|error| format!("cannot read git stdout: {error}"))?;
    let (stderr, _) = stderr
        .join()
        .map_err(|_| "git stderr reader stopped".to_owned())?
        .map_err(|error| format!("cannot read git stderr: {error}"))?;
    if stdout_overflow {
        return Err("git output was too large".into());
    }
    if !status.success() {
        let message = String::from_utf8_lossy(&stderr)
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("git command failed")
            .trim()
            .to_owned();
        return Err(message);
    }
    Ok(stdout)
}

fn read_bounded(mut reader: impl Read, limit: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut output = Vec::with_capacity(limit.min(64 * 1024));
    let mut overflow = false;
    let mut buffer = [0; 8192];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..read.min(remaining)]);
        overflow |= read > remaining;
    }
    Ok((output, overflow))
}

pub fn discover_repositories(workspace_root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut repositories = Vec::new();
    if !git_marker(workspace_root)
        && let Some(parent) = git_toplevel(workspace_root)
    {
        repositories.push(parent);
    }
    scan_for_repositories(workspace_root, &mut repositories)?;
    repositories.sort();
    repositories.dedup();
    Ok(repositories)
}

fn scan_for_repositories(directory: &Path, repositories: &mut Vec<PathBuf>) -> Result<(), String> {
    if git_marker(directory) {
        repositories.push(directory.to_path_buf());
    }
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("cannot inspect {}: {error}", directory.display()))?;
    let mut children = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            (file_type.is_dir() && !file_type.is_symlink()).then_some(entry.path())
        })
        .filter(|path| {
            let name = path.file_name().and_then(|name| name.to_str());
            // ponytail: generated dependency/build trees are skipped; use a
            // gitignore-aware walker if repositories inside them become real input.
            !matches!(name, Some(".git" | "node_modules" | "target"))
        })
        .collect::<Vec<_>>();
    children.sort();
    for child in children {
        let _ = scan_for_repositories(&child, repositories);
    }
    Ok(())
}

fn git_marker(directory: &Path) -> bool {
    fs::symlink_metadata(directory.join(".git"))
        .is_ok_and(|metadata| metadata.is_dir() || metadata.is_file())
}

fn git_toplevel(directory: &Path) -> Option<PathBuf> {
    let output = run_git(
        directory,
        &[
            OsString::from("rev-parse"),
            OsString::from("--show-toplevel"),
        ],
        None,
        64 * 1024,
    )
    .ok()?;
    let path = String::from_utf8(output).ok()?;
    Some(PathBuf::from(path.trim()))
}

#[cfg(test)]
mod tests {
    use std::{fs, process::Command};

    use super::discover_repositories;

    fn init(path: &std::path::Path) {
        fs::create_dir_all(path).expect("fixture directory");
        let status = Command::new("git")
            .args(["init", "--quiet"])
            .arg(path)
            .status()
            .expect("git should be installed for controller tests");
        assert!(status.success(), "git init failed");
    }

    #[test]
    fn discovers_multiple_repositories_below_a_non_repository_workspace() {
        let fixture = tempfile::tempdir().expect("temporary workspace");
        let first = fixture.path().join("apps/alpha");
        let second = fixture.path().join("packages/deep/beta");
        init(&first);
        init(&second);

        assert_eq!(
            discover_repositories(fixture.path()).expect("repository discovery"),
            vec![first, second]
        );
    }

    #[test]
    fn discovers_a_repository_nested_below_the_workspace_repository() {
        let fixture = tempfile::tempdir().expect("temporary workspace");
        let root = fixture.path().to_path_buf();
        let nested = root.join("examples/deep/repository");
        init(&root);
        init(&nested);

        assert_eq!(
            discover_repositories(&root).expect("repository discovery"),
            vec![root, nested]
        );
    }
}

use std::{
    fs,
    path::Path,
    process::Command,
    sync::mpsc::Receiver,
    time::{Duration, Instant},
};

use editur::git::{
    controller::{DiffArea, GitCommand, GitController, GitEvent},
    status::{ChangeKind, GitStatusSnapshot},
};

fn status(events: &Receiver<GitEvent>) -> GitStatusSnapshot {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match events
            .recv_timeout(remaining)
            .expect("controller should emit a status snapshot")
        {
            GitEvent::Status(snapshot) => return snapshot,
            GitEvent::OperationFailed { op, message } => panic!("{op} failed: {message}"),
            _ => {}
        }
    }
}

fn git(repository: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .output()
        .expect("git command should start");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn controller_initializes_stages_commits_and_returns_to_clean() {
    let fixture = tempfile::tempdir().expect("temporary workspace");
    let root = fixture.path().to_path_buf();
    let controller = GitController::start(root.clone(), || {});

    controller
        .send(GitCommand::Init)
        .expect("initialize command");
    let initialized = status(controller.events());
    assert_eq!(initialized.repositories.len(), 1);
    git(&root, &["config", "user.name", "Editur Test"]);
    git(&root, &["config", "user.email", "editur@example.invalid"]);

    fs::write(root.join("new.txt"), "new\n").expect("fixture file");
    controller
        .send(GitCommand::Refresh)
        .expect("refresh command");
    let untracked = status(controller.events());
    assert_eq!(
        untracked.repositories[0].entries[0].worktree,
        Some(ChangeKind::Untracked)
    );

    controller
        .send(GitCommand::Stage {
            repository: root.clone(),
            paths: vec!["new.txt".into()],
        })
        .expect("stage command");
    let staged = status(controller.events());
    assert_eq!(
        staged.repositories[0].entries[0].index,
        Some(ChangeKind::Added)
    );

    controller
        .send(GitCommand::Commit {
            repository: root,
            message: "Add fixture".into(),
        })
        .expect("commit command");
    let clean = status(controller.events());
    assert!(clean.repositories[0].entries.is_empty());
}

#[test]
fn controller_loads_unstaged_diff_from_index_and_worktree() {
    let fixture = tempfile::tempdir().expect("temporary workspace");
    let root = fixture.path().to_path_buf();
    git(&root, &["init", "--quiet"]);
    git(&root, &["config", "user.name", "Editur Test"]);
    git(&root, &["config", "user.email", "editur@example.invalid"]);
    fs::write(root.join("file.txt"), "base\n").expect("fixture file");
    git(&root, &["add", "file.txt"]);
    git(&root, &["commit", "--quiet", "-m", "base"]);
    fs::write(root.join("file.txt"), "changed\n").expect("modified fixture file");

    let controller = GitController::start(root.clone(), || {});
    controller
        .send(GitCommand::Refresh)
        .expect("refresh command");
    let snapshot = status(controller.events());
    controller
        .send(GitCommand::LoadDiff {
            repository: root.clone(),
            path: "file.txt".into(),
            area: DiffArea::Worktree,
        })
        .expect("diff command");

    let event = controller
        .events()
        .recv_timeout(Duration::from_secs(5))
        .expect("diff event");
    assert_eq!(
        event,
        GitEvent::Diff {
            repository: root,
            path: "file.txt".into(),
            area: DiffArea::Worktree,
            old: Some("base\n".into()),
            new: "changed\n".into(),
            generation: snapshot.generation,
        }
    );
}

#[test]
fn controller_unstages_an_unborn_index_then_discards_the_untracked_file() {
    let fixture = tempfile::tempdir().expect("temporary workspace");
    let root = fixture.path().to_path_buf();
    git(&root, &["init", "--quiet"]);
    fs::write(root.join("new.txt"), "new\n").expect("fixture file");
    let controller = GitController::start(root.clone(), || {});
    controller
        .send(GitCommand::Refresh)
        .expect("refresh command");
    status(controller.events());

    controller
        .send(GitCommand::Stage {
            repository: root.clone(),
            paths: vec!["new.txt".into()],
        })
        .expect("stage command");
    status(controller.events());
    controller
        .send(GitCommand::Unstage {
            repository: root.clone(),
            paths: vec!["new.txt".into()],
        })
        .expect("unstage command");
    let unstaged = status(controller.events());
    assert_eq!(
        unstaged.repositories[0].entries[0].worktree,
        Some(ChangeKind::Untracked)
    );

    controller
        .send(GitCommand::Discard {
            repository: root,
            paths: vec!["new.txt".into()],
        })
        .expect("discard command");
    let clean = status(controller.events());
    assert!(clean.repositories[0].entries.is_empty());
    assert!(!fixture.path().join("new.txt").exists());
}

#[test]
fn mutation_snapshot_precedes_a_later_diff_result() {
    let fixture = tempfile::tempdir().expect("temporary workspace");
    let root = fixture.path().to_path_buf();
    git(&root, &["init", "--quiet"]);
    fs::write(root.join("new.txt"), "new\n").expect("fixture file");
    let controller = GitController::start(root.clone(), || {});
    controller
        .send(GitCommand::Refresh)
        .expect("refresh command");
    status(controller.events());

    controller
        .send(GitCommand::Stage {
            repository: root.clone(),
            paths: vec!["new.txt".into()],
        })
        .expect("stage command");
    controller
        .send(GitCommand::LoadDiff {
            repository: root,
            path: "new.txt".into(),
            area: DiffArea::Staged,
        })
        .expect("diff command");

    assert!(matches!(
        controller
            .events()
            .recv_timeout(Duration::from_secs(5))
            .expect("fresh snapshot"),
        GitEvent::Status(_)
    ));
    assert!(matches!(
        controller
            .events()
            .recv_timeout(Duration::from_secs(5))
            .expect("later diff"),
        GitEvent::Diff {
            area: DiffArea::Staged,
            old: None,
            ..
        }
    ));
}

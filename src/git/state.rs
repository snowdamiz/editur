use std::{collections::HashSet, path::PathBuf};

use super::{
    controller::GitEvent,
    status::{GitStatusSnapshot, RepositoryStatus},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum GitAvailability {
    #[default]
    Loading,
    Ready,
    Unavailable,
}

#[derive(Default)]
pub struct GitState {
    pub availability: GitAvailability,
    pub snapshot: Option<GitStatusSnapshot>,
    pub selected_repository: Option<PathBuf>,
    pub commit_message: String,
    pub pending_paths: HashSet<(PathBuf, PathBuf)>,
    pub pending_operation: Option<&'static str>,
    pub last_refresh_error: Option<String>,
    pub focus_index: Option<usize>,
    pub focus_commit: bool,
}

impl GitState {
    pub fn repository(&self) -> Option<&RepositoryStatus> {
        let selected = self.selected_repository.as_ref()?;
        self.snapshot
            .as_ref()?
            .repositories
            .iter()
            .find(|repository| &repository.root == selected)
    }

    pub fn begin(
        &mut self,
        operation: &'static str,
        repository: &std::path::Path,
        paths: &[PathBuf],
    ) {
        self.pending_operation = Some(operation);
        self.pending_paths.extend(
            paths
                .iter()
                .map(|path| (repository.to_path_buf(), path.clone())),
        );
    }

    pub fn apply(&mut self, event: &GitEvent) {
        match event {
            GitEvent::Status(snapshot) => {
                if self
                    .snapshot
                    .as_ref()
                    .is_some_and(|current| snapshot.generation < current.generation)
                {
                    return;
                }
                if !self.selected_repository.as_ref().is_some_and(|selected| {
                    snapshot
                        .repositories
                        .iter()
                        .any(|repository| &repository.root == selected)
                }) {
                    self.selected_repository = snapshot
                        .repositories
                        .first()
                        .map(|repository| repository.root.clone());
                }
                self.snapshot = Some(snapshot.clone());
                self.availability = GitAvailability::Ready;
                self.pending_paths.clear();
                self.pending_operation = None;
                self.last_refresh_error = None;
                let last_index = snapshot
                    .repositories
                    .iter()
                    .map(|repository| repository.entries.len())
                    .max()
                    .and_then(|len| len.checked_sub(1));
                self.focus_index = self
                    .focus_index
                    .zip(last_index)
                    .map(|(index, last)| index.min(last));
            }
            GitEvent::RefreshFailed(message) => {
                self.last_refresh_error = Some(message.clone());
            }
            GitEvent::OperationFailed { .. } => {
                self.pending_paths.clear();
                self.pending_operation = None;
            }
            GitEvent::Committed { .. } => {
                self.commit_message.clear();
                self.pending_paths.clear();
                self.pending_operation = None;
            }
            GitEvent::GitUnavailable => {
                self.availability = GitAvailability::Unavailable;
                self.snapshot = None;
                self.pending_paths.clear();
                self.pending_operation = None;
            }
            GitEvent::Diff { .. } | GitEvent::WorktreeMutated { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GitAvailability, GitState};
    use crate::git::{
        controller::GitEvent,
        status::{BranchInfo, ChangeKind, GitEntry, GitStatusSnapshot, RepoInfo, RepositoryStatus},
    };

    fn snapshot(generation: u64, root: &str) -> GitStatusSnapshot {
        GitStatusSnapshot {
            generation,
            repositories: vec![RepositoryStatus {
                root: root.into(),
                info: RepoInfo {
                    branch: BranchInfo::Named {
                        name: "main".into(),
                        upstream: None,
                        ahead: 0,
                        behind: 0,
                        unborn: false,
                    },
                    last_commit_subject: None,
                },
                entries: Vec::new(),
            }],
        }
    }

    #[test]
    fn reducer_rejects_stale_status_and_finishes_pending_on_a_new_snapshot() {
        let mut state = GitState::default();
        state.apply(&GitEvent::Status(snapshot(2, "/new")));
        state.begin("stage", std::path::Path::new("/new"), &["file.rs".into()]);

        state.apply(&GitEvent::Status(snapshot(1, "/old")));
        assert_eq!(
            state.snapshot.as_ref().map(|value| value.generation),
            Some(2)
        );
        assert!(!state.pending_paths.is_empty());

        state.apply(&GitEvent::Status(snapshot(3, "/new")));
        assert_eq!(
            state.snapshot.as_ref().map(|value| value.generation),
            Some(3)
        );
        assert!(state.pending_paths.is_empty());
        assert_eq!(state.availability, GitAvailability::Ready);
    }

    #[test]
    fn status_does_not_focus_a_change_until_the_user_selects_one() {
        let mut state = GitState::default();
        let mut status = snapshot(1, "/repo");
        status.repositories[0].entries.push(GitEntry {
            path: "file.rs".into(),
            orig_path: None,
            index: None,
            worktree: Some(ChangeKind::Modified),
        });

        state.apply(&GitEvent::Status(status));

        assert_eq!(state.focus_index, None);
    }
}

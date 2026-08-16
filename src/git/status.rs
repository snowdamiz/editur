use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChangeKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
    TypeChange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitEntry {
    pub path: PathBuf,
    pub orig_path: Option<PathBuf>,
    pub index: Option<ChangeKind>,
    pub worktree: Option<ChangeKind>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BranchInfo {
    Named {
        name: String,
        upstream: Option<String>,
        ahead: u64,
        behind: u64,
        unborn: bool,
    },
    Detached {
        oid: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedStatus {
    pub branch: BranchInfo,
    pub entries: Vec<GitEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoInfo {
    pub branch: BranchInfo,
    pub last_commit_subject: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryStatus {
    pub root: PathBuf,
    pub info: RepoInfo,
    pub entries: Vec<GitEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitStatusSnapshot {
    pub generation: u64,
    pub repositories: Vec<RepositoryStatus>,
}

pub fn parse_status(bytes: &[u8]) -> Result<ParsedStatus, String> {
    let mut head = None;
    let mut oid = None;
    let mut upstream = None;
    let mut ahead = 0;
    let mut behind = 0;
    let mut records = Vec::new();
    let mut start = 0;

    for end in bytes
        .iter()
        .enumerate()
        .filter_map(|(index, byte)| (*byte == b'\n' || *byte == 0).then_some(index))
    {
        let record = &bytes[start..end];
        start = end + 1;
        if record.is_empty() {
            continue;
        }
        if let Some(value) = record.strip_prefix(b"# branch.oid ") {
            oid = Some(text(value));
        } else if let Some(value) = record.strip_prefix(b"# branch.head ") {
            head = Some(text(value));
        } else if let Some(value) = record.strip_prefix(b"# branch.upstream ") {
            upstream = Some(text(value));
        } else if let Some(value) = record.strip_prefix(b"# branch.ab ") {
            let value = text(value);
            let mut counts = value.split_whitespace();
            ahead = parse_count(counts.next(), '+')?;
            behind = parse_count(counts.next(), '-')?;
        } else {
            records.push(record);
        }
    }
    if start < bytes.len() {
        records.push(&bytes[start..]);
    }

    let head = head.ok_or_else(|| "git status did not report a branch".to_owned())?;
    let unborn = oid.as_deref() == Some("(initial)") || head == "(initial)";
    let branch = if head == "(detached)" {
        let oid = oid.ok_or_else(|| "detached status did not report an oid".to_owned())?;
        BranchInfo::Detached {
            oid: oid.chars().take(7).collect(),
        }
    } else {
        BranchInfo::Named {
            name: head,
            upstream,
            ahead,
            behind,
            unborn,
        }
    };
    let mut entries = Vec::new();
    let mut records = records.into_iter();
    while let Some(record) = records.next() {
        if record.starts_with(b"2 ") {
            let original = records
                .next()
                .ok_or_else(|| "rename status omitted the original path".to_owned())?;
            entries.push(parse_rename(record, original)?);
        } else if let Some(path) = record.strip_prefix(b"? ") {
            entries.push(GitEntry {
                path: text(path).into(),
                orig_path: None,
                index: None,
                worktree: Some(ChangeKind::Untracked),
            });
        } else if record.starts_with(b"u ") {
            entries.push(parse_unmerged(record)?);
        } else {
            entries.push(parse_ordinary(record)?);
        }
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(ParsedStatus { branch, entries })
}

fn parse_unmerged(record: &[u8]) -> Result<GitEntry, String> {
    let record = text(record);
    let mut fields = record.splitn(11, ' ');
    if fields.next() != Some("u") || fields.next().is_none() {
        return Err(format!("invalid unmerged record: {record}"));
    }
    for _ in 0..8 {
        fields
            .next()
            .ok_or_else(|| format!("invalid unmerged record: {record}"))?;
    }
    let path = fields
        .next()
        .ok_or_else(|| format!("invalid unmerged record: {record}"))?;
    Ok(GitEntry {
        path: path.into(),
        orig_path: None,
        index: Some(ChangeKind::Conflicted),
        worktree: Some(ChangeKind::Conflicted),
    })
}

fn parse_rename(record: &[u8], original: &[u8]) -> Result<GitEntry, String> {
    let record = text(record);
    let mut fields = record.splitn(10, ' ');
    if fields.next() != Some("2") {
        return Err(format!("invalid rename record: {record}"));
    }
    let xy = fields
        .next()
        .filter(|value| value.len() == 2)
        .ok_or_else(|| format!("invalid rename record: {record}"))?;
    for _ in 0..7 {
        fields
            .next()
            .ok_or_else(|| format!("invalid rename record: {record}"))?;
    }
    let path = fields
        .next()
        .ok_or_else(|| format!("invalid rename record: {record}"))?;
    let mut status = xy.chars();
    Ok(GitEntry {
        path: path.into(),
        orig_path: Some(text(original).into()),
        index: parse_change(status.next()),
        worktree: parse_change(status.next()),
    })
}

fn parse_ordinary(record: &[u8]) -> Result<GitEntry, String> {
    let record = text(record);
    let mut fields = record.splitn(9, ' ');
    if fields.next() != Some("1") {
        return Err(format!("unsupported porcelain v2 record: {record}"));
    }
    let xy = fields
        .next()
        .filter(|value| value.len() == 2)
        .ok_or_else(|| format!("invalid porcelain v2 record: {record}"))?;
    for _ in 0..6 {
        fields
            .next()
            .ok_or_else(|| format!("invalid porcelain v2 record: {record}"))?;
    }
    let path = fields
        .next()
        .ok_or_else(|| format!("invalid porcelain v2 record: {record}"))?;
    let mut status = xy.chars();
    Ok(GitEntry {
        path: path.into(),
        orig_path: None,
        index: parse_change(status.next()),
        worktree: parse_change(status.next()),
    })
}

fn parse_change(value: Option<char>) -> Option<ChangeKind> {
    match value {
        Some('M') => Some(ChangeKind::Modified),
        Some('A') => Some(ChangeKind::Added),
        Some('D') => Some(ChangeKind::Deleted),
        Some('R') => Some(ChangeKind::Renamed),
        Some('T') => Some(ChangeKind::TypeChange),
        Some('U') => Some(ChangeKind::Conflicted),
        _ => None,
    }
}

fn parse_count(value: Option<&str>, sign: char) -> Result<u64, String> {
    value
        .and_then(|value| value.strip_prefix(sign))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| "invalid branch ahead/behind counts".to_owned())
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::{BranchInfo, ChangeKind, GitEntry, ParsedStatus, parse_status};

    #[test]
    fn parses_named_branch_and_modified_worktree_entry() {
        let input = b"# branch.oid 0123456789abcdef\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -1\n1 .M N... 100644 100644 100644 0123456 0123456 src/main.rs\0";

        assert_eq!(
            parse_status(input),
            Ok(ParsedStatus {
                branch: BranchInfo::Named {
                    name: "main".into(),
                    upstream: Some("origin/main".into()),
                    ahead: 2,
                    behind: 1,
                    unborn: false,
                },
                entries: vec![GitEntry {
                    path: "src/main.rs".into(),
                    orig_path: None,
                    index: None,
                    worktree: Some(ChangeKind::Modified),
                }],
            })
        );
    }

    #[test]
    fn parses_nul_separated_rename_paths_verbatim() {
        let input = b"# branch.oid 0123456789abcdef\n# branch.head main\n2 R. N... 100644 100644 100644 0123456 789abcd R100 src/new name-\xc3\xa9.rs\0src/old name-\xc3\xa9.rs\0";

        let parsed = parse_status(input).expect("rename status should parse");

        assert_eq!(
            parsed.entries,
            vec![GitEntry {
                path: "src/new name-\u{e9}.rs".into(),
                orig_path: Some("src/old name-\u{e9}.rs".into()),
                index: Some(ChangeKind::Renamed),
                worktree: None,
            }]
        );
    }

    #[test]
    fn parses_untracked_path_with_spaces() {
        let input = b"# branch.oid 0123456789abcdef\n# branch.head main\n? notes/new file.md\0";

        let parsed = parse_status(input).expect("untracked status should parse");

        assert_eq!(
            parsed.entries,
            vec![GitEntry {
                path: "notes/new file.md".into(),
                orig_path: None,
                index: None,
                worktree: Some(ChangeKind::Untracked),
            }]
        );
    }

    #[test]
    fn parses_unmerged_entry_as_one_conflict() {
        let input = b"# branch.oid 0123456789abcdef\n# branch.head main\nu UU N... 100644 100644 100644 100644 1111111 2222222 3333333 src/conflict.rs\0";

        let parsed = parse_status(input).expect("unmerged status should parse");

        assert_eq!(
            parsed.entries,
            vec![GitEntry {
                path: "src/conflict.rs".into(),
                orig_path: None,
                index: Some(ChangeKind::Conflicted),
                worktree: Some(ChangeKind::Conflicted),
            }]
        );
    }

    #[test]
    fn parses_detached_and_unborn_branch_states() {
        let detached = parse_status(b"# branch.oid 0123456789abcdef\0# branch.head (detached)\0")
            .expect("detached status");
        let unborn =
            parse_status(b"# branch.oid (initial)\0# branch.head topic\0").expect("unborn status");

        assert_eq!(
            (detached.branch, unborn.branch),
            (
                BranchInfo::Detached {
                    oid: "0123456".into()
                },
                BranchInfo::Named {
                    name: "topic".into(),
                    upstream: None,
                    ahead: 0,
                    behind: 0,
                    unborn: true,
                }
            )
        );
    }

    #[test]
    fn preserves_index_and_worktree_status_on_the_same_entry() {
        let input = b"# branch.oid 0123456789abcdef\0# branch.head main\0\x31 MM N... 100644 100644 100644 0123456 789abcd src/both.rs\0";

        let parsed = parse_status(input).expect("dual-area status");

        assert_eq!(
            parsed.entries,
            vec![GitEntry {
                path: "src/both.rs".into(),
                orig_path: None,
                index: Some(ChangeKind::Modified),
                worktree: Some(ChangeKind::Modified),
            }]
        );
    }
}

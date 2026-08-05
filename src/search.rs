use std::{
    collections::BinaryHeap,
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

const MAX_INDEXED_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_INDEXED_CONTENT_BYTES: usize = 64 * 1024 * 1024;
const FILE_RESULT_LIMIT: usize = 10;
const CONTENT_RESULT_LIMIT: usize = 20;

struct IndexedFile {
    path: PathBuf,
    relative: String,
    relative_lower: String,
    filename_lower: String,
    content: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchHit {
    pub path: PathBuf,
    pub relative: String,
    pub line: Option<usize>,
    pub preview: String,
}

#[derive(Clone, Debug, Default)]
pub struct SearchResults {
    pub query: String,
    pub files: Vec<SearchHit>,
    pub contents: Vec<SearchHit>,
    pub indexed_files: usize,
    pub complete: bool,
}

pub struct SearchController {
    requests: Sender<String>,
    updates: Receiver<SearchResults>,
    results: SearchResults,
}

impl SearchController {
    pub fn new(root: PathBuf) -> Result<Self, String> {
        let (request_tx, request_rx) = mpsc::channel();
        let (update_tx, update_rx) = mpsc::channel();
        thread::Builder::new()
            .name("editur-search-index".into())
            .spawn(move || search_worker(root, request_rx, update_tx))
            .map_err(|error| format!("cannot start project search index: {error}"))?;
        Ok(Self {
            requests: request_tx,
            updates: update_rx,
            results: SearchResults::default(),
        })
    }

    pub fn set_query(&self, query: &str) -> Result<(), String> {
        self.requests
            .send(query.to_owned())
            .map_err(|_| "project search index stopped unexpectedly".to_owned())
    }

    pub fn poll(&mut self, current_query: &str) -> bool {
        let mut changed = false;
        for update in self.updates.try_iter() {
            if update.query == current_query {
                self.results = update;
                changed = true;
            }
        }
        changed
    }

    pub fn results(&self) -> &SearchResults {
        &self.results
    }
}

fn search_worker(root: PathBuf, requests: Receiver<String>, updates: Sender<SearchResults>) {
    let mut documents = Vec::new();
    let mut query = loop {
        let Ok(next) = requests.recv() else {
            return;
        };
        if !next.trim().is_empty() {
            break next;
        }
    };
    drain_query(&requests, &mut query);
    let mut last_update = Instant::now();
    let completed = walk_root(&root, |document| {
        documents.push(document);
        let query_changed = drain_query(&requests, &mut query);
        if query_changed || last_update.elapsed() >= Duration::from_millis(50) {
            if !publish_latest(&documents, &requests, &updates, &mut query, false) {
                return false;
            }
            last_update = Instant::now();
        }
        true
    });
    if !completed || !publish_latest(&documents, &requests, &updates, &mut query, true) {
        return;
    }

    while let Ok(next) = requests.recv() {
        query = next;
        drain_query(&requests, &mut query);
        if !publish_latest(&documents, &requests, &updates, &mut query, true) {
            break;
        }
    }
}

fn publish_latest(
    documents: &[IndexedFile],
    requests: &Receiver<String>,
    updates: &Sender<SearchResults>,
    query: &mut String,
    complete: bool,
) -> bool {
    loop {
        let current = query.clone();
        if let Some(results) = search_documents_while(documents, &current, complete, || {
            drain_query(requests, query)
        }) {
            return updates.send(results).is_ok();
        }
    }
}

fn drain_query(requests: &Receiver<String>, query: &mut String) -> bool {
    let mut changed = false;
    for next in requests.try_iter() {
        *query = next;
        changed = true;
    }
    changed
}

#[cfg(test)]
fn index_root(root: &Path) -> Vec<IndexedFile> {
    let mut documents = Vec::new();
    walk_root(root, |document| {
        documents.push(document);
        true
    });
    documents
}

fn walk_root(root: &Path, mut visit: impl FnMut(IndexedFile) -> bool) -> bool {
    let mut directories = vec![root.to_path_buf()];
    let mut remaining_content_bytes = MAX_INDEXED_CONTENT_BYTES;
    while let Some(directory) = directories.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                if !ignored_directory(&entry.file_name().to_string_lossy()) {
                    directories.push(entry.path());
                }
            } else if file_type.is_file()
                && let Some(document) = index_file(root, entry.path(), &mut remaining_content_bytes)
                && !visit(document)
            {
                return false;
            }
        }
    }
    true
}

fn ignored_directory(name: &str) -> bool {
    matches!(
        name,
        ".git" | ".hg" | ".svn" | "node_modules" | "target" | ".next"
    )
}

fn index_file(
    root: &Path,
    path: PathBuf,
    remaining_content_bytes: &mut usize,
) -> Option<IndexedFile> {
    let relative = path
        .strip_prefix(root)
        .ok()?
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    let mut relative_lower = relative.clone();
    relative_lower.make_ascii_lowercase();
    let mut filename_lower = path.file_name()?.to_string_lossy().into_owned();
    filename_lower.make_ascii_lowercase();
    let content = path
        .metadata()
        .ok()
        .filter(|metadata| {
            metadata.len() <= MAX_INDEXED_FILE_BYTES
                && metadata.len() as usize <= *remaining_content_bytes
        })
        .and_then(|_| fs::read(&path).ok())
        .filter(|bytes| !bytes.contains(&0))
        .and_then(|bytes| String::from_utf8(bytes).ok());
    if let Some(content) = &content {
        *remaining_content_bytes -= content.len();
    }
    Some(IndexedFile {
        path,
        relative,
        relative_lower,
        filename_lower,
        content,
    })
}

#[cfg(test)]
fn search_documents(documents: &[IndexedFile], query: &str, complete: bool) -> SearchResults {
    match search_documents_while(documents, query, complete, || false) {
        Some(results) => results,
        None => unreachable!("an uncancelled search cannot stop early"),
    }
}

fn search_documents_while(
    documents: &[IndexedFile],
    query: &str,
    complete: bool,
    mut cancelled: impl FnMut() -> bool,
) -> Option<SearchResults> {
    let query = query.trim();
    let mut results = SearchResults {
        query: query.to_owned(),
        indexed_files: documents.len(),
        complete,
        ..SearchResults::default()
    };
    if query.is_empty() {
        return Some(results);
    }
    let mut needle = query.to_owned();
    needle.make_ascii_lowercase();

    let mut files = BinaryHeap::with_capacity(FILE_RESULT_LIMIT + 1);
    for (index, document) in documents.iter().enumerate() {
        if index % 64 == 0 && cancelled() {
            return None;
        }
        let Some(score) = filename_score(document, &needle) else {
            continue;
        };
        files.push((
            score,
            document.relative.len(),
            document.relative.as_str(),
            index,
        ));
        if files.len() > FILE_RESULT_LIMIT {
            files.pop();
        }
    }
    results.files = files
        .into_sorted_vec()
        .into_iter()
        .map(|(_, _, _, index)| SearchHit {
            path: documents[index].path.clone(),
            relative: documents[index].relative.clone(),
            line: None,
            preview: "Filename match".into(),
        })
        .collect();

    for (index, document) in documents.iter().enumerate() {
        if index % 64 == 0 && cancelled() {
            return None;
        }
        let Some(content) = document.content.as_ref() else {
            continue;
        };
        if let Some(offset) = find_ascii_case_insensitive(content.as_bytes(), needle.as_bytes()) {
            let (line, preview) = line_preview(content, offset);
            results.contents.push(SearchHit {
                path: document.path.clone(),
                relative: document.relative.clone(),
                line: Some(line),
                preview,
            });
            if results.contents.len() == CONTENT_RESULT_LIMIT {
                break;
            }
        }
    }
    Some(results)
}

fn find_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|candidate| candidate.eq_ignore_ascii_case(needle))
}

fn filename_score(document: &IndexedFile, needle: &str) -> Option<u8> {
    if document.filename_lower == needle {
        Some(0)
    } else if document.filename_lower.starts_with(needle) {
        Some(1)
    } else if document.filename_lower.contains(needle) {
        Some(2)
    } else if document.relative_lower.contains(needle) {
        Some(3)
    } else {
        None
    }
}

fn line_preview(content: &str, offset: usize) -> (usize, String) {
    let line_start = content[..offset].rfind('\n').map_or(0, |index| index + 1);
    let line_end = content[offset..]
        .find('\n')
        .map_or(content.len(), |index| offset + index);
    let line = content[..line_start]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    let text = &content[line_start..line_end];
    let match_character = text[..offset - line_start].chars().count();
    let characters = text.chars().count();
    let start = match_character
        .saturating_sub(80)
        .min(characters.saturating_sub(240));
    let preview: String = text
        .chars()
        .skip(start)
        .take(240)
        .collect::<String>()
        .trim()
        .to_owned();
    (line, preview)
}

#[cfg(test)]
mod tests {
    use super::{SearchController, index_root, search_documents};
    use std::fs;
    use std::time::Duration;

    #[test]
    fn project_index_stays_idle_until_the_first_nonempty_query() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("needle.txt"), "needle\n").unwrap();
        let mut search = SearchController::new(temp.path().to_path_buf()).unwrap();

        std::thread::sleep(Duration::from_millis(100));
        search.poll("");

        assert!(!search.results().complete);
    }

    #[test]
    fn categorizes_ranked_filename_and_content_matches() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("src")).unwrap();
        fs::create_dir_all(temp.path().join("docs")).unwrap();
        fs::create_dir_all(temp.path().join("target")).unwrap();
        fs::write(
            temp.path().join("src/parser.rs"),
            "fn parse() {}\nthe hidden needle is here\n",
        )
        .unwrap();
        fs::write(temp.path().join("docs/needle-guide.md"), "documentation\n").unwrap();
        fs::write(temp.path().join("target/hidden.rs"), "needle\n").unwrap();

        let documents = index_root(temp.path());
        let results = search_documents(&documents, "needle", true);

        assert_eq!(results.files.len(), 1);
        assert_eq!(results.files[0].relative, "docs/needle-guide.md");
        assert_eq!(results.contents.len(), 1);
        assert_eq!(results.contents[0].relative, "src/parser.rs");
        assert_eq!(results.contents[0].line, Some(2));
        assert!(results.contents[0].preview.contains("hidden needle"));
        assert!(
            results
                .files
                .iter()
                .chain(&results.contents)
                .all(|result| !result.relative.starts_with("target/"))
        );
    }

    #[test]
    fn exact_filename_matches_rank_before_partial_paths() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("find.rs"), "").unwrap();
        fs::write(temp.path().join("prefix-find.rs"), "").unwrap();
        let documents = index_root(temp.path());

        let results = search_documents(&documents, "find.rs", true);

        assert_eq!(results.files[0].relative, "find.rs");
    }

    #[test]
    fn long_content_previews_keep_the_match_visible() {
        let temp = tempfile::tempdir().unwrap();
        let line = format!("{}needle{}", "x".repeat(300), "y".repeat(300));
        fs::write(temp.path().join("long.txt"), line).unwrap();
        let documents = index_root(temp.path());

        let results = search_documents(&documents, "needle", true);

        assert!(results.contents[0].preview.contains("needle"));
        assert!(results.contents[0].preview.chars().count() <= 240);
    }
}

//! Parsers for Git's machine-readable output formats.

use crate::types::{Branch, ChangeKind, Commit, DiffFile, FileChange, Remote, StashEntry, Status};

/// Field separator used in `--format` strings (ASCII unit separator).
pub const FIELD: char = '\u{1f}';
/// Record separator used in `--format` strings (ASCII record separator).
pub const RECORD: char = '\u{1e}';

/// Parses `git status --porcelain=v2 --branch -z --untracked-files=all`.
pub fn status(output: &str, root: &str) -> Status {
    let mut status = Status {
        root: root.to_owned(),
        ..Status::default()
    };
    let mut tokens = output.split('\0').filter(|t| !t.is_empty());
    while let Some(token) = tokens.next() {
        if let Some(header) = token.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" if value != "(initial)" => status.head = Some(value.to_owned()),
                "branch.head" => {
                    if value == "(detached)" {
                        status.detached = true;
                    } else {
                        status.branch = Some(value.to_owned());
                    }
                }
                "branch.upstream" => status.upstream = Some(value.to_owned()),
                "branch.ab" => {
                    for part in value.split_whitespace() {
                        if let Some(n) = part.strip_prefix('+') {
                            status.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            status.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let mut kind = token.chars();
        match kind.next() {
            Some('1') => {
                // 1 XY sub mH mI mW hH hI path
                let fields: Vec<&str> = token.splitn(9, ' ').collect();
                if let (Some(xy), Some(path)) = (fields.get(1), fields.get(8)) {
                    status.files.push(change(xy, path, None));
                }
            }
            Some('2') => {
                // 2 XY sub mH mI mW hH hI Xscore path \0 origPath
                let fields: Vec<&str> = token.splitn(10, ' ').collect();
                let original = tokens.next();
                if let (Some(xy), Some(path)) = (fields.get(1), fields.get(9)) {
                    status.files.push(change(xy, path, original));
                }
            }
            Some('u') => {
                // u XY sub m1 m2 m3 mW h1 h2 h3 path
                let fields: Vec<&str> = token.splitn(11, ' ').collect();
                if let Some(path) = fields.get(10) {
                    status.files.push(FileChange {
                        path: (*path).to_owned(),
                        original_path: None,
                        staged: None,
                        unstaged: Some(ChangeKind::Conflicted),
                        conflicted: true,
                    });
                }
            }
            Some('?') => {
                if let Some(path) = token.get(2..) {
                    status.files.push(FileChange {
                        path: path.to_owned(),
                        original_path: None,
                        staged: None,
                        unstaged: Some(ChangeKind::Untracked),
                        conflicted: false,
                    });
                }
            }
            _ => {}
        }
    }
    status.clean = status.files.is_empty();
    status
}

fn change(xy: &str, path: &str, original: Option<&str>) -> FileChange {
    let mut codes = xy.chars();
    let index = codes.next().unwrap_or('.');
    let worktree = codes.next().unwrap_or('.');
    FileChange {
        path: path.to_owned(),
        original_path: original.map(str::to_owned),
        staged: ChangeKind::from_code(index),
        unstaged: ChangeKind::from_code(worktree),
        conflicted: false,
    }
}

/// `--format` for [`log`]: hash, short hash, parents, author, email, date, subject.
pub const LOG_FORMAT: &str = "%H%x1f%h%x1f%P%x1f%an%x1f%ae%x1f%aI%x1f%s%x1e";

pub fn log(output: &str) -> Vec<Commit> {
    records(output)
        .filter_map(|fields| {
            let [hash, short, parents, author, email, date, subject] = fields[..] else {
                return None;
            };
            Some(Commit {
                hash: hash.to_owned(),
                short_hash: short.to_owned(),
                parents: parents.split_whitespace().map(str::to_owned).collect(),
                author: author.to_owned(),
                email: email.to_owned(),
                date: date.to_owned(),
                subject: subject.to_owned(),
            })
        })
        .collect()
}

/// `--format` for [`branches`] (`git for-each-ref`).
pub const BRANCH_FORMAT: &str = "%(refname)%1f%(refname:short)%1f%(HEAD)%1f%(upstream:short)%1f%(objectname:short)%1f%(committerdate:iso-strict)%1f%(contents:subject)%1e";

pub fn branches(output: &str) -> Vec<Branch> {
    records(output)
        .filter_map(|fields| {
            let [full, short, head, upstream, commit, date, subject] = fields[..] else {
                return None;
            };
            // `refs/remotes/origin/HEAD` is a pointer, not a branch.
            if full.ends_with("/HEAD") {
                return None;
            }
            Some(Branch {
                name: short.to_owned(),
                remote: full.starts_with("refs/remotes/"),
                current: head == "*",
                upstream: (!upstream.is_empty()).then(|| upstream.to_owned()),
                commit: commit.to_owned(),
                date: date.to_owned(),
                subject: subject.to_owned(),
            })
        })
        .collect()
}

/// `--format` for [`stashes`] (`git stash list`).
pub const STASH_FORMAT: &str = "%gd%x1f%gs%x1f%cI%x1e";

pub fn stashes(output: &str) -> Vec<StashEntry> {
    records(output)
        .filter_map(|fields| {
            let [reference, message, date] = fields[..] else {
                return None;
            };
            let index = reference
                .strip_prefix("stash@{")?
                .strip_suffix('}')?
                .parse()
                .ok()?;
            Some(StashEntry {
                reference: reference.to_owned(),
                index,
                message: message.to_owned(),
                date: date.to_owned(),
            })
        })
        .collect()
}

/// Parses `git remote -v` (one entry per remote, fetch URL preferred).
pub fn remotes(output: &str) -> Vec<Remote> {
    let mut remotes: Vec<Remote> = Vec::new();
    for line in output.lines() {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(url)) = (parts.next(), parts.next()) else {
            continue;
        };
        if !remotes.iter().any(|r| r.name == name) {
            remotes.push(Remote {
                name: name.to_owned(),
                url: url.to_owned(),
            });
        }
    }
    remotes
}

/// Parses `git diff --numstat -z`.
pub fn numstat(output: &str) -> Vec<DiffFile> {
    let mut files = Vec::new();
    let mut tokens = output.split('\0');
    while let Some(token) = tokens.next() {
        if token.is_empty() {
            continue;
        }
        let mut parts = token.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let binary = added == "-" && deleted == "-";
        let (path, original_path) = if path.is_empty() {
            // Rename: "<a>\t<d>\t\0<old>\0<new>\0".
            let old = tokens.next().unwrap_or_default();
            let new = tokens.next().unwrap_or_default();
            (new.to_owned(), Some(old.to_owned()))
        } else {
            (path.to_owned(), None)
        };
        files.push(DiffFile {
            path,
            original_path,
            additions: added.parse().ok(),
            deletions: deleted.parse().ok(),
            binary,
        });
    }
    files
}

/// Splits `%x1e`-terminated records into `%x1f`-separated fields.
fn records(output: &str) -> impl Iterator<Item = Vec<&str>> {
    output
        .split(RECORD)
        .map(|record| record.trim_start_matches(['\n', '\r']))
        .filter(|record| !record.is_empty())
        .map(|record| record.split(FIELD).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_headers_and_entries() {
        let out = [
            "# branch.oid 1234567890abcdef",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -1",
            "1 .M N... 100644 100644 100644 aaa bbb src/app file.ts",
            "1 A. N... 000000 100644 100644 000 ccc new.txt",
            "1 D. N... 100644 000000 000000 ddd 000 gone.txt",
            "2 R. N... 100644 100644 100644 eee eee R100 renamed.txt",
            "old name.txt",
            "u UU N... 100644 100644 100644 100644 f1 f2 f3 both.txt",
            "? notes/todo.md",
            "",
        ]
        .join("\0");
        let status = status(&out, "/repo");
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.head.as_deref(), Some("1234567890abcdef"));
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert_eq!((status.ahead, status.behind), (2, 1));
        assert!(!status.clean);
        assert_eq!(status.files.len(), 6);

        let modified = &status.files[0];
        assert_eq!(modified.path, "src/app file.ts");
        assert_eq!(modified.staged, None);
        assert_eq!(modified.unstaged, Some(ChangeKind::Modified));

        assert_eq!(status.files[1].staged, Some(ChangeKind::Added));
        assert_eq!(status.files[2].staged, Some(ChangeKind::Deleted));

        let renamed = &status.files[3];
        assert_eq!(renamed.path, "renamed.txt");
        assert_eq!(renamed.original_path.as_deref(), Some("old name.txt"));
        assert_eq!(renamed.staged, Some(ChangeKind::Renamed));

        assert!(status.files[4].conflicted);
        assert_eq!(status.files[5].unstaged, Some(ChangeKind::Untracked));
        assert_eq!(status.files[5].path, "notes/todo.md");
    }

    #[test]
    fn parses_initial_and_detached_heads() {
        let initial = status("# branch.oid (initial)\0# branch.head main\0", "/r");
        assert_eq!(initial.head, None);
        assert!(initial.clean);

        let detached = status("# branch.oid abc\0# branch.head (detached)\0", "/r");
        assert!(detached.detached);
        assert_eq!(detached.branch, None);
    }

    #[test]
    fn parses_log_records() {
        let out = format!(
            "h1{f}a1{f}p1 p2{f}Ana{f}ana@x.dev{f}2026-01-02T03:04:05+00:00{f}Merge{r}\nh2{f}a2{f}{f}Bia{f}bia@x.dev{f}2026-01-01T00:00:00+00:00{f}Primeiro commit{r}\n",
            f = FIELD,
            r = RECORD
        );
        let commits = log(&out);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].parents, vec!["p1", "p2"]);
        assert!(commits[1].parents.is_empty());
        assert_eq!(commits[1].subject, "Primeiro commit");
    }

    #[test]
    fn parses_branches_and_skips_remote_head() {
        let out = format!(
            "refs/heads/main{f}main{f}*{f}origin/main{f}abc{f}2026-01-01T00:00:00Z{f}init{r}\nrefs/remotes/origin/HEAD{f}origin{f} {f}{f}abc{f}2026-01-01T00:00:00Z{f}init{r}\nrefs/remotes/origin/main{f}origin/main{f} {f}{f}abc{f}2026-01-01T00:00:00Z{f}init{r}\n",
            f = FIELD,
            r = RECORD
        );
        let branches = branches(&out);
        assert_eq!(branches.len(), 2);
        assert!(branches[0].current && !branches[0].remote);
        assert_eq!(branches[0].upstream.as_deref(), Some("origin/main"));
        assert!(branches[1].remote && !branches[1].current);
    }

    #[test]
    fn parses_numstat_including_renames_and_binaries() {
        let out = "3\t1\tsrc/a.rs\0-\t-\timg.png\0\
                   0\t0\t\0old.txt\0new.txt\0";
        let files = numstat(out);
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].additions, Some(3));
        assert!(files[1].binary && files[1].additions.is_none());
        assert_eq!(files[2].path, "new.txt");
        assert_eq!(files[2].original_path.as_deref(), Some("old.txt"));
    }

    #[test]
    fn parses_remotes_and_stashes() {
        let remotes = remotes(
            "origin\thttps://example.com/a.git (fetch)\norigin\thttps://example.com/a.git (push)\nfork\tgit@x:y.git (fetch)\n",
        );
        assert_eq!(remotes.len(), 2);
        assert_eq!(remotes[1].name, "fork");

        let out = format!(
            "stash@{{0}}{f}On main: wip{f}2026-01-01T00:00:00Z{r}\n",
            f = FIELD,
            r = RECORD
        );
        let stashes = stashes(&out);
        assert_eq!(stashes[0].index, 0);
        assert_eq!(stashes[0].message, "On main: wip");
    }
}

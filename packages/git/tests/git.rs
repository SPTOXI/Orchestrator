//! Integration tests against real temporary repositories (no network: the
//! "remote" is a bare repository on disk).

use orchestrator_git::{
    ChangeKind, DiffOptions, Git, GitError, PullMode, PullOptions, PushOptions, ResetMode,
};
use std::fs;
use std::path::Path;
use std::process::Command;

fn git() -> Git {
    Git::detect().expect("git must be installed to run these tests")
}

/// Runs raw git for test setup.
fn sh(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("run git");
    assert!(
        status.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}

fn init_repo(dir: &Path) {
    sh(dir, &["init", "-q"]);
    sh(dir, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    sh(dir, &["config", "user.name", "Orchestrator Test"]);
    sh(dir, &["config", "user.email", "test@orchestrator.dev"]);
    sh(dir, &["config", "commit.gpgsign", "false"]);
    sh(dir, &["config", "core.autocrlf", "false"]);
}

fn repo_with_commit() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    fs::write(dir.path().join("README.md"), "# demo\n").unwrap();
    sh(dir.path(), &["add", "README.md"]);
    sh(dir.path(), &["commit", "-q", "-m", "Primeiro commit"]);
    dir
}

#[test]
fn status_reports_every_kind_of_change() {
    let repo = repo_with_commit();
    let root = repo.path();
    let git = git();

    let clean = git.status(root).unwrap();
    assert!(clean.clean);
    assert_eq!(clean.branch.as_deref(), Some("main"));
    assert!(clean.head.is_some());

    fs::write(root.join("README.md"), "# demo\nmais\n").unwrap();
    fs::write(root.join("novo arquivo.txt"), "x").unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "fn a() {}").unwrap();
    sh(root, &["add", "src/lib.rs"]);

    let status = git.status(root).unwrap();
    assert!(!status.clean);
    let find = |p: &str| status.files.iter().find(|f| f.path == p).unwrap();
    assert_eq!(find("README.md").unstaged, Some(ChangeKind::Modified));
    assert_eq!(
        find("novo arquivo.txt").unstaged,
        Some(ChangeKind::Untracked)
    );
    assert_eq!(find("src/lib.rs").staged, Some(ChangeKind::Added));
}

#[test]
fn add_commit_log_and_diff() {
    let repo = repo_with_commit();
    let root = repo.path();
    let git = git();

    fs::write(root.join("README.md"), "# demo\nlinha nova\n").unwrap();
    let unstaged = git.diff(root, &DiffOptions::default()).unwrap();
    assert!(unstaged.patch.contains("+linha nova"));
    assert_eq!(unstaged.files[0].additions, Some(1));

    git.add(root, &["README.md".into()], false).unwrap();
    let staged = git
        .diff(
            root,
            &DiffOptions {
                staged: true,
                ..DiffOptions::default()
            },
        )
        .unwrap();
    assert!(staged.patch.contains("+linha nova"));

    let result = git.commit(root, "Adiciona linha", false, false).unwrap();
    assert_eq!(result.subject, "Adiciona linha");
    assert_eq!(result.branch.as_deref(), Some("main"));

    let log = git.log(root, 10, None, None).unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].hash, result.hash);
    assert_eq!(log[1].subject, "Primeiro commit");
    assert_eq!(log[0].parents, vec![log[1].hash.clone()]);

    let truncated = git
        .diff(
            root,
            &DiffOptions {
                target: Some("HEAD~1".into()),
                max_bytes: Some(10),
                ..DiffOptions::default()
            },
        )
        .unwrap();
    assert!(truncated.truncated);
    assert!(truncated.patch.len() <= 10);
}

#[test]
fn empty_commit_message_and_nothing_to_commit_fail() {
    let repo = repo_with_commit();
    let git = git();
    assert!(matches!(
        git.commit(repo.path(), "  ", false, false),
        Err(GitError::Invalid(_))
    ));
    let err = git.commit(repo.path(), "nada", false, false).unwrap_err();
    assert!(matches!(err, GitError::Failed { .. }), "{err}");
}

#[test]
fn unborn_repository_has_no_commits_and_can_unstage() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let git = git();

    let status = git.status(dir.path()).unwrap();
    assert_eq!(status.head, None);
    assert!(git.log(dir.path(), 5, None, None).unwrap().is_empty());

    fs::write(dir.path().join("a.txt"), "a").unwrap();
    git.add(dir.path(), &[], true).unwrap();
    assert_eq!(
        git.status(dir.path()).unwrap().files[0].staged,
        Some(ChangeKind::Added)
    );
    git.reset(dir.path(), ResetMode::Mixed, None, &["a.txt".into()])
        .unwrap();
    assert_eq!(
        git.status(dir.path()).unwrap().files[0].unstaged,
        Some(ChangeKind::Untracked)
    );
}

#[test]
fn branches_checkout_and_delete() {
    let repo = repo_with_commit();
    let root = repo.path();
    let git = git();

    git.create_branch(root, "feature/x", None).unwrap();
    let names: Vec<_> = git
        .branches(root)
        .unwrap()
        .into_iter()
        .map(|b| (b.name, b.current))
        .collect();
    assert!(names.contains(&("main".into(), true)));
    assert!(names.contains(&("feature/x".into(), false)));

    git.checkout(root, "feature/x", false, None).unwrap();
    assert_eq!(
        git.status(root).unwrap().branch.as_deref(),
        Some("feature/x")
    );

    git.checkout(root, "outra", true, None).unwrap();
    assert_eq!(git.status(root).unwrap().branch.as_deref(), Some("outra"));

    git.checkout(root, "main", false, None).unwrap();
    git.delete_branch(root, "outra", false).unwrap();
    assert!(!git
        .branches(root)
        .unwrap()
        .iter()
        .any(|b| b.name == "outra"));

    assert!(matches!(
        git.checkout(root, "--orphan", false, None),
        Err(GitError::Invalid(_))
    ));
}

#[test]
fn stash_push_list_pop() {
    let repo = repo_with_commit();
    let root = repo.path();
    let git = git();

    fs::write(root.join("README.md"), "alterado\n").unwrap();
    git.stash_push(root, Some("trabalho em andamento"), false)
        .unwrap();
    assert!(git.status(root).unwrap().clean);

    let stashes = git.stash_list(root).unwrap();
    assert_eq!(stashes.len(), 1);
    assert!(stashes[0].message.contains("trabalho em andamento"));

    git.stash_apply(root, "pop", 0).unwrap();
    assert!(!git.status(root).unwrap().clean);
    assert!(git.stash_list(root).unwrap().is_empty());
}

#[test]
fn reset_soft_and_hard() {
    let repo = repo_with_commit();
    let root = repo.path();
    let git = git();

    fs::write(root.join("b.txt"), "b").unwrap();
    git.add(root, &[], true).unwrap();
    git.commit(root, "Segundo", false, false).unwrap();

    git.reset(root, ResetMode::Soft, Some("HEAD~1"), &[])
        .unwrap();
    let status = git.status(root).unwrap();
    assert_eq!(status.files[0].staged, Some(ChangeKind::Added));

    git.reset(root, ResetMode::Hard, None, &[]).unwrap();
    assert!(!root.join("b.txt").exists());
    assert!(git.status(root).unwrap().clean);

    assert!(matches!(
        git.reset(root, ResetMode::Hard, None, &["x".into()]),
        Err(GitError::Invalid(_))
    ));
}

#[test]
fn push_and_pull_through_a_local_remote() {
    let remote = tempfile::tempdir().unwrap();
    sh(remote.path(), &["init", "-q", "--bare"]);
    sh(remote.path(), &["symbolic-ref", "HEAD", "refs/heads/main"]);

    let repo = repo_with_commit();
    let root = repo.path();
    let git = git();
    sh(
        root,
        &[
            "remote",
            "add",
            "origin",
            &remote.path().display().to_string(),
        ],
    );
    assert_eq!(git.remotes(root).unwrap()[0].name, "origin");

    git.push(
        root,
        &PushOptions {
            remote: Some("origin".into()),
            branch: Some("main".into()),
            set_upstream: true,
            ..PushOptions::default()
        },
    )
    .unwrap();
    let status = git.status(root).unwrap();
    assert_eq!(status.upstream.as_deref(), Some("origin/main"));
    assert_eq!((status.ahead, status.behind), (0, 0));

    // A second clone pushes a commit; the first pulls it.
    let other = tempfile::tempdir().unwrap();
    let clone_path = other.path().join("clone");
    sh(
        other.path(),
        &[
            "clone",
            "-q",
            &remote.path().display().to_string(),
            &clone_path.display().to_string(),
        ],
    );
    sh(&clone_path, &["config", "user.name", "Outro"]);
    sh(
        &clone_path,
        &["config", "user.email", "outro@orchestrator.dev"],
    );
    sh(&clone_path, &["config", "commit.gpgsign", "false"]);
    fs::write(clone_path.join("remoto.txt"), "r").unwrap();
    sh(&clone_path, &["add", "."]);
    sh(&clone_path, &["commit", "-q", "-m", "Do outro clone"]);
    sh(&clone_path, &["push", "-q"]);

    git.pull(
        root,
        &PullOptions {
            mode: Some(PullMode::FfOnly),
            ..PullOptions::default()
        },
    )
    .unwrap();
    assert!(root.join("remoto.txt").exists());
    assert_eq!(
        git.log(root, 1, None, None).unwrap()[0].subject,
        "Do outro clone"
    );
}

#[test]
fn outside_a_repository_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let err = git().status(dir.path()).unwrap_err();
    assert!(matches!(err, GitError::NotARepository(_)), "{err}");
}

#[test]
fn version_is_reported() {
    assert!(git().version().unwrap().starts_with("git version"));
}

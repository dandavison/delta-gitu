use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Once, OnceLock},
};

use git2::ConfigLevel;

use temp_dir::TempDir;
use url::Url;

pub struct RepoTestContext {
    pub dir: PathBuf,
    pub remote_dir: PathBuf,
    pub local_repo: git2::Repository,
}

fn open_repo(dir: &Path) -> git2::Repository {
    git2::Repository::open(dir).unwrap()
}

#[macro_export]
macro_rules! repo_setup_clone {
    () => {{ RepoTestContext::setup_clone(function_name!()) }};
}

impl RepoTestContext {
    /// A remote with one commit, and a clone of it, for the test `test_name`: a copy of the pair
    /// [`template`] makes once a run, with the clone's remote its own.
    pub fn setup_clone(test_name: &str) -> Self {
        fs::create_dir_all("testfiles").unwrap();
        let testfiles = fs::canonicalize("testfiles").unwrap();
        let dir = &testfiles.join(test_name.replace(":", "_"));
        fs::create_dir_all(dir).unwrap();
        fs::remove_dir_all(dir).unwrap();

        set_env_vars();
        let template = template();
        let local_dir = dir.join("local");
        let remote_dir = dir.join("remote");
        copy_dir(&template.join("remote"), &remote_dir);
        copy_dir(&template.join("local"), &local_dir);
        // git clone recorded where it cloned from in the clone's config and its reflog.
        let url = |remote: &Path| Url::from_file_path(remote).unwrap().to_string();
        replace_in_files(
            &local_dir.join(".git"),
            &url(&template.join("remote")),
            &url(&remote_dir),
        );

        let local_repo = open_repo(&local_dir);
        assert_local_test_repo(&local_repo, &local_dir, &remote_dir);
        assert_remote_test_repo(&open_repo(&remote_dir));

        Self {
            local_repo,
            dir: local_dir,
            remote_dir,
        }
    }
}

/// A bare remote with one commit, `initial-file`, and a clone of it, in `remote` and `local` of
/// the directory returned: made once a run, as making them takes a dozen git commands, which
/// would be most of what most tests take, where copying them takes milliseconds.
fn template() -> &'static Path {
    static TEMPLATE: OnceLock<PathBuf> = OnceLock::new();
    TEMPLATE.get_or_init(|| {
        let dir = env::temp_dir().join(format!("gitu-test-repos-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let local_dir = dir.join("local");
        let remote_dir = dir.join("remote");
        fs::create_dir_all(&local_dir).unwrap();
        fs::create_dir_all(&remote_dir).unwrap();
        // As tests compare paths, without the symlinks macOS's temporary directory is under.
        let local_dir = fs::canonicalize(local_dir).unwrap();
        let remote_dir = fs::canonicalize(remote_dir).unwrap();

        set_env_vars();
        run(
            &remote_dir,
            &["git", "init", "--bare", "--initial-branch=main"],
        );
        set_config(&remote_dir);
        clone_and_commit(&remote_dir, "initial-file", "hello");

        let url = Url::from_file_path(&remote_dir).unwrap().to_string();
        run(&local_dir, &["git", "clone", &url, "."]);
        set_config(&local_dir);
        local_dir.parent().unwrap().to_path_buf()
    })
}

/// Copy the directory `from`, and all in it, to `to`.
fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Replace `from` with `to` in the files `git_dir` records a remote's URL in: its config, and
/// its reflogs.
fn replace_in_files(git_dir: &Path, from: &str, to: &str) {
    let replace = |path: &Path| {
        let text = fs::read_to_string(path).unwrap();
        fs::write(path, text.replace(from, to)).unwrap();
    };
    replace(&git_dir.join("config"));
    fn logs(dir: &Path, replace: &dyn Fn(&Path)) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                logs(&path, replace);
            } else {
                replace(&path);
            }
        }
    }
    logs(&git_dir.join("logs"), &replace);
}

/// The commit every test repository starts at.
const INITIAL_COMMIT: &str = "b66a0bf";

/// Just to make sure we're not accidentally modifying gitu's repo: the clone is the test's, at
/// the initial commit, and its remote is the test's.
fn assert_local_test_repo(repo: &git2::Repository, local_dir: &Path, remote_dir: &Path) {
    assert_eq!(
        repo.workdir().unwrap(),
        fs::canonicalize(local_dir).unwrap().join("")
    );
    let head = repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    assert!(head.starts_with(INITIAL_COMMIT), "{head}");

    let origin = repo.find_remote("origin").unwrap();
    let actual = Url::parse(origin.url().unwrap()).unwrap();
    let expected = Url::from_file_path(remote_dir).unwrap();
    assert_eq!(actual, expected, "unexpected origin URL for test repo");
}

fn assert_remote_test_repo(repo: &git2::Repository) {
    assert!(repo.is_bare());
    let main = repo
        .find_reference("refs/heads/main")
        .unwrap()
        .peel_to_commit()
        .unwrap();
    assert!(main.id().to_string().starts_with(INITIAL_COMMIT));
    assert!(repo.remotes().unwrap().is_empty());
}

pub fn set_env_vars() {
    // https://git-scm.com/book/en/v2/Git-Internals-Environment-Variables
    unsafe {
        env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
        env::set_var("GIT_CONFIG_SYSTEM", "/dev/null");
        env::set_var("GIT_AUTHOR_NAME", "Author Name");
        env::set_var("GIT_AUTHOR_EMAIL", "author@email.com");
        env::set_var("GIT_AUTHOR_DATE", "Fri Feb 16 11:11 2024 +0100");
        env::set_var("GIT_COMMITTER_NAME", "Committer Name");
        env::set_var("GIT_COMMITTER_EMAIL", "committer@email.com");
        env::set_var("GIT_COMMITTER_DATE", "Sun Feb 18 14:00 2024 +0100");
        env::set_var("LC_ALL", "C");
        // Opening the editor runs a command that does nothing, rather than the
        // editor of whoever runs the tests.
        env::set_var("GITU_SHOW_EDITOR", "true");
    }

    // libgit2 finds the global config by its own search path and reads none of
    // the variables above, so it has to be emptied separately or the machine's
    // own git config reaches the tests.
    static EMPTY_SEARCH_PATH: Once = Once::new();
    EMPTY_SEARCH_PATH.call_once(|| {
        for level in [ConfigLevel::System, ConfigLevel::Global, ConfigLevel::XDG] {
            unsafe { git2::opts::set_search_path(level, "") }.unwrap();
        }
    });
}

pub fn run_ignore_status(dir: &Path, cmd: &[&str]) -> String {
    String::from_utf8(
        Command::new(cmd[0])
            .args(&cmd[1..])
            .current_dir(dir)
            .output()
            .unwrap_or_else(|_| panic!("failed to execute {:?}", cmd))
            .stderr,
    )
    .unwrap()
}

pub fn run(dir: &Path, cmd: &[&str]) -> String {
    let output = Command::new(cmd[0])
        .args(&cmd[1..])
        .current_dir(dir)
        .output()
        .unwrap_or_else(|_| panic!("failed to execute {:?}", cmd));

    if !output.status.success() {
        let stderr = String::from_utf8(output.stderr).unwrap();
        panic!("failed to execute {:?}. Output: {}", cmd, stderr)
    }

    String::from_utf8(output.stdout).unwrap()
}

fn set_config(path: &Path) {
    run(path, &["git", "config", "user.email", "ci@example.com"]);
    run(path, &["git", "config", "user.name", "CI"]);
}

pub fn clone_and_commit(remote_dir: &Path, file_name: &str, file_content: &str) {
    let other_dir = TempDir::new().unwrap();

    let url = Url::from_file_path(remote_dir).unwrap().to_string();
    run(other_dir.path(), &["git", "clone", &url, "."]);

    set_config(other_dir.path());

    commit(other_dir.path(), file_name, file_content);
    run(other_dir.path(), &["git", "push"]);
}

pub fn commit(dir: &Path, file_name: &str, contents: &str) {
    let path = dir.to_path_buf().join(file_name);
    let message = match path.try_exists() {
        Ok(true) => format!("modify {}\n\nCommit body goes here\n", file_name),
        _ => format!("add {}\n\nCommit body goes here\n", file_name),
    };
    fs::write(path, contents).expect("error writing to file");
    run(dir, &["git", "add", file_name]);
    run(dir, &["git", "commit", "-m", &message]);
}

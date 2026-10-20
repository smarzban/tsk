//! `tsk backup` and `tsk restore` through the real binary: saves start detached backups that
//! never fail the save, and a target restores into a fresh state dir (the new-laptop flow).
//! Temp dirs only; never the real store or a real remote.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tsk-cli-backup-{label}-{nanos}-{seq}"));
        fs::create_dir_all(&dir).expect("create temp dir");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn tsk(state: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tsk"))
        .args(args)
        .arg("--state-dir")
        .arg(state)
        .env_remove("TSK_AGENT")
        .env("TSK_NO_UPDATE_CHECK", "1")
        .output()
        .expect("run tsk")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn add(state: &Path, title: &str) {
    let output = tsk(state, &["add", "-t", title, "--desk"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
}

fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git");
    assert!(output.status.success(), "git {args:?}: {}", stderr(&output));
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// An empty bare repository under `root`, as its path argument.
fn bare_repo(root: &Path) -> String {
    let repo = root.join("remote.git");
    fs::create_dir_all(&repo).expect("repo dir");
    // Not `main`: backups follow the remote's default branch (Git for Windows: `master`).
    git(
        &repo,
        &["init", "--quiet", "--bare", "--initial-branch=trunk"],
    );
    repo.to_str().expect("utf-8").to_string()
}

fn commits(repo: &str) -> usize {
    git(Path::new(repo), &["rev-list", "--count", "--all"])
        .trim()
        .parse()
        .unwrap_or(0)
}

#[test]
fn a_save_backs_up_in_the_background() {
    let root = TempDir::new("save");
    let repo = bare_repo(&root.0);
    let state = root.0.join("state");
    let added = tsk(&state, &["backup", "add", &repo]);
    assert_eq!(added.status.code(), Some(0), "{}", stderr(&added));
    assert!(
        stdout(&added).contains("(every save)"),
        "{}",
        stdout(&added)
    );
    assert_eq!(commits(&repo), 0, "nothing to back up yet");

    add(&state, "first task");
    wait_for("the first push", || commits(&repo) == 1);
    let live = fs::read(state.join("tsk.json")).expect("live store");
    let pushed = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["show", "HEAD:"])
        .output()
        .expect("git");
    assert!(pushed.status.success());
    let machine = git(
        Path::new(&repo),
        &["ls-tree", "--name-only", "HEAD", "machines/"],
    );
    let machine = machine.trim();
    let copy = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["show", &format!("HEAD:{machine}/tsk.json")])
        .output()
        .expect("git show");
    assert_eq!(copy.stdout, live, "byte for byte");

    // The child records its success just after the push.
    wait_for("the recorded success", || {
        stdout(&tsk(&state, &["backup", "list"])).contains("last backup: 20")
    });
}

#[test]
fn a_failing_backup_never_fails_or_slows_the_save() {
    let state = TempDir::new("failing");
    fs::write(
        state.0.join("config.toml"),
        "[[backup]]\nurl = \"file:///nonexistent/tsk-t220-repo.git\"\n",
    )
    .expect("config");

    let started = Instant::now();
    add(&state.0, "saved anyway");
    assert!(started.elapsed() < Duration::from_secs(10));
    let listed = tsk(&state.0, &["list", "--desk", "--json"]);
    assert!(stdout(&listed).contains("saved anyway"));

    wait_for("the recorded failure", || {
        stdout(&tsk(&state.0, &["backup", "list"])).contains("last error")
    });
    let listed = stdout(&tsk(&state.0, &["backup", "list"]));
    assert!(listed.contains("git clone failed"), "{listed}");

    let now = tsk(&state.0, &["backup"]);
    assert_eq!(now.status.code(), Some(1));
    assert!(
        stderr(&now).contains("git clone failed"),
        "{}",
        stderr(&now)
    );
}

#[test]
fn a_target_restores_into_a_fresh_state_dir() {
    let root = TempDir::new("git");
    let repo = bare_repo(&root.0);

    let laptop = root.0.join("old-laptop");
    add(&laptop, "written on the old laptop");
    add(&laptop, "and another");
    let added = tsk(&laptop, &["backup", "add", &repo]);
    assert_eq!(added.status.code(), Some(0), "{}", stderr(&added));
    // `add` tests the target with a first backup.
    assert!(
        stdout(&added).contains("backed up to"),
        "{}",
        stdout(&added)
    );
    assert!(stdout(&added).contains("machines/"), "{}", stdout(&added));
    let again = tsk(&laptop, &["backup"]);
    assert!(stdout(&again).contains("up to date"), "{}", stdout(&again));

    let fresh = root.0.join("new-laptop");
    let added = tsk(&fresh, &["backup", "add", &repo]);
    assert_eq!(added.status.code(), Some(0), "{}", stderr(&added));
    assert!(
        stdout(&added).contains("reached the repository"),
        "{}",
        stdout(&added)
    );
    let fresh_config = fs::read(fresh.join("config.toml")).expect("fresh config");
    let listed = tsk(&fresh, &["restore"]);
    assert_eq!(listed.status.code(), Some(0), "{}", stderr(&listed));
    let table = stdout(&listed);
    let row = table.lines().nth(1).expect("one version row");
    let id = row.split_whitespace().nth(1).expect("id column");
    assert!(row.contains(" 2 "), "two tasks: {row}");

    let restored = tsk(&fresh, &["restore", id]);
    assert_eq!(restored.status.code(), Some(0), "{}", stderr(&restored));
    assert!(stdout(&restored).starts_with(&format!("restored {id}")));
    // Restore leaves config.toml alone and says where the backed-up one is.
    assert!(
        stdout(&restored).contains("config.toml was not changed")
            && stdout(&restored).contains("machines/"),
        "{}",
        stdout(&restored)
    );
    assert_eq!(
        fs::read(fresh.join("config.toml")).expect("config"),
        fresh_config
    );
    assert_eq!(
        fs::read(fresh.join("tsk.json")).expect("restored"),
        fs::read(laptop.join("tsk.json")).expect("original")
    );
    let tasks = stdout(&tsk(&fresh, &["list", "--desk", "--json"]));
    assert!(tasks.contains("written on the old laptop"), "{tasks}");

    let unknown = tsk(&fresh, &["restore", "deadbeef"]);
    assert_eq!(unknown.status.code(), Some(1));
    assert!(stderr(&unknown).contains("unknown version"));
    assert!(!stderr(&unknown).contains("trash restore"));
    let task = tsk(&fresh, &["restore", "T12"]);
    assert_eq!(task.status.code(), Some(1));
    assert!(
        stderr(&task).contains("did you mean `tsk trash restore T12`?"),
        "{}",
        stderr(&task)
    );
}

#[test]
fn backup_add_tests_the_target_and_keeps_it_when_the_first_backup_fails() {
    let state = TempDir::new("add-test");
    add(&state.0, "needs a home");
    let missing = state.0.join("no-such-repo.git");
    let added = tsk(
        &state.0,
        &["backup", "add", missing.to_str().expect("utf-8")],
    );
    assert_eq!(added.status.code(), Some(1), "{}", stdout(&added));
    assert!(
        stderr(&added).contains("the first backup to"),
        "{}",
        stderr(&added)
    );
    assert!(
        stderr(&added).contains("git clone failed"),
        "{}",
        stderr(&added)
    );
    assert!(
        stderr(&added).contains("then run `tsk backup`"),
        "{}",
        stderr(&added)
    );
    let listed = stdout(&tsk(&state.0, &["backup", "list"]));
    assert!(
        listed.contains("no-such-repo.git"),
        "the target is kept: {listed}"
    );
}

#[test]
fn a_relative_state_dir_backs_up() {
    let root = TempDir::new("relative");
    let repo = bare_repo(&root.0);
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_tsk"))
            .args(args)
            .args(["--state-dir", "rel"])
            .current_dir(&root.0)
            .env_remove("TSK_AGENT")
            .env("TSK_NO_UPDATE_CHECK", "1")
            .output()
            .expect("run tsk")
    };
    let added = run(&["add", "-t", "relative", "--desk"]);
    assert_eq!(added.status.code(), Some(0), "{}", stderr(&added));
    let target = run(&["backup", "add", &repo]);
    assert_eq!(target.status.code(), Some(0), "{}", stderr(&target));
    let now = run(&["backup"]);
    assert_eq!(now.status.code(), Some(0), "{}", stderr(&now));
    assert_eq!(commits(&repo), 1);
}

#[test]
fn a_target_unreachable_before_any_task_shows_its_error_in_list() {
    let state = TempDir::new("add-fresh");
    let missing = state.0.join("no-such-repo.git");
    let added = tsk(
        &state.0,
        &["backup", "add", missing.to_str().expect("utf-8")],
    );
    assert_eq!(added.status.code(), Some(1), "{}", stdout(&added));
    assert!(!state.0.join("tsk.json").exists(), "no store yet");
    let listed = stdout(&tsk(&state.0, &["backup", "list"]));
    assert!(
        listed.contains("last error (") && listed.contains("git clone failed"),
        "{listed}"
    );
}

#[test]
fn restore_of_a_task_number_points_at_trash_even_without_targets() {
    let state = TempDir::new("restore-task");
    let restored = tsk(&state.0, &["restore", "T12"]);
    assert_eq!(restored.status.code(), Some(1));
    assert!(
        stderr(&restored).contains("did you mean `tsk trash restore T12`?"),
        "{}",
        stderr(&restored)
    );
}

#[test]
fn a_row_number_is_not_sent_to_the_trash() {
    let state = TempDir::new("restore-row");
    let restored = tsk(&state.0, &["restore", "2"]);
    assert_eq!(restored.status.code(), Some(1));
    let message = stderr(&restored);
    assert!(message.contains("2 is not a version id"), "{message}");
    assert!(!message.contains("did you mean"), "{message}");
}

/// `tsk` with git told to fetch and push `url` from the local `repo`: a credential URL that
/// reaches a real repository without a network.
fn tsk_reaching(state: &Path, url: &str, repo: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tsk"))
        .args(args)
        .arg("--state-dir")
        .arg(state)
        .env_remove("TSK_AGENT")
        .env("TSK_NO_UPDATE_CHECK", "1")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", format!("url.{repo}.insteadOf"))
        .env("GIT_CONFIG_VALUE_0", url)
        .output()
        .expect("run tsk")
}

#[test]
fn credentials_in_a_target_url_are_never_printed() {
    let root = TempDir::new("credentials");
    let repo = bare_repo(&root.0);
    let url = "https://me:ghp_S3CRET@example.invalid/me/b.git";
    let shown = "https://***@example.invalid/me/b.git";
    let state = root.0.join("state");
    add(&state, "kept private");
    let run = |args: &[&str]| {
        let output = tsk_reaching(&state, url, &repo, args);
        let text = format!("{}{}", stdout(&output), stderr(&output));
        assert!(
            !text.contains("S3CRET"),
            "{args:?} printed the token: {text}"
        );
        (output, text)
    };
    let (added, text) = run(&["backup", "add", url]);
    assert_eq!(added.status.code(), Some(0), "{text}");
    assert!(text.contains(&format!("added target {shown}")), "{text}");
    assert!(
        text.contains(&format!("backed up to {shown} machines/")),
        "{text}"
    );
    let (_, text) = run(&["backup", "list"]);
    assert!(text.contains(&format!("1. {shown}")), "{text}");
    let (_, text) = run(&["backup"]);
    assert!(text.contains(&format!("{shown} is up to date")), "{text}");
    let (listed, text) = run(&["restore"]);
    assert_eq!(listed.status.code(), Some(0), "{text}");
    assert!(text.contains(shown), "{text}");
    // The location as list shows it names the target.
    let (removed, text) = run(&["backup", "remove", shown]);
    assert_eq!(removed.status.code(), Some(0), "{text}");
    assert!(
        text.contains(&format!("removed git target {shown}")),
        "{text}"
    );
    assert_eq!(commits(&repo), 1);

    // An unreachable one: git's errors quote the URL.
    let lost = root.0.join("lost");
    add(&lost, "kept private too");
    let failed = tsk(&lost, &["backup", "add", url]);
    let text = format!("{}{}", stdout(&failed), stderr(&failed));
    assert_eq!(failed.status.code(), Some(1), "{text}");
    for args in [&["backup", "list"][..], &["backup"], &["restore"]] {
        let output = tsk(&lost, args);
        let text = format!("{text}{}{}", stdout(&output), stderr(&output));
        assert!(
            !text.contains("S3CRET"),
            "{args:?} printed the token: {text}"
        );
        assert!(text.contains(shown), "{args:?}: {text}");
    }
}

#[test]
fn a_usage_error_never_echoes_a_token() {
    let state = TempDir::new("usage-token");
    for args in [
        &[
            "backup",
            "add",
            "/srv/a.git",
            "https://me:S3CRET@host/b.git",
        ][..],
        &["backup", "now", "https://me:S3CRET@host/b.git"],
        &["restore", "deadbeef", "https://me:S3CRET@host/b.git"],
    ] {
        let output = tsk(&state.0, args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let text = stderr(&output);
        assert!(!text.contains("S3CRET"), "{args:?}: {text}");
        assert!(text.contains("https://***@host/b.git"), "{args:?}: {text}");
    }
    // A URL given where a version id goes is echoed back as the unknown id.
    let repo = bare_repo(&state.0);
    fs::write(
        state.0.join("config.toml"),
        format!("[[backup]]\nurl = {repo:?}\n"),
    )
    .expect("config");
    let output = tsk(&state.0, &["restore", "https://me:S3CRET@host/b.git"]);
    assert_eq!(output.status.code(), Some(1));
    let text = stderr(&output);
    assert!(!text.contains("S3CRET"), "{text}");
    assert!(
        text.contains("unknown version https://***@host/b.git"),
        "{text}"
    );
}

#[test]
fn an_id_restores_a_version_past_the_newest_twenty() {
    let root = TempDir::new("past-twenty");
    let repo = bare_repo(&root.0);
    let state = root.0.join("state");
    add(&state, "version 1");
    let added = tsk(&state, &["backup", "add", &repo, "--every", "daily"]);
    assert_eq!(added.status.code(), Some(0), "{}", stderr(&added));
    for n in 2..=21 {
        add(&state, &format!("version {n}"));
        let now = tsk(&state, &["backup"]);
        assert_eq!(now.status.code(), Some(0), "{}", stderr(&now));
    }
    let rows = |args: &[&str]| -> Vec<String> {
        let listed = tsk(&state, args);
        assert_eq!(listed.status.code(), Some(0), "{}", stderr(&listed));
        stdout(&listed)
            .lines()
            .skip(1)
            .filter(|line| !line.starts_with("Restore one"))
            .map(str::to_string)
            .collect()
    };
    assert_eq!(rows(&["restore"]).len(), 20);
    let every = rows(&["restore", "--all"]);
    assert_eq!(every.len(), 21);
    let oldest = every[20].split_whitespace().nth(1).expect("id").to_string();
    let restored = tsk(&state, &["restore", &oldest]);
    assert_eq!(restored.status.code(), Some(0), "{}", stderr(&restored));
    assert!(stdout(&restored).starts_with(&format!("restored {oldest} (1 tasks")));
}

#[test]
fn backup_add_checks_a_detached_run_reaches_the_target_too() {
    let state = TempDir::new("probe-ok");
    add(&state.0, "reachable");
    let repo = bare_repo(&state.0);
    let added = tsk(&state.0, &["backup", "add", &repo]);
    assert_eq!(added.status.code(), Some(0), "{}", stderr(&added));
    assert_eq!(stderr(&added), "", "the detached probe reached it");
    let leftovers: Vec<_> = fs::read_dir(&state.0)
        .expect("state")
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("backup.probe-")
        })
        .collect();
    assert!(leftovers.is_empty(), "the probe's answer is removed");
}

/// The foreground backup reaches the target over a fake ssh, but the detached run's
/// connection is refused (as when the board's environment has no ssh-agent): `add` says so.
#[cfg(unix)]
#[test]
fn backup_add_says_when_only_the_detached_run_cannot_reach_the_target() {
    use std::os::unix::fs::PermissionsExt;
    let state = TempDir::new("probe-fails");
    add(&state.0, "reachable only here");
    let repo = bare_repo(&state.0);
    let marker = state.0.join("pushed");
    let ssh = state.0.join("fake-ssh");
    // The last argument is the remote command; after the first push, refuse every fetch.
    fs::write(
        &ssh,
        format!(
            "#!/bin/sh\nfor last; do :; done\ncase \"$last\" in\n  git-upload-pack*) [ -f '{m}' ] && {{ echo 'Permission denied (publickey).' >&2; exit 255; }} ;;\n  git-receive-pack*) sh -c \"$last\" && touch '{m}'; exit $? ;;\nesac\nexec sh -c \"$last\"\n",
            m = marker.display()
        ),
    )
    .expect("fake ssh");
    fs::set_permissions(&ssh, fs::Permissions::from_mode(0o755)).expect("chmod");
    let added = Command::new(env!("CARGO_BIN_EXE_tsk"))
        .args(["backup", "add", &format!("ssh://fake{repo}"), "--state-dir"])
        .arg(&state.0)
        .env("GIT_SSH_COMMAND", &ssh)
        .env_remove("TSK_AGENT")
        .env("TSK_NO_UPDATE_CHECK", "1")
        .output()
        .expect("run tsk");
    assert_eq!(added.status.code(), Some(0), "{}", stderr(&added));
    assert!(marker.exists(), "the foreground backup pushed");
    assert!(
        stderr(&added).contains("a background backup (as a save starts one) could not reach"),
        "{}",
        stderr(&added)
    );
    assert!(
        stderr(&added).contains("Permission denied (publickey)."),
        "{}",
        stderr(&added)
    );
}

#[test]
fn backup_and_restore_refuse_without_targets() {
    let state = TempDir::new("none");
    let restore = tsk(&state.0, &["restore"]);
    assert_eq!(restore.status.code(), Some(1));
    assert!(stderr(&restore).contains("no backup targets"));
    let backup = tsk(&state.0, &["backup"]);
    assert_eq!(backup.status.code(), Some(1));
    let usage = tsk(&state.0, &["backup", "add", "/x.git", "--every", "hourly"]);
    assert_eq!(usage.status.code(), Some(2));
    // The `=` form is the same flag.
    let equals = tsk(&state.0, &["backup", "add", "/x.git", "--every=hourly"]);
    assert_eq!(equals.status.code(), Some(2));
    assert_eq!(stderr(&equals), stderr(&usage));
    assert!(stderr(&equals).contains("--every hourly is not save or daily"));
    let usage = tsk(&state.0, &["backup", "add", "/x.git", "--folder"]);
    assert_eq!(usage.status.code(), Some(2), "folder targets are gone");
    assert!(!state.0.join("config.toml").exists());
}

/// The detached backup must not depend on the user's global git setup: signing on, hooks that
/// fail everything, and no identity, all from an isolated global config.
#[cfg(unix)]
#[test]
fn a_backup_ignores_the_users_global_git_setup() {
    use std::os::unix::fs::PermissionsExt;
    let root = TempDir::new("hostile-global");
    let repo = bare_repo(&root.0);
    let state = root.0.join("state");
    add(&state, "pushed anyway");
    let home = root.0.join("home");
    let hooks = home.join("hooks");
    fs::create_dir_all(&hooks).expect("hooks");
    // Client-side hooks only. The receiving side of a local bare repository is the server: it
    // runs its own hooks, which are the server's business.
    for hook in ["pre-commit", "commit-msg", "post-commit", "pre-push"] {
        let path = hooks.join(hook);
        fs::write(&path, "#!/bin/sh\nexit 1\n").expect("hook");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let global = home.join("gitconfig");
    fs::write(
        &global,
        format!(
            "[commit]\n\tgpgsign = true\n[gpg]\n\tprogram = /usr/bin/false\n[core]\n\thooksPath = {}\n",
            hooks.display()
        ),
    )
    .expect("global config");
    let output = Command::new(env!("CARGO_BIN_EXE_tsk"))
        .args(["backup", "add", &repo, "--state-dir"])
        .arg(&state)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("GIT_CONFIG_GLOBAL", &global)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_AUTHOR_NAME")
        .env_remove("GIT_AUTHOR_EMAIL")
        .env_remove("GIT_COMMITTER_NAME")
        .env_remove("GIT_COMMITTER_EMAIL")
        .env("TSK_NO_UPDATE_CHECK", "1")
        .output()
        .expect("run tsk");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}{}",
        stdout(&output),
        stderr(&output)
    );
    assert_eq!(commits(&repo), 1);
}

/// `tsk backup setup` against a fake `gh` (never GitHub): `TSK_GH` points at a script whose
/// repositories are local bare repositories under its directory.
#[cfg(unix)]
mod setup {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const FAKE_GH: &str = r#"#!/bin/sh
D="$FAKE_GH_DIR"
name=$(printf '%s' "$3" | tr / _)
echo "${GH_HOST:-default} $*" >> "$D/calls"
case "$1 $2" in
  "auth status") [ -f "$D/logged-in" ] && exit 0
    echo "You are not logged into any GitHub hosts." >&2; exit 1 ;;
  "api user") echo me; exit 0 ;;
  "repo view") [ -f "$D/$name.json" ] && { cat "$D/$name.json"; exit 0; }
    echo "GraphQL: Could not resolve to a Repository with the name '$3'." >&2; exit 1 ;;
  "repo create") echo "$*" >> "$D/created"
    git init --quiet --bare "$D/$name.git" || exit 1
    printf '{"visibility":"PRIVATE","sshUrl":"%s","url":"https://example.invalid/%s"}' "$D/$name.git" "$3" > "$D/$name.json"
    exit 0 ;;
  "config get") echo "${FAKE_GH_PROTOCOL:-ssh}"; exit 0 ;;
esac
echo "fake gh: unexpected $*" >&2; exit 2
"#;

    struct Fake {
        root: TempDir,
        gh: PathBuf,
    }

    impl Fake {
        fn new(logged_in: bool) -> Self {
            let root = TempDir::new("gh");
            let gh = root.0.join("gh");
            fs::write(&gh, FAKE_GH).expect("fake gh");
            fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).expect("chmod");
            if logged_in {
                fs::write(root.0.join("logged-in"), "").expect("login");
            }
            Self { root, gh }
        }

        fn dir(&self) -> &Path {
            &self.root.0
        }

        /// A repository that already exists on the fake GitHub.
        fn existing(&self, full: &str, visibility: &str) {
            let name = full.replace('/', "_");
            let repo = self.dir().join(format!("{name}.git"));
            fs::create_dir_all(&repo).expect("repo");
            git(&repo, &["init", "--quiet", "--bare"]);
            fs::write(
                self.dir().join(format!("{name}.json")),
                format!(
                    r#"{{"visibility":"{visibility}","sshUrl":"{}","url":"https://example.invalid/{full}"}}"#,
                    repo.display()
                ),
            )
            .expect("repo json");
        }

        /// Every gh call, each line prefixed with the `GH_HOST` it saw.
        fn calls(&self) -> String {
            fs::read_to_string(self.dir().join("calls")).unwrap_or_default()
        }

        fn created(&self) -> String {
            fs::read_to_string(self.dir().join("created")).unwrap_or_default()
        }

        fn setup(&self, state: &Path, extra: &[&str]) -> Output {
            Command::new(env!("CARGO_BIN_EXE_tsk"))
                .args(["backup", "setup"])
                .args(extra)
                .arg("--state-dir")
                .arg(state)
                .env("TSK_GH", &self.gh)
                .env("FAKE_GH_DIR", self.dir())
                .env("TSK_NO_UPDATE_CHECK", "1")
                .output()
                .expect("run tsk")
        }
    }

    #[test]
    fn without_gh_it_says_how_to_get_it_and_writes_nothing() {
        let state = TempDir::new("no-gh");
        let output = Command::new(env!("CARGO_BIN_EXE_tsk"))
            .args(["backup", "setup", "--state-dir"])
            .arg(&state.0)
            .env("TSK_GH", state.0.join("missing-gh"))
            .output()
            .expect("run tsk");
        assert_eq!(output.status.code(), Some(1));
        assert!(
            stderr(&output).contains("https://cli.github.com"),
            "{}",
            stderr(&output)
        );
        assert!(
            stderr(&output).contains("tsk backup add <git-url>"),
            "{}",
            stderr(&output)
        );
        assert!(!state.0.join("config.toml").exists());
    }

    #[test]
    fn logged_out_gh_says_to_log_in_and_writes_nothing() {
        let fake = Fake::new(false);
        let state = TempDir::new("logged-out");
        let output = fake.setup(&state.0, &[]);
        assert_eq!(output.status.code(), Some(1));
        assert!(
            stderr(&output).contains("gh auth login"),
            "{}",
            stderr(&output)
        );
        // gh's own words come through, and tsk's two lines print as two lines.
        assert!(
            stderr(&output).contains("You are not logged into any GitHub hosts."),
            "{}",
            stderr(&output)
        );
        assert!(
            stderr(&output).contains("again.\nFor another git host"),
            "{}",
            stderr(&output)
        );
        assert!(!stderr(&output).contains("\\u{"), "{}", stderr(&output));
        assert!(!state.0.join("config.toml").exists());
        assert!(fake.created().is_empty());
    }

    #[test]
    fn it_creates_a_private_repository_backs_up_and_is_a_no_op_after() {
        let fake = Fake::new(true);
        let state = TempDir::new("create");
        add(&state.0, "kept safe");
        let output = fake.setup(&state.0, &[]);
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        assert!(
            stdout(&output).starts_with("created private repository me/tsk-backup\n"),
            "{}",
            stdout(&output)
        );
        assert!(
            stdout(&output).contains("backed up to"),
            "{}",
            stdout(&output)
        );
        assert_eq!(fake.created(), "repo create me/tsk-backup --private\n");
        let repo = fake.dir().join("me_tsk-backup.git");
        assert_eq!(commits(repo.to_str().expect("utf-8")), 1);

        let again = fake.setup(&state.0, &[]);
        assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
        assert!(
            stdout(&again).starts_with("already backing up to me/tsk-backup"),
            "{}",
            stdout(&again)
        );
        assert_eq!(fake.created().lines().count(), 1, "nothing created twice");

        // A new machine finds the other machine's versions and is pointed at restore.
        let fresh = TempDir::new("create-fresh");
        let output = fake.setup(&fresh.0, &[]);
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        assert!(stdout(&output).starts_with("using private repository me/tsk-backup"));
        assert!(
            stdout(&output).contains("run `tsk restore`"),
            "{}",
            stdout(&output)
        );
        assert!(
            !fresh.0.join("tsk.json").exists(),
            "never restores by itself"
        );
    }

    #[test]
    fn a_new_machine_without_a_state_dir_reuses_its_repository() {
        let fake = Fake::new(true);
        fake.existing("me/tsk-backup", "PRIVATE");
        let root = TempDir::new("new-laptop");
        let state = root.0.join("not-created-yet");
        let output = fake.setup(&state, &[]);
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        assert!(stdout(&output).starts_with("using private repository me/tsk-backup\n"));
        assert!(state.join("config.toml").exists());
    }

    #[test]
    fn it_reuses_an_existing_private_repository() {
        let fake = Fake::new(true);
        fake.existing("team/backups", "PRIVATE");
        let state = TempDir::new("reuse");
        let output = fake.setup(&state.0, &["--repo", "team/backups"]);
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        assert!(stdout(&output).starts_with("using private repository team/backups\n"));
        assert!(fake.created().is_empty());
        let listed = stdout(&tsk(&state.0, &["backup", "list"]));
        assert!(listed.contains("team_backups.git"), "{listed}");
    }

    #[test]
    fn it_uses_the_clone_url_of_the_users_git_protocol() {
        let fake = Fake::new(true);
        fake.existing("me/tsk-backup", "PRIVATE");
        let state = TempDir::new("https");
        let output = Command::new(env!("CARGO_BIN_EXE_tsk"))
            .args(["backup", "setup", "--state-dir"])
            .arg(&state.0)
            .env("TSK_GH", &fake.gh)
            .env("FAKE_GH_DIR", fake.dir())
            .env("FAKE_GH_PROTOCOL", "https")
            .env("TSK_NO_UPDATE_CHECK", "1")
            .output()
            .expect("run tsk");
        // gh's https URL for the fake host is the one tried (and cannot be reached).
        assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
        assert!(
            stderr(&output).contains("example.invalid/me/tsk-backup.git"),
            "{}",
            stderr(&output)
        );
    }

    #[test]
    fn it_refuses_a_private_repository_with_other_content() {
        let fake = Fake::new(true);
        fake.existing("me/notes", "PRIVATE");
        let work = TempDir::new("notes-work");
        git(&work.0, &["init", "--quiet"]);
        fs::write(work.0.join("README.md"), "mine").expect("readme");
        git(&work.0, &["add", "-A"]);
        git(
            &work.0,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "-m",
                "notes",
            ],
        );
        let repo = fake.dir().join("me_notes.git");
        git(
            &work.0,
            &[
                "push",
                "--quiet",
                repo.to_str().expect("utf-8"),
                "HEAD:refs/heads/main",
            ],
        );
        git(&repo, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        let state = TempDir::new("other-content");
        let output = fake.setup(&state.0, &["--repo", "notes"]);
        assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
        assert!(
            stderr(&output).contains("me/notes has other content"),
            "{}",
            stderr(&output)
        );
        assert!(!state.0.join("config.toml").exists());
    }

    #[test]
    fn it_refuses_a_public_repository_and_writes_nothing() {
        let fake = Fake::new(true);
        fake.existing("me/tsk-backup", "PUBLIC");
        let state = TempDir::new("public");
        let output = fake.setup(&state.0, &[]);
        assert_eq!(output.status.code(), Some(1));
        assert!(
            stderr(&output).contains("is not private"),
            "{}",
            stderr(&output)
        );
        assert!(!state.0.join("config.toml").exists());
    }

    #[test]
    fn a_named_host_reaches_every_gh_call() {
        let fake = Fake::new(true);
        fake.existing("team/backups", "PRIVATE");
        let state = TempDir::new("host");
        let output = fake.setup(&state.0, &["--repo", "ghe.example.com/team/backups"]);
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        assert!(
            stdout(&output).starts_with("using private repository ghe.example.com/team/backups\n"),
            "{}",
            stdout(&output)
        );
        let calls = fake.calls();
        assert!(!calls.is_empty());
        assert!(
            calls
                .lines()
                .all(|call| call.starts_with("ghe.example.com ")),
            "{calls}"
        );
    }

    #[test]
    fn an_internal_repository_is_used_with_a_note() {
        let fake = Fake::new(true);
        fake.existing("me/tsk-backup", "INTERNAL");
        let state = TempDir::new("internal");
        let output = fake.setup(&state.0, &[]);
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        assert!(
            stdout(&output).contains("me/tsk-backup is internal"),
            "{}",
            stdout(&output)
        );
        assert!(state.0.join("config.toml").exists());
    }
}

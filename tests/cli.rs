//! End-to-end: the binary against scratch trackers and git repos.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A scratch directory, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("deeds-cli-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir.canonicalize().unwrap())
    }

    fn path(&self, rel: &str) -> PathBuf {
        let path = self.0.join(rel);
        fs::create_dir_all(&path).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Hermetic git: no user config, a fixed identity.
fn hermetic(cmd: &mut Command) -> &mut Command {
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .env_remove("DEEDS_DIR")
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = hermetic(&mut Command::new("git"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", err(&out));
    String::from_utf8(out.stdout).unwrap()
}

fn subjects(dir: &Path) -> Vec<String> {
    git(dir, &["log", "--format=%s"])
        .lines()
        .map(String::from)
        .collect()
}

fn deeds_cmd(cwd: &Path, deeds_dir: Option<&Path>, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_deeds"));
    hermetic(&mut cmd).current_dir(cwd).args(args);
    if let Some(dir) = deeds_dir {
        cmd.env("DEEDS_DIR", dir);
    }
    cmd
}

/// Run deeds with `stdin` as its input (None: /dev/null), asserting success.
fn run(cwd: &Path, deeds_dir: Option<&Path>, args: &[&str], stdin: Option<&str>) -> Output {
    let out = try_run(cwd, deeds_dir, args, stdin);
    assert!(out.status.success(), "deeds {args:?}: {}", err(&out));
    out
}

fn try_run(cwd: &Path, deeds_dir: Option<&Path>, args: &[&str], stdin: Option<&str>) -> Output {
    let mut child = deeds_cmd(cwd, deeds_dir, args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pipe = child.stdin.take().unwrap();
    if let Some(text) = stdin {
        pipe.write_all(text.as_bytes()).unwrap();
    }
    drop(pipe);
    child.wait_with_output().unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// `deeds new`'s id.
fn new_id(o: &Output) -> String {
    out(o).split_whitespace().next().unwrap().to_string()
}

fn today() -> String {
    let o = Command::new("date").arg("+%F").output().unwrap();
    String::from_utf8(o.stdout).unwrap().trim().to_string()
}

/// A standalone tracker, initialized by `deeds init`.
fn standalone(s: &Scratch) -> PathBuf {
    let dir = s.0.join("home/deeds");
    run(&s.0, Some(&dir), &["init"], None);
    dir
}

#[test]
fn lookup_prefers_deeds_dir_and_says_so() {
    let s = Scratch::new();
    let project = s.path("project");
    let inline = s.path("project/.deeds");
    let sub = s.path("project/src");
    let other = s.path("other");

    let o = run(&sub, None, &["new", "a"], None);
    assert!(inline.join(format!("{}.md", new_id(&o))).is_file());

    let o = run(&sub, Some(&other), &["new", "b"], None);
    assert!(other.join(format!("{}.md", new_id(&o))).is_file());
    let notice = err(&o);
    assert!(
        notice.contains(&other.display().to_string())
            && notice.contains(&inline.display().to_string()),
        "{notice}"
    );
    assert_eq!(notice.lines().count(), 1);

    // No notice when nothing is shadowed, or DEEDS_DIR is that same .deeds/.
    assert_eq!(err(&run(&s.0, Some(&other), &["list"], None)), "");
    assert_eq!(err(&run(&project, Some(&inline), &["list"], None)), "");

    let o = try_run(&sub, Some(Path::new("rel/deeds")), &["list"], None);
    assert!(!o.status.success() && err(&o).contains("absolute"));
    let o = try_run(&sub, Some(&s.0.join("missing")), &["list"], None);
    assert!(!o.status.success() && err(&o).contains("deeds init"));
}

#[test]
fn any_deeds_dir_is_a_tracker() {
    let s = Scratch::new();
    let bare = s.path("bare");
    let id = new_id(&run(&s.0, Some(&bare), &["new", "t"], None));
    run(&s.0, Some(&bare), &["close", &id], None);
    assert!(bare.join(format!("closed/{id}.md")).is_file());
}

#[test]
fn init_with_deeds_dir_makes_a_repo_and_no_agents_block() {
    let s = Scratch::new();
    let dir = standalone(&s);
    assert!(dir.join(".git").is_dir());
    assert!(!s.0.join("AGENTS.md").exists() && !dir.join("AGENTS.md").exists());
    // Rerunnable.
    run(&s.0, Some(&dir), &["init"], None);
}

#[test]
fn autocommit_snapshots_then_commits_the_command() {
    let s = Scratch::new();
    let dir = standalone(&s);
    let a = new_id(&run(&s.0, Some(&dir), &["new", "first", "one"], None));
    assert_eq!(subjects(&dir), ["deeds new first one"]);

    // A direct edit is committed on its own before the next call's change.
    fs::write(dir.join(format!("{a}.md")), "# first one\n\nedited\n").unwrap();
    let b = new_id(&run(&s.0, Some(&dir), &["new", "second"], None));
    assert_eq!(
        subjects(&dir),
        [
            "deeds new second",
            &format!("snapshot: {a}"),
            "deeds new first one"
        ]
    );
    let snapshot_files = git(&dir, &["show", "--name-only", "--format=", "HEAD~1"]);
    assert_eq!(snapshot_files.trim(), format!("{a}.md"));
    let change_files = git(&dir, &["show", "--name-only", "--format=", "HEAD"]);
    assert_eq!(change_files.trim(), format!("{b}.md"));

    run(&s.0, Some(&dir), &["close", &b, "-m", "done it"], None);
    assert_eq!(subjects(&dir)[0], format!("deeds close {b} -m \"done it\""));
    assert_eq!(git(&dir, &["status", "--porcelain"]), "");
}

#[test]
fn inline_tracker_is_not_committed() {
    let s = Scratch::new();
    let project = s.path("project");
    git(&project, &["init", "-q"]);
    s.path("project/.deeds");
    git(&project, &["commit", "-q", "--allow-empty", "-m", "base"]);

    run(&project, None, &["new", "t"], None);
    run(&project, None, &["commit"], None);
    assert_eq!(subjects(&project), ["base"]);
    assert!(git(&project, &["status", "--porcelain"]).contains(".deeds/"));
}

#[test]
fn commit_is_just_the_snapshot() {
    let s = Scratch::new();
    let dir = standalone(&s);
    let o = run(&s.0, Some(&dir), &["commit"], None);
    assert_eq!(out(&o), "");
    let id = new_id(&run(&s.0, Some(&dir), &["new", "t"], None));
    run(&s.0, Some(&dir), &["commit"], None);
    assert_eq!(subjects(&dir), ["deeds new t"]);

    fs::write(dir.join(format!("{id}.md")), "# t\n\nedited\n").unwrap();
    let o = run(&s.0, Some(&dir), &["commit"], None);
    assert_eq!(out(&o).trim(), format!("snapshot: {id}"));
    assert_eq!(subjects(&dir).len(), 2);
}

#[test]
fn concurrent_calls_wait_for_the_lock() {
    let s = Scratch::new();
    let dir = standalone(&s);
    let lock = File::open(dir.join(".git")).unwrap();
    lock.lock().unwrap();

    let mut child = deeds_cmd(&s.0, Some(&dir), &["new", "t"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert!(child.try_wait().unwrap().is_none(), "ran while locked");
    drop(lock);
    assert!(child.wait().unwrap().success());
    assert_eq!(subjects(&dir), ["deeds new t"]);
}

#[test]
fn new_reads_its_body_from_stdin() {
    let s = Scratch::new();
    let dir = s.path("d");
    let body = "\nWhat and why.\n\n## For you\n\n- [ ] Decide.\n\n";
    let id = new_id(&run(&s.0, Some(&dir), &["new", "t"], Some(body)));
    let text = fs::read_to_string(dir.join(format!("{id}.md"))).unwrap();
    assert_eq!(
        text,
        "# t\n\nWhat and why.\n\n## For you\n\n- [ ] Decide.\n"
    );

    let id = new_id(&run(&s.0, Some(&dir), &["new", "u"], None));
    let text = fs::read_to_string(dir.join(format!("{id}.md"))).unwrap();
    assert_eq!(text, "# u\n");
}

#[test]
fn note_stamps_its_checkout_and_keeps_log_last() {
    let s = Scratch::new();
    let dir = s.path("tracker");
    let id = new_id(&run(&s.0, Some(&dir), &["new", "t"], Some("Body.\n")));
    let path = dir.join(format!("{id}.md"));
    let date = today();

    // Outside any repo: date only. Multi-line text indents under the first.
    run(&s.0, Some(&dir), &["note", &id], Some("one\n\nmore\n"));

    // A git main checkout, and a worktree of it.
    let proj = s.path("proj");
    git(&proj, &["init", "-q"]);
    git(&proj, &["commit", "-q", "--allow-empty", "-m", "base"]);
    git(&proj, &["worktree", "add", "-q", "../wt-dir"]);
    let wt = s.0.join("wt-dir");
    run(&s.path("proj/sub"), Some(&dir), &["note", &id], Some("two"));
    run(&wt, Some(&dir), &["note", &id], Some("three"));

    // A jj secondary workspace, its `.jj/repo` pointing at the main one.
    s.path("jjmain/.jj/repo");
    s.path("jjws/.jj");
    fs::write(s.0.join("jjws/.jj/repo"), "../../jjmain/.jj/repo").unwrap();
    run(&s.path("jjws"), Some(&dir), &["note", &id], Some("four"));

    // A section added after the log: the next note moves the log back last.
    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
    f.write_all(b"\n## For you\n\n- [ ] Look.\n").unwrap();
    run(&s.path("jjmain"), Some(&dir), &["note", &id], Some("five"));

    let expected = format!(
        "# t\n\nBody.\n\n## For you\n\n- [ ] Look.\n\n## Log\n\n\
         - {date}: one\n\n  more\n\
         - {date} proj: two\n\
         - {date} proj@wt-dir: three\n\
         - {date} jjmain@jjws: four\n\
         - {date} jjmain: five\n"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), expected);

    let o = try_run(&s.0, Some(&dir), &["note", &id], Some("  \n"));
    assert!(!o.status.success() && err(&o).contains("empty"));
}

#[test]
fn close_message_is_a_log_entry() {
    let s = Scratch::new();
    let dir = s.path("d");
    let id = new_id(&run(
        &s.0,
        Some(&dir),
        &["new", "t"],
        Some("## For you\n\n- [ ] x\n"),
    ));
    run(&s.0, Some(&dir), &["close", &id, "-m", "shipped"], None);
    let text = fs::read_to_string(dir.join(format!("closed/{id}.md"))).unwrap();
    assert!(
        text.ends_with(&format!(
            "- [ ] x\n\n## Log\n\n- {}: Closed: shipped\n",
            today()
        )),
        "{text}"
    );
}

#[test]
fn autocommit_ignores_an_inherited_git_repo() {
    let s = Scratch::new();
    let dir = standalone(&s);
    let other = s.path("other");
    git(&other, &["init", "-q"]);
    let o = deeds_cmd(&s.0, Some(&dir), &["new", "t"])
        .env("GIT_DIR", other.join(".git"))
        .env("GIT_OBJECT_DIRECTORY", other.join(".git/objects"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(subjects(&dir), ["deeds new t"]);
}

#[test]
fn skill_follows_the_mode() {
    let s = Scratch::new();
    let inline = out(&run(&s.path("p/.deeds"), None, &["skill"], None));
    assert!(inline.contains(".deeds/closed/") && !inline.contains("$DEEDS_DIR"));
    let text = out(&run(&s.0, Some(&s.0.join("not-yet")), &["skill"], None));
    assert!(text.contains("deeds commits the tracker itself") && !text.contains(".deeds/"));
}

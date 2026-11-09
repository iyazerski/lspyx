//! End-to-end checks of the CLI and its workspace daemon against the real ty.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// A Python project with its own HOME, so the daemon it starts is isolated from real ones.
struct Project {
    home: TempDir,
    root: TempDir,
}

impl Project {
    fn new(files: &[(&str, &str)]) -> Self {
        let project = Self {
            home: TempDir::new().expect("failed to create test home"),
            root: TempDir::new().expect("failed to create test project"),
        };
        fs::write(project.root.path().join("pyproject.toml"), "").unwrap();
        for (path, text) in files {
            project.write(path, text);
        }
        project
    }

    fn write(&self, path: &str, text: &str) {
        let path = self.root.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lspyx"));
        command
            .args(args)
            .current_dir(self.root.path())
            .env("HOME", self.home.path());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("failed to run lspyx")
    }

    /// Run lspyx and return its stdout, failing the test when the command fails.
    fn lspyx(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "lspyx {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn path(&self) -> &Path {
        self.root.path()
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = self.run(&["daemon", "stop"]);
    }
}

#[test]
fn daemon_sees_files_changed_after_it_started() {
    let project = Project::new(&[
        ("pkg/__init__.py", ""),
        ("pkg/a.py", "def helper():\n    return 1\n"),
        ("pkg/b.py", "from pkg.a import helper\n\nhelper()\n"),
    ]);
    // Open a.py in the daemon, so the next edit must replace its in-memory copy.
    project.lspyx(&["usages", "pkg/a.py:1:5"]);

    project.write(
        "pkg/a.py",
        "def helper():\n    return 1\n\n\ndef fresh():\n    return 2\n",
    );
    project.write("pkg/b.py", "from pkg.a import fresh\n\nfresh()\n");
    project.write("pkg/c.py", "def brand_new():\n    pass\n");

    let goto = project.lspyx(&["goto", "pkg/b.py:3:1"]);
    assert!(goto.contains("pkg/a.py:5:5"), "{goto}");
    let found = project.lspyx(&["find-symbol", "brand_new"]);
    assert!(found.starts_with("1 symbol"), "{found}");

    fs::remove_file(project.path().join("pkg/c.py")).unwrap();
    let found = project.lspyx(&["find-symbol", "brand_new"]);
    assert!(found.starts_with("no symbols found"), "{found}");
}

#[test]
fn request_after_ty_crash_runs_on_a_fresh_daemon() {
    let project = Project::new(&[("app.py", "def helper():\n    pass\n")]);
    project.lspyx(&["find-symbol", "helper"]);
    let pid = daemon_pid(&project);
    let killed = Command::new("pkill")
        .args(["-9", "-P", &pid])
        .status()
        .expect("failed to run pkill");
    assert!(killed.success(), "ty wasn't running under daemon {pid}");

    let found = project.lspyx(&["find-symbol", "helper"]);

    assert!(found.starts_with("1 symbol"), "{found}");
    assert_ne!(daemon_pid(&project), pid);
}

fn daemon_pid(project: &Project) -> String {
    let status = project.lspyx(&["daemon", "status"]);
    status
        .lines()
        .find_map(|line| line.strip_prefix("pid: "))
        .unwrap_or_else(|| panic!("no pid in {status}"))
        .to_string()
}

#[test]
fn missing_ty_fails_fast_with_the_daemon_error() {
    let project = Project::new(&[("app.py", "")]);
    let empty_path = TempDir::new().unwrap();
    let started = Instant::now();

    let output = project
        .command(&["find-symbol", "helper"])
        .env("PATH", empty_path.path())
        .env_remove("LSPYX_TY_PATH")
        .output()
        .expect("failed to run lspyx");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("unable to find ty"), "{stderr}");
    assert!(started.elapsed() < Duration::from_secs(4), "{stderr}");
}

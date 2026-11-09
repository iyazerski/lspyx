//! End-to-end checks of the CLI and its workspace daemon against the real ty.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

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

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_lspyx"))
            .args(args)
            .current_dir(self.root.path())
            .env("HOME", self.home.path())
            .output()
            .expect("failed to run lspyx")
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

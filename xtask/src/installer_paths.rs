//! Whether a change needs the installers built before it is pushed.
//!
//! CI builds the installers for a pull request only when it touches the paths
//! under `pull_request: paths:` in `.github/workflows/build-installers.yaml`.
//! The ladder reads the same list, so the two cannot drift: a change that makes
//! CI build the installers makes the ladder build them, and any other change
//! gets the cheaper compile checks.

use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Command;

const WORKFLOW: &str = ".github/workflows/build-installers.yaml";

/// The `pull_request` trigger's path patterns, as written in the workflow.
fn workflow_paths(workflow: &str) -> Result<Vec<String>> {
    let mut in_pull_request = false;
    let mut in_paths = false;
    let mut paths = Vec::new();
    for line in workflow.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 2 {
            in_pull_request = trimmed == "pull_request:";
            in_paths = false;
        } else if in_pull_request && indent == 4 {
            in_paths = trimmed == "paths:";
        } else if in_paths && indent >= 6 {
            if let Some(pattern) = trimmed.strip_prefix("- ") {
                paths.push(pattern.trim().to_string());
            }
        } else if indent < 2 && in_pull_request {
            break;
        }
    }
    if paths.is_empty() {
        bail!("no `pull_request: paths:` found in {WORKFLOW}");
    }
    Ok(paths)
}

/// Whether `file` matches a GitHub path pattern. The workflow uses only
/// `dir/**` and exact file names; anything else is refused rather than
/// guessed at.
fn matches(pattern: &str, file: &str) -> Result<bool> {
    if let Some(dir) = pattern.strip_suffix("/**") {
        if dir.contains('*') {
            bail!("{WORKFLOW}: path pattern `{pattern}` is not supported by xtask");
        }
        return Ok(file.starts_with(&format!("{dir}/")));
    }
    if pattern.contains('*') {
        bail!("{WORKFLOW}: path pattern `{pattern}` is not supported by xtask");
    }
    Ok(file == pattern)
}

fn touches(patterns: &[String], files: &[String]) -> Result<Option<String>> {
    for file in files {
        for pattern in patterns {
            if matches(pattern, file)? {
                return Ok(Some(file.clone()));
            }
        }
    }
    Ok(None)
}

fn git(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// Paths from `git ... -z` output: split on NUL, so a name with non-ASCII
/// characters comes back as written instead of C-quoted.
fn names(listing: &[u8]) -> Vec<String> {
    listing
        .split(|b| *b == 0)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect()
}

/// What `changed_files` could not do.
enum Unknown {
    NoMain,
    ListFailed,
}

/// The files this checkout changes against `main`: commits since the merge
/// base, uncommitted edits and new files. Renames are listed as a delete and
/// an add, so a file moved out of an installer path still counts.
fn changed_files(root: &Path) -> Result<Vec<String>, Unknown> {
    let base = ["origin/main", "main"]
        .iter()
        .find_map(|candidate| git(root, &["merge-base", candidate, "HEAD"]))
        .map(|out| String::from_utf8_lossy(&out).trim().to_string())
        .ok_or(Unknown::NoMain)?;
    let mut files = Vec::new();
    for args in [
        vec!["diff", "--name-only", "--no-renames", "-z", &base],
        vec!["ls-files", "--others", "--exclude-standard", "-z"],
    ] {
        files.extend(names(&git(root, &args).ok_or(Unknown::ListFailed)?));
    }
    Ok(files)
}

/// Why the installers are needed, or `None` when the change leaves them out.
/// When the changed files cannot be listed, they are always needed.
pub fn needed(root: &Path) -> Result<Option<String>> {
    let workflow =
        std::fs::read_to_string(root.join(WORKFLOW)).with_context(|| format!("read {WORKFLOW}"))?;
    let patterns = workflow_paths(&workflow)?;
    let files = match changed_files(root) {
        Ok(files) => files,
        Err(Unknown::NoMain) => return Ok(Some("no main to compare with".into())),
        Err(Unknown::ListFailed) => return Ok(Some("could not list the changed files".into())),
    };
    Ok(touches(&patterns, &files)?.map(|file| format!("{file} changed")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKFLOW_TEXT: &str = "\
name: x
on:
  workflow_dispatch:
    inputs:
      updates:
        required: true
  # A comment.
  pull_request:
    types: [opened]
    paths:
      - .github/**
      - crates/platform/**
      - Cargo.lock

permissions:
  contents: read
";

    #[test]
    fn reads_the_pull_request_paths() {
        assert_eq!(
            workflow_paths(WORKFLOW_TEXT).unwrap(),
            [".github/**", "crates/platform/**", "Cargo.lock"]
        );
    }

    #[test]
    fn the_real_workflow_lists_paths() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let text = std::fs::read_to_string(root.join(WORKFLOW)).unwrap();
        let paths = workflow_paths(&text).unwrap();
        assert!(paths.iter().any(|p| p == "xtask/**"));
        for p in &paths {
            matches(p, "x").unwrap();
        }
    }

    #[test]
    fn matches_directories_and_files() {
        let paths = workflow_paths(WORKFLOW_TEXT).unwrap();
        let hit = |f: &str| touches(&paths, &[f.to_string()]).unwrap().is_some();
        assert!(hit("crates/platform/src/lib.rs"));
        assert!(hit("Cargo.lock"));
        assert!(hit(".github/workflows/check.yaml"));
        assert!(!hit("crates/app/src/route.rs"));
        assert!(!hit("crates/platform-extra/x.rs"));
        assert!(!hit("crates/Cargo.lock"));
    }

    #[test]
    fn splits_git_z_output_without_quoting() {
        assert_eq!(
            names("docs/note.txt\0packaging/ä.txt\0".as_bytes()),
            ["docs/note.txt", "packaging/ä.txt"]
        );
        assert!(names(b"").is_empty());
    }

    /// A file moved out of `packaging/`, one edited under a non-ASCII name and
    /// a new untracked file all count as changes, against a real repository.
    #[test]
    fn lists_renames_untracked_and_non_ascii_files() {
        let root =
            std::env::temp_dir().join(format!("ircord-installer-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("packaging")).unwrap();
        let git = |args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(&root)
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "init.defaultBranch=main",
                ])
                .args(args)
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q"]);
        std::fs::write(root.join("packaging/note.txt"), "a note\n".repeat(20)).unwrap();
        std::fs::write(root.join("packaging/ä.txt"), "one").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "base"]);
        std::fs::create_dir_all(root.join("docs")).unwrap();
        git(&["mv", "packaging/note.txt", "docs/note.txt"]);
        std::fs::write(root.join("packaging/ä.txt"), "two").unwrap();
        std::fs::write(root.join("new.txt"), "new").unwrap();
        let files = match changed_files(&root) {
            Ok(files) => files,
            Err(_) => panic!("the repository has a main"),
        };
        let _ = std::fs::remove_dir_all(&root);
        for expected in [
            "docs/note.txt",
            "packaging/note.txt",
            "packaging/ä.txt",
            "new.txt",
        ] {
            assert!(
                files.iter().any(|f| f == expected),
                "{expected} in {files:?}"
            );
        }
    }

    #[test]
    fn refuses_patterns_it_cannot_read() {
        assert!(matches("crates/*/src/**", "crates/a/src/x.rs").is_err());
        assert!(matches("**/*.rs", "a.rs").is_err());
    }
}

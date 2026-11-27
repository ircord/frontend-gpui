//! `cargo xtask credits`: write the people who have contributed to this
//! client into `crates/client/src/ircoid/contributors.txt`, which Ircoid's
//! credits screen compiles in. Run while preparing a release (README,
//! "Publishing a release"); `--check` fails when the file is out of date.
//!
//! A person is the author of a commit reachable from `HEAD`, or named in a
//! `Co-authored-by` trailer of one, after `.mailmap`; committers are not
//! counted. Names equal but for case and surrounding spaces are one person.
//! Bots and the names in [`EXCLUDED`] are left out. The order is each
//! person's first commit, oldest first.

use anyhow::{Context, Result, bail};
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

/// The file the client compiles in.
pub const FILE: &str = "crates/client/src/ircoid/contributors.txt";

/// Names that are not people, or not contributors.
const EXCLUDED: &[&str] = &["Claude", "GitHub", "dependabot"];

const HEADER: &str = "\
# People who have contributed to IRCord's GPUI client, in the order of their first commit.
# Written by `cargo xtask credits` while preparing a release (README, \"Publishing a release\").
# Do not edit by hand: a second spelling of someone belongs in .mailmap.
";

/// The people in `lines`, each a `Name <address>` or a bare name in history
/// order, by the rule in the module docs.
pub fn people(lines: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in lines.lines() {
        let name = match line.find('<') {
            Some(at) => &line[..at],
            None => line,
        };
        let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
        if name.is_empty()
            || name.to_lowercase().ends_with("[bot]")
            || EXCLUDED.iter().any(|e| e.eq_ignore_ascii_case(&name))
        {
            continue;
        }
        if !out
            .iter()
            .any(|known| known.to_lowercase() == name.to_lowercase())
        {
            out.push(name);
        }
    }
    out
}

/// The file for `names`.
pub fn render(names: &[String]) -> String {
    let mut out = HEADER.to_string();
    for name in names {
        out.push_str(name);
        out.push('\n');
    }
    out
}

/// The names in a file [`render`] wrote, as the client reads it.
#[cfg(test)]
fn parse(file: &str) -> Vec<String> {
    file.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .context("run git")?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8(out.stdout)?)
}

/// `Name <address>` lines through `.mailmap`, in order.
fn mailmap(root: &Path, lines: &[&str]) -> Result<Vec<String>> {
    if lines.is_empty() {
        return Ok(Vec::new());
    }
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["check-mailmap", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("run git check-mailmap")?;
    let mut input = child.stdin.take().context("git's stdin")?;
    let text = lines.join("\n") + "\n";
    let writer = std::thread::spawn(move || input.write_all(text.as_bytes()));
    let out = child.wait_with_output()?;
    writer
        .join()
        .map_err(|_| anyhow::anyhow!("writing to git"))??;
    if !out.status.success() {
        bail!("git check-mailmap failed");
    }
    Ok(String::from_utf8(out.stdout)?
        .lines()
        .map(str::to_string)
        .collect())
}

/// The history's people, oldest first.
fn history(root: &Path) -> Result<Vec<String>> {
    if git(root, &["rev-parse", "--is-shallow-repository"])?.trim() == "true" {
        bail!("this is a shallow clone, so the list would be short: fetch the whole history first");
    }
    let log = git(
        root,
        &[
            "log",
            "--reverse",
            "--format=%aN <%aE>%n%(trailers:key=Co-authored-by,valueonly,unfold)",
            "HEAD",
        ],
    )?;
    let lines: Vec<&str> = log
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    // Only `Name <address>` goes through the mailmap: given a bare name it
    // answers with the name as an address.
    let addressed: Vec<&str> = lines.iter().copied().filter(|l| l.contains('<')).collect();
    let mut mapped = mailmap(root, &addressed)?.into_iter();
    let mut ordered = Vec::new();
    for line in lines {
        if line.contains('<') {
            ordered.push(
                mapped
                    .next()
                    .context("git check-mailmap gave fewer lines")?,
            );
        } else {
            ordered.push(line.to_string());
        }
    }
    Ok(people(&ordered.join("\n")))
}

pub fn run(root: &Path, check: bool) -> Result<()> {
    let path = root.join(FILE);
    let names = history(root)?;
    let wanted = render(&names);
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    if check {
        if old != wanted {
            bail!("{FILE} is out of date: run `cargo xtask credits`");
        }
        println!("{FILE} lists the {} contributors", names.len());
        return Ok(());
    }
    if old != wanted {
        std::fs::write(&path, &wanted).with_context(|| format!("write {FILE}"))?;
    }
    let before: Vec<String> = old
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| l.trim().to_lowercase())
        .collect();
    for name in &names {
        if !before.contains(&name.to_lowercase()) {
            let hint = if name.contains(' ') {
                ""
            } else {
                " (one word: is this a handle of someone already listed? Then add a .mailmap line)"
            };
            println!("new: {name}{hint}");
        }
    }
    println!("{FILE}: {} contributors", names.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_person_with_two_addresses_is_listed_once() {
        let lines = "Ada Lovelace <>\nAda Lovelace <ada@example.org>\nada lovelace <a@b.c>";
        assert_eq!(people(lines), ["Ada Lovelace"]);
    }

    #[test]
    fn the_order_is_first_commit_and_co_authors_count() {
        let lines = "Grace Hopper <g@x>\nAlan Turing <a@x>\nGrace Hopper <g@x>\nKatherine Johnson";
        assert_eq!(
            people(lines),
            ["Grace Hopper", "Alan Turing", "Katherine Johnson"],
            "a bare-name trailer is kept"
        );
    }

    #[test]
    fn bots_and_tools_are_left_out() {
        let lines = "dependabot[bot] <49699333+dependabot[bot]@users.noreply.github.com>\n\
                     Claude <noreply@anthropic.com>\nGitHub <noreply@github.com>\n\
                     Radia Perlman <r@x>";
        assert_eq!(people(lines), ["Radia Perlman"]);
    }

    #[test]
    fn the_file_reads_back() {
        let names = vec!["Ada Lovelace".to_string(), "Alan Turing".to_string()];
        let file = render(&names);
        assert!(file.starts_with("# People"));
        assert_eq!(parse(&file), names);
    }
}

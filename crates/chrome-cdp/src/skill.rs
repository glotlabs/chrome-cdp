//! `chrome-cdp skill` (plan 08, milestone 8.3): the file that teaches an
//! agent the tool, in three forms.
//!
//! - `SKILL.md` for Claude Code and other Agent Skills clients: frontmatter
//!   (name, description) and the workflow, pitfalls and a cheat sheet.
//!   `--install` writes it to `.claude/skills/chrome-cdp/SKILL.md` in the
//!   project (the nearest directory up with `.git` or `.claude`, else the
//!   current one) or, with `--user`, in `$CLAUDE_CONFIG_DIR` or `~/.claude`.
//! - An `AGENTS.md` section, for agents that read that file.
//! - `llms.txt`: a summary, the help topics and every command by task,
//!   generated from the clap definitions so it cannot drift.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use protocol::schema::{SkillData, SkillWrite};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};

use crate::cli::{SkillArgs, SkillFormat};
use crate::help;

/// `SKILL.md`.
pub(crate) const SKILL: &str = include_str!("help/SKILL.md");

/// The `AGENTS.md` section.
const AGENTS: &str = include_str!("help/AGENTS.md");

pub(crate) fn run(args: &SkillArgs) -> Result<Output, ErrorInfo> {
    if args.install {
        return install(args.user);
    }
    let text = match args.format {
        SkillFormat::Skill => SKILL.to_owned(),
        SkillFormat::AgentsMd => AGENTS.to_owned(),
        SkillFormat::LlmsTxt => llms_txt(),
    };
    let data = SkillData {
        format: args.format.name().to_owned(),
        text: Some(text.clone()),
        path: None,
        write: None,
    };
    let shown = text.trim_end().to_owned();
    Ok(Output::typed(&data, shown.clone()).quiet(shown))
}

/// Writes `SKILL.md` where Claude Code finds it.
fn install(user: bool) -> Result<Output, ErrorInfo> {
    let dir = if user {
        user_dir()?
    } else {
        project_dir()?.join(".claude")
    };
    let path = dir.join("skills").join("chrome-cdp").join("SKILL.md");
    let shown = crate::input::absolute_path(&path)?;
    let write = match std::fs::read_to_string(&path) {
        Ok(old) if old == SKILL => SkillWrite::Unchanged,
        Ok(_) => SkillWrite::Updated,
        Err(_) => SkillWrite::Created,
    };
    if write != SkillWrite::Unchanged {
        let failed = |e: std::io::Error| {
            ErrorInfo::new(ErrorKind::BadArgs, format!("cannot write {shown}: {e}"))
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(failed)?;
        }
        std::fs::write(&path, SKILL).map_err(failed)?;
    }
    let text = match write {
        SkillWrite::Created => format!("installed {shown}"),
        SkillWrite::Updated => format!("updated {shown}"),
        SkillWrite::Unchanged => format!("{shown} is up to date"),
    };
    let data = SkillData {
        format: SkillFormat::Skill.name().to_owned(),
        text: None,
        path: Some(shown.clone()),
        write: Some(write),
    };
    Ok(Output::typed(&data, text).quiet(shown))
}

/// The nearest directory up from the current one that has `.git` or
/// `.claude`, else the current one.
fn project_dir() -> Result<PathBuf, ErrorInfo> {
    let cwd = std::env::current_dir().map_err(|e| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot read the current directory: {e}"),
        )
    })?;
    Ok(project_root(&cwd).unwrap_or(cwd))
}

fn project_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(".git").exists() || dir.join(".claude").is_dir())
        .map(Path::to_path_buf)
}

/// Claude Code's user directory: `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
fn user_dir() -> Result<PathBuf, ErrorInfo> {
    let set = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    if let Some(dir) = set("CLAUDE_CONFIG_DIR") {
        return Ok(PathBuf::from(dir));
    }
    set("HOME")
        .map(|home| PathBuf::from(home).join(".claude"))
        .ok_or_else(|| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                "HOME is not set, so there is no user directory to install to",
            )
            .hint(Hint::run(Cmd::new("skill").switch("install")).why("installs into the project"))
        })
}

/// `llms.txt` (llmstxt.org): a title, a summary, then sections. A CLI
/// has no pages to link, so the entries are commands to run.
fn llms_txt() -> String {
    let mut out = String::from(
        "# chrome-cdp\n\n\
         > A single-binary CLI that drives Chrome over the Chrome DevTools \
         Protocol, built for AI agents: one shell command per step, compact \
         output, refs (e12) from an accessibility snapshot instead of \
         selectors, actions that wait by themselves, and errors that name \
         the next command to run.\n\n\
         Every command takes `--json` (one JSON object on stdout) and `-q` \
         (only the result); `chrome-cdp <command> --help` has examples.\n\n\
         ## Guides\n\n\
         - `chrome-cdp skill`: the agent workflow, pitfalls and a cheat sheet\n\
         - `chrome-cdp help`: the overview and the locator grammar\n",
    );
    for topic in help::TOPICS {
        let _ = writeln!(out, "- `chrome-cdp help {}`: {}", topic.name, topic.summary);
    }
    for (group, commands) in help::grouped() {
        let _ = write!(out, "\n## {group}\n\n");
        for (name, about) in commands {
            let _ = writeln!(out, "- `chrome-cdp {name}`: {about}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{AGENTS, SKILL, llms_txt, project_root};

    /// About 4 characters make a token of English prose.
    const TOKENS_PER_CHAR: usize = 4;

    #[test]
    fn the_skill_has_frontmatter_and_stays_under_2k_tokens() {
        let rest = SKILL.strip_prefix("---\n").unwrap_or_default();
        let (front, body) = rest.split_once("\n---\n").unwrap_or_default();
        assert!(front.contains("name: chrome-cdp\n"), "{front}");
        let description = front
            .lines()
            .find_map(|l| l.strip_prefix("description: "))
            .unwrap_or_default();
        // Agent Skills: at most 1024 characters, and no angle brackets.
        assert!((50..=1024).contains(&description.len()), "{description}");
        assert!(!description.contains(['<', '>']));
        assert!(body.contains("snapshot -i"));
        assert!(
            SKILL.len() / TOKENS_PER_CHAR < 2000,
            "SKILL.md is about {} tokens",
            SKILL.len() / TOKENS_PER_CHAR
        );
        assert!(AGENTS.len() / TOKENS_PER_CHAR < 400);
    }

    /// Every `chrome-cdp <word>` the skill shows runs a real command.
    #[test]
    fn the_skill_names_real_commands() {
        use clap::CommandFactory as _;
        let root = crate::cli::Cli::command();
        // The cheat sheet's code block.
        let cheat = SKILL
            .split("## Cheat sheet\n\n```\n")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .unwrap_or_default();
        let mut words: Vec<&str> = [SKILL, AGENTS]
            .iter()
            .flat_map(|text| text.split("chrome-cdp ").skip(1))
            .filter_map(|rest| rest.split_whitespace().next())
            .map(|w| w.trim_end_matches(['`', ',', '.', ';', ')']))
            .filter(|w| !w.is_empty() && !w.starts_with('<'))
            .collect();
        // Cheat sheet lines start with commands, some joined by `|`.
        for line in cheat.lines() {
            let spec = line.split("  ").next().unwrap_or_default();
            for part in spec.split(" | ") {
                if let Some(word) = part.split_whitespace().next() {
                    words.push(word);
                }
            }
        }
        assert!(words.len() > 30, "{words:?}");
        for word in words {
            if word.starts_with('-') || word.contains(['\'', '/', '[', ':']) {
                continue;
            }
            assert!(root.find_subcommand(word).is_some(), "no command `{word}`");
        }
    }

    #[test]
    fn llms_txt_lists_the_topics_and_every_command() {
        let text = llms_txt();
        assert!(text.starts_with("# chrome-cdp\n\n> "));
        assert!(text.contains("- `chrome-cdp help locators`: "));
        assert!(text.contains("\n## Acting\n\n- `chrome-cdp click`: Click an element"));
    }

    #[test]
    fn the_project_is_the_nearest_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let deep = repo.join("src").join("app");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir(repo.join(".git")).unwrap();
        assert_eq!(project_root(&deep).as_deref(), Some(repo.as_path()));
        assert_eq!(
            project_root(Path::new(&repo)).as_deref(),
            Some(repo.as_path())
        );
    }
}

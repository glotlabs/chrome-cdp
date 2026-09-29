<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp skill`

Print a SKILL.md that teaches an agent to use chrome-cdp, or install it

```text
chrome-cdp skill [OPTIONS]
```

Options:

- `--format <FORMAT>`: What to print: the SKILL.md, an AGENTS.md section, or an llms.txt (one of `skill`, `agents-md`, `llms-txt`; default `skill`)
- `--install`: Write SKILL.md where Claude Code finds it (see --user) instead of printing it
- `--project`: With --install: into this project's .claude/skills (the default)
- `--user`: With --install: into ~/.claude/skills, for every project

```text
The skill is an Agent Skills file (Claude Code and others): frontmatter
with a name and description, then the workflow (open, snapshot -i, act by
ref, read the report, errors and console when stuck), the common pitfalls
and a cheat sheet, in under 2k tokens.

--install writes it to .claude/skills/chrome-cdp/SKILL.md in the project
(the nearest directory up with .git or .claude, else this one); with
--user to $CLAUDE_CONFIG_DIR/skills/… or ~/.claude/skills/…. Running it
again updates the file. --format agents-md prints a section for AGENTS.md
and --format llms-txt an llms.txt with every command.

Examples:
  chrome-cdp skill                          # print SKILL.md
  chrome-cdp skill --install                # into this project's .claude/skills
  chrome-cdp skill --install --user         # for every project
  chrome-cdp skill --format agents-md >> AGENTS.md
  chrome-cdp skill --format llms-txt > llms.txt
```


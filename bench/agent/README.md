# Agent benchmark tasks

The tasks of the agent benchmark (plan 10.4; docs/benchmarks.md says how
to run it). Each is a dir `tasks/<id>/` with a `task.toml` and, if the
task needs them, `files/`: copied into the agent's working dir.

```sh
cargo xtask bench agent -- --list       # the tasks
cargo xtask bench agent -- --check      # test every task without an agent
```

## task.toml

```toml
title = "Log in"
prompt = """
Sign in at {site}/bench/login.html as ada@example.com …
"""
arms = ["chrome-cdp"]    # optional: both arms by default
network = true           # optional: on a public site; --offline skips it
timeout_secs = 600       # optional: the run fails after this long

[setup]                  # optional, applied in this order:
copy = [{ from = "examples/e2e-shop", to = ".", exclude = ["node_modules"] }]
#                          then files/ is copied over it
remove = ["e2e/vars"]
replace = [{ file = "dist/assets/index-*.js", from = "…", to = "…" }]
serve = "app"            # serve ./app on a local dev server as {dev}

[verify]                 # every check given must pass
answer = ["7F3K-92"]     # in the reply's last `ANSWER:` line
page = """…"""           # a chrome-cdp batch script, in a fresh session
unchanged = ["e2e/scenarios"]

[[verify.record]]        # the page posted this to /bench/record
form = "login"
fields = { email = "ada@example.com", remember = true }

[[verify.test]]          # chrome-cdp test --json <args> passes
args = ["--backend", "local"]
min_passed = 12

[solution]               # what --check applies; the verifier must pass after it
answer = "…"
batch = """…"""          # a chrome-cdp batch script
continue_on_error = true # batch --continue, for a step meant to stop
copy = […]
replace = […]
```

Placeholders in the prompt, `replace`, `page`, the solution's `batch` and
the test args: `{site}` and `{site_b}` (the fixture site's two origins,
`127.0.0.1` and `localhost`), `{dev}` (the `serve`d dir), `{workdir}`,
`{port}` and `{port2}` (free ports), `{fixtures}` (the fixtures binary,
which also serves e2e-shop's real backend: `{fixtures} --shop dist`).

## Writing a task

- Check the outcome, not the agent's account of it. A page on the fixture
  site (`testdata/site/bench/`) posts what the user did with `record({…})`
  from `record.js`; a fixed app is checked by a `page` script; a test suite
  by `test`. Use `answer` only for questions, and have the prompt ask for an
  `ANSWER:` line.
- Say what to do, not how: no locators, no tool names. The arm's preamble
  says which browser tool the agent has.
- Give every task a `[solution]`, and run `--check`: the verifier must fail
  on the untouched task and pass after the solution.
- Keep answers hard to read from the page's source when the task is about
  the browser (the voucher, the popup's code and the beta code are built by
  script).

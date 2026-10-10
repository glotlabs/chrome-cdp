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
network = true           # optional: on a public site; --offline skips it
timeout_secs = 600       # optional: the run fails after this long

[setup]                  # optional, applied in this order:
copy = [{ from = "examples/e2e-shop", to = ".", exclude = ["node_modules"] }]
#                          then files/ is copied over it
link = [{ from = "testdata/apps-src/vue/node_modules", to = "app/node_modules",
          hint = "run `cargo xtask apps vue`" }]   # symlinks, after files/
remove = ["e2e/vars"]
replace = [{ file = "dist/assets/index-*.js", from = "…", to = "…" }]
serve = "app"            # serve ./app on a local dev server as {dev}
# or a real dev server as {dev} (http://127.0.0.1:{port}): a shell command
# run in the working dir, which must listen on {port}; killed afterwards
dev = "cd app && exec node node_modules/vite/bin/vite.js --port {port} --strictPort"

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
`127.0.0.1` and `localhost`), `{dev}` (the `serve`d dir or the `dev`
server), `{workdir}`,
`{port}` and `{port2}` (free ports), `{fixtures}` (the fixtures binary,
which also serves e2e-shop's real backend: `{fixtures} --shop dist`).

## Writing a task

- Check the outcome, not the agent's account of it. A page on the fixture
  site (`testdata/site/bench/`) posts what the user did with `record({…})`
  from `record.js`; a fixed app is checked by a `page` script; a test suite
  by `test`. Use `answer` only for questions, and have the prompt ask for an
  `ANSWER:` line.
- Say what to do, not how: no locators, no tool names. The preamble
  tells the agent it has `chrome-cdp`.
- Give every task a `[solution]`, and run `--check`: the verifier must fail
  on the untouched task and pass after the solution.
- The `find-code-*` tasks run a fixture app of `testdata/apps-src` on
  Vite's dev server, with the `node_modules` that `cargo xtask apps`
  installed (linked, not copied), and plant a bug in a component file the
  page's text does not name. Their `files/app/vite.config.js` is a plain
  config in place of the fixtures' shared build.
- Keep answers hard to read from the page's source when the task is about
  the browser (the voucher, the popup's code and the beta code are built by
  script).

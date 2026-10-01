<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp repro`

Pack what the session did into a bundle that replays it offline

```text
chrome-cdp repro [OPTIONS] <COMMAND>
```

```text
A repro bundle is a small project that replays a bug found in a session,
with no server: `repro start` marks the start (it keeps the cookies and
storage, the time, and from then on the response bodies of the session's
requests, documents and scripts included); reproduce the bug; `repro save
DIR` writes the bundle. `chrome-cdp test DIR` replays it from its own mocks
and fails the way the session did; once the bug is fixed it passes, and it
is a regression test. docs/e2e/repro.md has the layout.

Examples:
  chrome-cdp repro start
  chrome-cdp click 'role=button[name="Pay"]' && chrome-cdp expect role=status text Paid
  chrome-cdp repro save bugs/pay-twice --name "pays twice"
  chrome-cdp test bugs/pay-twice
```

## `chrome-cdp repro start`

Mark where the repro starts, and keep the requests' bodies from now

```text
chrome-cdp repro start [OPTIONS]
```

Options:

- `--scope <GLOB>`: Keep only the bodies of these requests: a path glob (`/api/**`) or a full-URL glob (`https://api.example.com/**`); repeatable

```text
Keeps the session's cookies and storage (as `state save` does), the time
(the bundle's test freezes it there), the page's viewport, locale and time
zone, and the history position. From now on each request's response body
is kept as it finishes loading (Chrome drops a page's bodies once it
navigates away); --scope keeps only some (path or full-URL globs, as a
mock set's scope). The page open now is where the bundle's test starts.
Starting again starts over. Needs a running session.

Examples:
  chrome-cdp repro start
  chrome-cdp repro start --scope '/api/**' --scope '/app/**'
```

## `chrome-cdp repro save`

Write the bundle: a test, its mocks, the state and the evidence

```text
chrome-cdp repro save [OPTIONS] <DIR>
```

Arguments:

- `<DIR>`: The bundle's directory: new or empty, unless --force

Options:

- `--name <NAME>`: The test's name (`@test "NAME"`); the directory's name if not given
- `--max-body <SIZE>`: Bodies bigger than this become placeholders (images and fonts: a fifth of it): `5MB`, `200KB`, or bytes (default `5MB`)
- `--force`: Write into a directory that has files (the bundle's own are replaced)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `4000`)

```text
DIR gets chrome-cdp.toml (a project whose every request is answered by
mocks/repro.toml; one no mock answers fails), repro.cdp (the commands since
`repro start` as a test, as `history --as-scenario` writes them, ending
with the last command that failed, its error in a comment), mocks/repro.toml and mocks/data/ (every request recorded),
state.json (the cookies and storage at the start), evidence/ (the last
snapshot, a screenshot, console, errors and requests) and README.md (what
happened, the last error, versions, how to replay). Bodies over --max-body
(5MB), and images and fonts over a fifth of it, become placeholders; the
README lists them. Passwords typed into password fields become
${secret:PASSWORD}; ${secret:…} values stay placeholders, and the README
names the secrets a replay needs. Without `repro start` it uses the whole
history, an empty state and the network log as it is, and warns. DIR must
be empty or new, unless --force.

Examples:
  chrome-cdp repro save bugs/cart-total
  chrome-cdp repro save bugs/cart-total --name "cart total ignores the coupon"
  chrome-cdp test bugs/cart-total
```


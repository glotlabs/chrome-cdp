# Recipes: common app flows, mocked

Each recipe is a mock set and a scenario that uses it. The first line of
each block names its file; a test copies them into a project and runs
`chrome-cdp fmt --check` and `chrome-cdp lint` on it, so they stay valid.
[mocks.md](mocks.md) has every key of a mock file, and
[`examples/e2e-shop`](../../examples/e2e-shop) runs several of these
against a real app.

- [Sign in once, or skip the login page](#authentication)
- [Pagination](#pagination)
- [File upload](#file-upload)
- [Optimistic UI](#optimistic-ui)
- [Polling](#polling)
- [Feature flags](#feature-flags)
- [Error states](#error-states)

## Authentication

Named state (`[state]`) makes the mocks remember the login: `sign in`
sets it, `me` reads it. The API answers `/api/me` with 200 either way,
since a 401 on every page load would be a console error in every test.

```toml
# e2e/mocks/auth.toml
[settings]
scope = ["/api/**"]

[state]
signed_in = false
user = ""

[[mock]]
name = "sign in"
match = { method = "POST", path = "/api/login", json = { email = "*", password = "secret" } }
respond = { json = { email = "{{json.email}}" } }
set_state = { signed_in = true, user = "{{json.email}}" }

[[mock]]
name = "wrong password"
match = { method = "POST", path = "/api/login" }
respond = { status = 401, json = { error = "Wrong email or password" } }

[[mock]]
name = "me: signed in"
match = { method = "GET", path = "/api/me" }
respond = { json = { signed_in = true, email = "{{state.user}}" } }
when = { signed_in = true }

[[mock]]
name = "me: signed out"
match = { method = "GET", path = "/api/me" }
respond = { json = { signed_in = false } }

[[mock]]
name = "sign out"
match = { method = "POST", path = "/api/logout" }
respond = { json = { ok = true } }
set_state = { signed_in = false, user = "" }
```

Most tests do not need to go through the login page. A set that only
starts the state signed in skips it; layer it where a test needs a user:

```toml
# e2e/mocks/signed-in.toml
[state]
signed_in = true
user = "ada@example.com"
```

```cdp
# e2e/scenarios/auth.cdp
@mocks auth

@test "signs in"
  goto /login
  fill label=Email ada@example.com
  fill label=Password secret
  click 'role=button[name="Sign in"]'
  expect 'text=Signed in as ada@example.com' visible
  expect request POST /api/login --json-body '{"email":"ada@example.com"}'

@test "a signed-in user sees the account page"
  @mocks signed-in
  goto /account
  expect 'role=heading[name="Account"]' visible
```

With a real backend (or an app that keeps a token in `localStorage`),
sign in once per file and hand the result to each test in a file:
`state save` in `@before-all`, `state load` in `@before`. Token auth can
be checked in the match: `headers = { authorization = "Bearer *" }`.

## Pagination

A collection pages its list itself: `?page=2&limit=10`, with the items
wrapped as `{ "items": [...], "total": N }` and an `X-Total-Count`
header. Seed it with more items than a page holds.

```toml
# e2e/mocks/products.toml
[settings]
scope = ["/api/**"]

[[collection]]
name = "products"
path = "/api/products"
seed = "data/products-50.json"
list = { page_param = "page", page_size_param = "limit", page_size = 10, wrap = "items" }
```

```cdp
# e2e/scenarios/pagination.cdp
@mocks products

@test "shows the second page"
  goto /products
  expect 'role=listitem' count 10
  click 'role=button[name="Next page"]'
  expect request GET /api/products --query page=2
  expect 'text=Page 2 of 5' visible
```

For hand-made pages (a cursor API, say), match the query instead:
`match = { method = "GET", path = "/api/feed", query = { cursor = "abc*" } }`,
a mock per page, the last page first when globs overlap.

## Nested resources

A collection whose path has parameters keeps each parent's items apart:
`GET /api/users/7/todos` lists the items whose `user` field is 7 (as
text, so a number in the seed matches), an item of another user is 404,
and a POST gets `user` set. `params` names another field for a
parameter, here `userId`. Ids stay unique across all users.

```toml
# e2e/mocks/todos.toml
[settings]
scope = ["/api/**"]

[[collection]]
name = "todos"
path = "/api/users/:user/todos"
params = { user = "userId" }
items = [
  { id = 1, userId = 1, title = "Buy milk" },
  { id = 2, userId = 2, title = "Walk the dog" },
]
```

## File upload

`upload` puts files into the file input (or the chooser a button opens);
the mock answers the upload, and `expect request` checks what was sent.
`match.form` reads `multipart/form-data` bodies too: a text field's
value is its text, a file field's is its file name. `{{form.x}}` and
`{{form_files.x}}` (`filename`, `content_type`, `size` in bytes) put
them in the answer.

```toml
# e2e/mocks/upload.toml
[settings]
scope = ["/api/**"]

[[mock]]
name = "upload avatar"
match = { method = "POST", path = "/api/avatar", form = { avatar = "*.png" } }
respond = { status = 201, json = { url = "/avatars/{{uuid}}.png", name = "{{form_files.avatar.filename}}" } }

[[mock]]
name = "upload avatar: not a PNG"
match = { method = "POST", path = "/api/avatar" }
respond = { status = 415, json = { error = "PNG only" } }
```

```cdp
# e2e/scenarios/upload.cdp
@mocks upload

@test "uploads an avatar"
  goto /account
  upload label=Avatar e2e/files/avatar.png
  expect 'text=Avatar saved' visible
  expect request POST /api/avatar --header 'content-type: multipart/form-data*'
```

## Optimistic UI

An app that shows a change before the server confirms it must roll back
when the server refuses. A `delay` keeps the request open long enough to
see the optimistic state; the answer is an error.

```toml
# e2e/mocks/like-fails.toml
[[mock]]
name = "like: fails slowly"
match = { method = "POST", path = "/api/posts/:id/like" }
respond = { status = 500, json = { error = "try again" }, delay = "1s" }
```

```cdp
# e2e/scenarios/optimistic.cdp
@test "a like that fails is rolled back"
  @mocks like-fails
  @allow-console-errors  # the 500 is a failed load
  goto /posts/1
  click 'role=button[name="Like"]'
  expect 'text=1 like' visible
  expect 'text=0 likes' visible
  expect role=alert text "Couldn't save your like"
```

## Polling

A `sequence` answers each request in turn, so a job can be running
twice and then done. Time is frozen in every test; `time advance` moves
it on and runs the timers due by then, so the test does not wait for
the app's real interval.

```toml
# e2e/mocks/job.toml
[[mock]]
name = "job: running, running, done"
match = { method = "GET", path = "/api/jobs/:id" }
sequence = [
  { json = { status = "running" } },
  { json = { status = "running" } },
  { json = { status = "done", url = "/reports/7.pdf" } },
]
```

```cdp
# e2e/scenarios/polling.cdp
@test "shows the report when the job is done"
  @mocks job
  goto /jobs/7
  expect 'text=Running' visible
  time advance 5s
  time advance 5s
  expect 'role=link[name="Download report"]' visible
  expect request GET /api/jobs/7 --count 3
```

## Feature flags

Put the flags in `base`, and a set per flag that answers the same request
differently. A later set is tried first, so layering the flag's set turns
it on for one test.

```toml
# e2e/mocks/flags.toml
[[mock]]
name = "flags: defaults"
match = { method = "GET", path = "/api/flags" }
respond = { json = { new_checkout = false, dark_mode = false } }
```

```toml
# e2e/mocks/flags/new-checkout.toml
[[mock]]
name = "flags: new checkout"
match = { method = "GET", path = "/api/flags" }
respond = { json = { new_checkout = true, dark_mode = false } }
```

```cdp
# e2e/scenarios/flags.cdp
@mocks flags

@test "the old checkout by default"
  goto /checkout
  expect 'role=heading[name="Checkout"]' visible

@test "the new checkout behind its flag"
  @mocks flags/new-checkout
  goto /checkout
  expect 'role=heading[name="Checkout (new)"]' visible
```

## Error states

One set per kind of failure, layered onto `base` for the test that needs
it. Tests that expect failed requests need `@allow-console-errors`, since
Chrome logs a failed load as a console error.

```toml
# e2e/mocks/errors/api-down.toml
[[mock]]
name = "api down"
match = { path = "/api/**" }
respond = { status = 503, json = { error = "down for maintenance" } }
```

```toml
# e2e/mocks/errors/offline.toml
[[mock]]
name = "no network"
match = { path = "/api/**" }
respond = { abort = "internet-disconnected" }
```

```toml
# e2e/mocks/errors/slow.toml
[[mock]]
name = "slow products"
match = { method = "GET", path = "/api/products" }
respond = { json = [], delay = "3s" }
```

```cdp
# e2e/scenarios/errors.cdp
@allow-console-errors

@test "the API is down"
  @mocks errors/api-down
  goto /
  expect role=alert text 'down for maintenance'

@test "the network is gone"
  @mocks errors/offline
  goto /
  expect 'text=You are offline' visible

@test "a spinner while products load"
  @mocks errors/slow
  goto /products
  expect role=progressbar visible
  expect 'text=No products yet' visible
```

An empty state needs no set of its own: a collection's items can be
replaced in the test, `mock seed products data/empty.json`, and `expect
mock-state products count 0` checks it.

# Fixture apps

The same small cart, five times: `plain` (TypeScript, no framework),
`react18`, `react19`, `vue` (Vue 3) and `svelte` (Svelte 5). Plan 15's
tests (`source`, node stacks, logpoints, locals, initiators) run against
them. `App` > `CartList` > `CartItem` ×3, with the same `data-testid`s in
every app.

Their **dev builds** are committed under `testdata/site/apps/dev/<name>/`,
so the tests need no Node: development mode (the frameworks' debug info),
no minification, external source maps. `shared/build.mjs` is the Vite
config they share. Each build writes a hash of its sources to
`sources.txt`; `crates/engine/tests/apps.rs` fails when a source changed
without a rebuild.

After changing an app, rebuild it (needs Node):

    cargo xtask apps [NAME...]

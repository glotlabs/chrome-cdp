# Vendored frameworks

Prebuilt React, Vue and Svelte, so the pages that use them need no Node
in tests (plan 10, milestone 10.1):

- `../actions/frameworks.html` checks that `fill` updates the state of
  real React, Vue and Svelte inputs.
- `../apps/` holds the same small todo app in each framework.

The files here are their production builds (all MIT-licensed; their
license headers are kept):

- `react.production.min.js`, `react-dom.production.min.js`: the UMD
  builds of `react@18.3.1` and `react-dom@18.3.1` (18 is the last release
  with UMD builds).
- `vue.global.prod.js`: `dist/vue.global.prod.js` of `vue@3.5.43`. It
  includes the template compiler, so pages write in-DOM templates.
- `svelte-echo.js`, `svelte-todos.js`: `src/Echo.svelte` and
  `src/Todos.svelte` compiled with `svelte@5.57.1`
  (`compile(source, { generate: 'client', name })`) and bundled with its
  runtime by `esbuild@0.28.2` (`--bundle --minify --format=iife
  --target=chrome120 --conditions=browser,production`) from an entry
  that mounts the component on `#svelte-app`. To rebuild one, copy `src/`
  to a scratch folder and run there:

  ```sh
  npm init -y && npm i svelte@5.57.1 esbuild@0.28.2
  node build-svelte.mjs Todos.svelte <repo>/testdata/site/vendor/svelte-todos.js
  ```

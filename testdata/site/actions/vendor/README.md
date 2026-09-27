# Vendored frameworks for the fill fixtures

`../frameworks.html` checks that `fill` updates the state of real React,
Vue and Svelte inputs. The files here are their production builds (all
MIT-licensed; their license headers are kept):

- `react.production.min.js`, `react-dom.production.min.js`: the UMD
  builds of `react@18.3.1` and `react-dom@18.3.1` (18 is the last release
  with UMD builds).
- `vue.global.prod.js`: `dist/vue.global.prod.js` of `vue@3.5.43`.
- `svelte-echo.js`: `Echo.svelte` compiled with `svelte@5.57.1`
  (`compile(source, { generate: 'client', name: 'Echo' })`) and bundled
  with its runtime by `esbuild@0.28.2` (`--bundle --minify --format=iife
  --target=chrome120 --conditions=browser,production`) from an entry
  that mounts it on `#svelte-app`.

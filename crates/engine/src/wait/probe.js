// chrome-cdp's wait probe (plan 03, milestone 3.4), called with
// Runtime.callFunctionOn in a frame: checks `spec` on every animation
// frame (or every 100 ms where frames do not run, e.g. a hidden tab) until
// it holds or `budget` ms pass. Returns {done, observed, value?, fatal?}.
// `fatal` is an error in the spec itself (a bad selector, or with
// `bad_args` a bad `expect` regex): no use waiting. `expect` checks
// (plan 09) also count their tries in `observed.tries`.
// Locators are the in-frame steps of plan 04's locators, evaluated by the
// injected bundle (`__ccdp.query`) from `root` (a ref's element, or the
// document when null). The engine puts `--fn`'s expression (or null) in
// place of the userFn placeholder below.
async function (spec, budget, root) {
  if (spec.kind !== 'fn' && spec.kind !== 'load' && !globalThis.__ccdp) {
    throw new Error('__ccdp is not loaded');
  }
  const deadline = Date.now() + budget;
  const userFn = __FN__;
  const norm = (s) => String(s).replace(/\s+/g, ' ').trim();

  class Fatal extends Error {}
  const find = (steps) => {
    try {
      return globalThis.__ccdp.query(steps, root);
    } catch (e) {
      if (e && e.name === 'LocatorError') throw new Fatal(e.message);
      throw e;
    }
  };
  // Visible: a non-empty box and not `visibility: hidden` (Playwright's
  // definition); `display: none` has no box.
  const visible = (el) => {
    const box = el.getBoundingClientRect();
    if (box.width <= 0 || box.height <= 0) return false;
    return el.checkVisibility ? el.checkVisibility({ visibilityProperty: true }) : true;
  };
  const enabled = (el) => !el.matches(':disabled') && !el.closest('[aria-disabled="true"]');

  const checks = {
    element() {
      const els = find(spec.steps);
      const shown = els.filter(visible).length;
      const on = els.filter(enabled).length;
      const observed = { count: els.length, visible: shown, enabled: on };
      const done = {
        attached: els.length > 0,
        detached: els.length === 0,
        visible: shown > 0,
        hidden: shown === 0,
        enabled: on > 0,
      }[spec.state];
      return { done, observed };
    },
    text() {
      let scope = document.body;
      if (spec.within) {
        scope = find(spec.within)[0];
        if (!scope) return { done: spec.gone, observed: { scope: false, found: false } };
      }
      const found = !!scope && norm(scope.innerText || '').includes(norm(spec.text));
      return { done: found !== spec.gone, observed: { found } };
    },
    load() {
      const state = document.readyState;
      const done = spec.domContentLoaded ? state !== 'loading' : state === 'complete';
      return { done, observed: { readyState: state } };
    },
    async expect() {
      const check = spec.check;
      const seen = spec.steps
        ? await globalThis.__ccdp.expectElements(check, find(spec.steps))
        : globalThis.__ccdp.expectPage(check);
      return { done: !seen.invalid && seen.pass !== spec.not, observed: seen };
    },
    async fn() {
      let value = userFn();
      if (value && typeof value.then === 'function') value = await value;
      let shown;
      try {
        shown = JSON.parse(JSON.stringify(value === undefined ? null : value));
      } catch (e) {
        shown = String(value);
      }
      return { done: !!value, observed: { value: shown }, value: shown };
    },
  };

  let tries = 0;
  for (;;) {
    let result;
    tries++;
    try {
      result = await checks[spec.kind]();
    } catch (e) {
      if (e instanceof Fatal) return { done: false, fatal: e.message };
      if (e && e.name === 'ArgError') return { done: false, fatal: e.message, bad_args: true };
      const error = e && e.message ? `${e.name || 'Error'}: ${e.message}` : String(e);
      result = { done: false, observed: { error } };
    }
    if (spec.kind === 'expect' && result.observed) result.observed.tries = tries;
    if (result.done || Date.now() >= deadline) return result;
    await new Promise((resolve) => {
      requestAnimationFrame(resolve);
      setTimeout(resolve, 100);
    });
  }
}

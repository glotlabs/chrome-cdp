// chrome-cdp's Web Vitals of a load run's page views (plan 23.4). It runs
// in chrome-cdp's isolated world of every top-level document, after perf.js
// (whose observer it reads), and reports each page view once: when the page
// is left (`pagehide`, through the run's binding), or when the load run asks
// at the end of an iteration (`globalThis[Symbol.for('chrome-cdp.vitals')]`,
// which returns it). A page that is not http(s), such as the about:blank a
// context starts with, is no page view.
//
// It also measures how late the page's main thread runs a timer (plan
// 23.5): one every 100 ms (a 0 ms loop would be clamped to 4 ms, and wake
// 250 times a second in every page, spending the CPU it measures), each
// late by how long the thread was busy or starved. A page view reports the
// p99 and the most of its timers' lateness, in ms, as `lag` and `lag_max`.
(() => {
  const KEY = Symbol.for('chrome-cdp.vitals');
  if (globalThis[KEY] || window !== window.top) return;
  const BINDING = '__chromeCdpVitals';
  let reported = false;
  const EVERY = 100;
  const KEPT = 3000;
  const lags = [];
  let lagMax = 0;
  let due = performance.now() + EVERY;
  const tick = () => {
    const now = performance.now();
    // A hidden page's timers are slowed down on purpose: no lag.
    if (!document.hidden) {
      const late = Math.max(0, now - due);
      if (lags.length < KEPT) lags.push(late);
      else lags[Math.floor(Math.random() * KEPT)] = late;
      if (late > lagMax) lagMax = late;
    }
    due = now + EVERY;
    if (!reported) setTimeout(tick, EVERY);
  };
  if (/^https?:$/.test(location.protocol)) setTimeout(tick, EVERY);
  const lag = () => {
    if (!lags.length) return [null, null];
    const sorted = [...lags].sort((a, b) => a - b);
    const p99 = sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * 0.99))];
    return [Math.round(p99 * 10) / 10, Math.round(lagMax * 10) / 10];
  };
  const read = () => {
    const state = globalThis[Symbol.for('chrome-cdp.perf')];
    if (reported || !state || !/^https?:$/.test(location.protocol)) return null;
    reported = true;
    for (const [observer, handle] of state.observers) observer.takeRecords().forEach(handle);
    const nav = performance.getEntriesByType('navigation')[0];
    const fcp = performance.getEntriesByType('paint').find((p) => p.name === 'first-contentful-paint');
    // INP: the interaction at the 98th percentile, as web-vitals picks it.
    const longest = [...state.interactions.values()].sort((a, b) => b.duration - a.duration);
    const count = Math.max(performance.interactionCount || 0, state.interactions.size);
    const inp = longest.length ? longest[Math.min(longest.length - 1, Math.floor(count / 50))] : null;
    const time = (n) => (n == null || n <= 0 ? null : n);
    const [lagP99, lagMost] = lag();
    return {
      url: location.href,
      ttfb: nav ? time(nav.responseStart) : null,
      fcp: fcp ? time(fcp.startTime) : null,
      load: nav ? time(nav.loadEventEnd) : null,
      lcp: state.lcp ? time(state.lcp.time) : null,
      cls: state.cls,
      inp: inp ? inp.duration : null,
      lag: lagP99,
      lag_max: lagMost,
    };
  };
  globalThis[KEY] = read;
  addEventListener('pagehide', () => {
    const vitals = read();
    const send = globalThis[BINDING];
    if (vitals && typeof send === 'function') send(JSON.stringify(vitals));
  }, { capture: true });
})();

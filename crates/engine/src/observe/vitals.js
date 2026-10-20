// chrome-cdp's Web Vitals of a load run's page views (plan 23.4). It runs
// in chrome-cdp's isolated world of every top-level document, after perf.js
// (whose observer it reads), and reports each page view once: when the page
// is left (`pagehide`, through the run's binding), or when the load run asks
// at the end of an iteration (`globalThis[Symbol.for('chrome-cdp.vitals')]`,
// which returns it). A page that is not http(s), such as the about:blank a
// context starts with, is no page view.
(() => {
  const KEY = Symbol.for('chrome-cdp.vitals');
  if (globalThis[KEY] || window !== window.top) return;
  const BINDING = '__chromeCdpVitals';
  let reported = false;
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
    return {
      url: location.href,
      ttfb: nav ? time(nav.responseStart) : null,
      fcp: fcp ? time(fcp.startTime) : null,
      load: nav ? time(nav.loadEventEnd) : null,
      lcp: state.lcp ? time(state.lcp.time) : null,
      cls: state.cls,
      inp: inp ? inp.duration : null,
    };
  };
  globalThis[KEY] = read;
  addEventListener('pagehide', () => {
    const vitals = read();
    const send = globalThis[BINDING];
    if (vitals && typeof send === 'function') send(JSON.stringify(vitals));
  }, { capture: true });
})();

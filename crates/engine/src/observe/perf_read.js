// What `perf` reads from the page (plan 06, milestone 6.5), after perf.js
// (which installs the observer if this world has none yet). Runs in
// chrome-cdp's isolated world of the top frame; the value is JSON.
(() => {
  const state = globalThis[Symbol.for('chrome-cdp.perf')];
  // Entries the observers have not been called back with yet.
  for (const [observer, handle] of state.observers) observer.takeRecords().forEach(handle);
  const round = (n) => (n == null || n < 0 ? null : Math.round(n));
  const nav = performance.getEntriesByType('navigation')[0];
  const paints = performance.getEntriesByType('paint');
  const fcp = paints.find((p) => p.name === 'first-contentful-paint');
  const resources = performance.getEntriesByType('resource');
  let transfer = nav ? nav.transferSize : 0;
  for (const r of resources) transfer += r.transferSize || 0;
  // INP: the interaction at the 98th percentile, as web-vitals picks it.
  const longest = [...state.interactions.values()].sort((a, b) => b.duration - a.duration);
  const count = Math.max(performance.interactionCount || 0, state.interactions.size);
  const inp = longest.length
    ? longest[Math.min(longest.length - 1, Math.floor(count / 50))]
    : null;
  // Total blocking time: each long task's time over 50ms, after FCP.
  const after = fcp ? fcp.startTime : 0;
  let blocking = 0;
  let longTasks = 0;
  for (const [start, duration] of state.longTasks) {
    if (start + duration < after) continue;
    longTasks += 1;
    blocking += duration - 50;
  }
  return {
    url: location.href,
    fromStart: state.fromStart,
    navigation: nav ? {
      type: nav.type,
      ttfb: round(nav.responseStart),
      domContentLoaded: round(nav.domContentLoadedEventEnd || null),
      load: round(nav.loadEventEnd || null),
    } : null,
    fcp: fcp ? round(fcp.startTime) : null,
    requests: resources.length + (nav ? 1 : 0),
    transfer,
    lcp: state.lcp ? { ...state.lcp, time: round(state.lcp.time) } : null,
    cls: Math.round(state.cls * 10000) / 10000,
    shifts: state.shifts,
    inp: inp ? { ...inp, duration: round(inp.duration), interactions: count } : null,
    interactions: count,
    blocking: round(blocking),
    longTasks,
    elements: document.getElementsByTagName('*').length,
    sinceLoad: round(performance.now()),
  };
})()

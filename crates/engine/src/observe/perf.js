// chrome-cdp's page speed observer, for `perf` (plan 06, milestone 6.5).
// It runs in chrome-cdp's isolated world of every top-level document before
// the page's own scripts (and later, on demand, in a world made after the
// page loaded: then it sees only what the browser buffered). It keeps what
// the Web Vitals need and the page cannot tell afterwards: the latest LCP,
// the layout shift session windows (CLS), each interaction's longest event
// (INP), and the long tasks. It never touches the page's own world.
(() => {
  const KEY = Symbol.for('chrome-cdp.perf');
  if (globalThis[KEY] || window !== window.top) return;
  const describe = (el) => {
    if (!el || !el.localName) return null;
    let text = el.localName;
    if (el.id) text += `#${el.id}`;
    else if (el.classList && el.classList.length) text += `.${[...el.classList].slice(0, 2).join('.')}`;
    return text;
  };
  const state = {
    // Whether it watched from the document's start (else it has only what
    // the browser buffered: event timing only for 104ms and slower).
    fromStart: document.readyState === 'loading',
    lcp: null,
    cls: 0,
    shifts: 0,
    window: 0,
    windowStart: 0,
    windowEnd: 0,
    // interactionId -> the longest event of that interaction.
    interactions: new Map(),
    longTasks: [],
    observers: [],
  };
  const handle = {
    'largest-contentful-paint': (e) => {
      state.lcp = { time: e.startTime, size: e.size, element: describe(e.element), url: e.url || null };
    },
    'layout-shift': (e) => {
      if (e.hadRecentInput) return;
      state.shifts += 1;
      // A session window: shifts less than 1s apart, 5s at most.
      if (state.window > 0 && e.startTime - state.windowEnd < 1000
          && e.startTime - state.windowStart < 5000) {
        state.window += e.value;
      } else {
        state.window = e.value;
        state.windowStart = e.startTime;
      }
      state.windowEnd = e.startTime;
      state.cls = Math.max(state.cls, state.window);
    },
    event: (e) => {
      if (!e.interactionId) return;
      const target = describe(e.target);
      const known = state.interactions.get(e.interactionId);
      if (!known) {
        state.interactions.set(e.interactionId, { duration: e.duration, name: e.name, target });
        return;
      }
      if (e.duration > known.duration) known.duration = e.duration;
      // Pointer events can have no target; the click of the same
      // interaction has one.
      if (target && (!known.target || e.duration >= known.duration)) {
        known.name = e.name;
        known.target = target;
      }
    },
    longtask: (e) => {
      if (state.longTasks.length < 1000) state.longTasks.push([e.startTime, e.duration]);
    },
  };
  handle['first-input'] = handle.event;
  const types = [
    ['largest-contentful-paint', {}],
    ['layout-shift', {}],
    ['event', { durationThreshold: 16 }],
    ['first-input', {}],
    ['longtask', {}],
  ];
  for (const [type, options] of types) {
    try {
      const observer = new PerformanceObserver((list) => list.getEntries().forEach(handle[type]));
      observer.observe({ type, buffered: true, ...options });
      state.observers.push([observer, handle[type]]);
    } catch {
      // A type this browser does not have.
    }
  }
  globalThis[KEY] = state;
})();

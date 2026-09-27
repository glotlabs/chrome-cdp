// chrome-cdp's DOM-quiet probe (plan 03, milestone 3.4, the settle):
// resolves once a MutationObserver has seen no change to the document for
// `quietMs`, or after `maxMs` in all. Returns {quiet, mutations}.
function (quietMs, maxMs) {
  return new Promise((resolve) => {
    const start = performance.now();
    let last = start;
    let mutations = 0;
    const observer = new MutationObserver((records) => {
      mutations += records.length;
      last = performance.now();
    });
    observer.observe(document, {
      subtree: true, childList: true, attributes: true, characterData: true,
    });
    const tick = () => {
      const now = performance.now();
      const quiet = now - last >= quietMs;
      if (quiet || now - start >= maxMs) {
        observer.disconnect();
        resolve({ quiet, mutations });
        return;
      }
      setTimeout(tick, Math.max(1, Math.min(quietMs - (now - last), maxMs - (now - start))));
    };
    setTimeout(tick, quietMs);
  });
}

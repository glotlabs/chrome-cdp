// chrome-cdp's user agent in a worker (`ua`, plan 13): Chrome 154 sends a
// worker's requests with the overridden User-Agent, but its
// `navigator.userAgent` keeps the browser's own. Called with the user
// agent before the worker's script, and again when it changes; `null`
// gives the browser's back.
(function (ua) {
  const key = Symbol.for("chrome-cdp.ua");
  const proto = Object.getPrototypeOf(navigator);
  let saved = globalThis[key];
  if (!saved) {
    if (ua === null) return;
    saved = {
      userAgent: Object.getOwnPropertyDescriptor(proto, "userAgent"),
      appVersion: Object.getOwnPropertyDescriptor(proto, "appVersion"),
    };
    Object.defineProperty(globalThis, key, { configurable: true, value: saved });
  }
  if (ua === null) {
    for (const [name, descriptor] of Object.entries(saved)) {
      if (descriptor) Object.defineProperty(proto, name, descriptor);
    }
    delete globalThis[key];
    return;
  }
  const getter = (value) => ({ configurable: true, enumerable: true, get: () => value });
  Object.defineProperty(proto, "userAgent", getter(ua));
  Object.defineProperty(proto, "appVersion", getter(ua.replace(/^Mozilla\//, "")));
})

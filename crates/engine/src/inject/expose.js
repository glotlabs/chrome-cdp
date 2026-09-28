// An exposed function (`chrome-cdp expose`): takes the binding Chrome put
// in this world, hides it, and puts a function under the asked name that
// sends its arguments as a JSON array and returns a promise.
(() => {
  const name = __NAME__;
  const binding = __BINDING__;
  const send = globalThis[binding];
  if (typeof send !== 'function') return;
  delete globalThis[binding];
  const plain = (key, value) =>
    value === undefined ? null : typeof value === 'bigint' ? String(value) : value;
  const exposed = (...args) => {
    let payload;
    try {
      payload = JSON.stringify(args, plain);
    } catch {
      payload = JSON.stringify(args.map((a) => String(a)));
    }
    send(payload);
    return Promise.resolve();
  };
  Object.defineProperty(globalThis, name, {
    value: exposed,
    configurable: true,
    writable: true,
  });
})();

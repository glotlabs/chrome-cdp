// An exposed function (`chrome-cdp expose`): takes the binding Chrome put
// in this world, hides it, and puts a function under the asked name that
// sends its arguments as a JSON array and returns a promise. With answers
// (`expose NAME --returns`), each call has a number and its promise waits
// for the daemon to hand that call its answer.
(() => {
  const name = __NAME__;
  const binding = __BINDING__;
  const answers = __ANSWERS__;
  const send = globalThis[binding];
  if (typeof send !== 'function') return;
  delete globalThis[binding];
  const plain = (key, value) =>
    value === undefined ? null : typeof value === 'bigint' ? String(value) : value;
  // The arguments as JSON, `wrap`ped; as strings when JSON cannot take them.
  const encode = (args, wrap) => {
    try {
      return JSON.stringify(wrap(args), plain);
    } catch {
      return JSON.stringify(wrap(args.map((a) => String(a))));
    }
  };
  const pending = new Map();
  let last = 0;
  const exposed = (...args) => {
    if (!answers) {
      send(encode(args, (a) => a));
      return Promise.resolve();
    }
    const call = ++last;
    const promise = new Promise((resolve) => pending.set(call, resolve));
    send(encode(args, (a) => ({ call, args: a })));
    return promise;
  };
  Object.defineProperty(globalThis, name, {
    value: exposed,
    configurable: true,
    writable: true,
  });
  if (answers) {
    Object.defineProperty(globalThis, Symbol.for('chrome-cdp.expose.' + name), {
      value: (call, answer) => {
        const resolve = pending.get(call);
        pending.delete(call);
        resolve?.(answer);
      },
      configurable: true,
    });
  }
})();

// chrome-cdp's frozen clock (`time freeze`, plan 07): while it is
// installed, `new Date()`, `Date()`, `Date.now()` and `Intl.DateTimeFormat`'s
// `format()`/`formatToParts()` without a date give the frozen time. Timers,
// `performance.now()` and animations keep running in real time, so a page
// never stalls. Called with the time (ms since the Unix epoch) at each new
// document, and again in the existing ones when the time changes; `null`
// takes it out.
(function (at) {
  const key = Symbol.for("chrome-cdp.clock");
  const installed = globalThis[key];
  if (installed) {
    installed.set(at);
    return;
  }
  if (at === null) return;

  const RealDate = Date;
  const proto = Intl.DateTimeFormat.prototype;
  const realFormat = Object.getOwnPropertyDescriptor(proto, "format");
  const realFormatToParts = Object.getOwnPropertyDescriptor(proto, "formatToParts");
  let current = at;

  const now = { now() { return current; } }.now;
  const FakeDate = new Proxy(RealDate, {
    apply() {
      return new RealDate(current).toString();
    },
    construct(target, args, newTarget) {
      const target_ = newTarget === FakeDate ? target : newTarget;
      return Reflect.construct(target, args.length ? args : [current], target_);
    },
    get(target, prop) {
      return prop === "now" ? now : Reflect.get(target, prop, target);
    },
  });

  function uninstall() {
    globalThis.Date = RealDate;
    RealDate.prototype.constructor = RealDate;
    Object.defineProperty(proto, "format", realFormat);
    Object.defineProperty(proto, "formatToParts", realFormatToParts);
    delete globalThis[key];
  }

  globalThis.Date = FakeDate;
  RealDate.prototype.constructor = FakeDate;
  Object.defineProperty(proto, "format", {
    configurable: true,
    get() {
      const format = realFormat.get.call(this);
      return (date) => format(date === undefined ? current : date);
    },
  });
  Object.defineProperty(proto, "formatToParts", {
    configurable: true,
    writable: true,
    value: function formatToParts(date) {
      return realFormatToParts.value.call(this, date === undefined ? current : date);
    },
  });
  Object.defineProperty(globalThis, key, {
    configurable: true,
    value: {
      set(time) {
        if (time === null) uninstall();
        else current = time;
      },
    },
  });
})

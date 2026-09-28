// chrome-cdp's frozen clock (`time freeze`, plan 07): while it is
// installed, `new Date()`, `Date()`, `Date.now()` and `Intl.DateTimeFormat`'s
// `format()`/`formatToParts()` without a date give the frozen time. Timers,
// `performance.now()` and animations keep running in real time, so a page
// never stalls. Moving the frozen time on (`time advance`, plan 11.6) also
// moves `performance.now()` on by as much, and runs the `setTimeout` and
// `setInterval` callbacks due by then, each once, in the order they are
// due. Called with the time (ms since the Unix epoch) at each new
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

  // The timers stay once wrapped (a page may hold their ids), so a later
  // freeze in the same document finds them.
  const timersKey = Symbol.for("chrome-cdp.timers");
  const timers = globalThis[timersKey] || wrapTimers(timersKey);

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
        if (time === null) {
          uninstall();
          return;
        }
        const moved = time - current;
        current = time;
        if (moved > 0) timers.advance(moved);
      },
    },
  });

  // Wraps setTimeout, setInterval and performance.now so the time can
  // jump: each timer knows when it is due (in performance.now() time), and
  // a jump re-arms the pending ones for what is left. A timer's id is the
  // id of the first real timer behind it; real ids are never reused, so
  // clearTimeout tells ours from the page's own.
  function wrapTimers(timersKey) {
    const realSetTimeout = globalThis.setTimeout;
    const realClearTimeout = globalThis.clearTimeout;
    const realSetInterval = globalThis.setInterval;
    const perf = globalThis.performance;
    const realNow = perf.now.bind(perf);
    let skew = 0;
    const pending = new Map();
    const vnow = () => realNow() + skew;

    function arm(timer, delay) {
      timer.real = realSetTimeout(() => fire(timer), Math.max(0, delay));
    }
    function fire(timer) {
      if (!pending.has(timer.id)) return;
      if (timer.interval === null) {
        pending.delete(timer.id);
      } else {
        timer.due = vnow() + timer.interval;
        arm(timer, timer.interval);
      }
      timer.callback.apply(globalThis, timer.args);
    }
    function schedule(callback, delay, args, repeat) {
      if (typeof callback !== "function") {
        // A string of code: left to the browser, and not moved on.
        return (repeat ? realSetInterval : realSetTimeout)(callback, delay, ...args);
      }
      delay = Number(delay);
      // As the browser does: NaN, negative and over 2^31-1 mean 0.
      if (!(delay >= 0 && delay <= 0x7fffffff)) delay = 0;
      const timer = { callback, args, interval: repeat ? delay : null, due: vnow() + delay };
      arm(timer, delay);
      timer.id = timer.real;
      pending.set(timer.id, timer);
      return timer.id;
    }
    function clear(id) {
      const timer = pending.get(id);
      if (timer) {
        pending.delete(id);
        realClearTimeout(timer.real);
      } else {
        realClearTimeout(id);
      }
    }

    globalThis.setTimeout = function setTimeout(callback, delay, ...args) {
      return schedule(callback, delay, args, false);
    };
    globalThis.setInterval = function setInterval(callback, delay, ...args) {
      return schedule(callback, delay, args, true);
    };
    globalThis.clearTimeout = function clearTimeout(id) {
      clear(id);
    };
    globalThis.clearInterval = function clearInterval(id) {
      clear(id);
    };
    perf.now = function now() {
      return vnow();
    };
    const timers = {
      advance(ms) {
        skew += ms;
        const due = [...pending.values()].sort((a, b) => a.due - b.due);
        for (const timer of due) {
          realClearTimeout(timer.real);
          arm(timer, timer.due - vnow());
        }
      },
    };
    Object.defineProperty(globalThis, timersKey, { configurable: true, value: timers });
    return timers;
  }
})

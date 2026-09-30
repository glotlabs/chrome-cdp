// chrome-cdp's frozen clock (`time freeze`, plan 07): while it is
// installed, `new Date()`, `Date()`, `Date.now()` and `Intl.DateTimeFormat`'s
// `format()`/`formatToParts()` without a date give the frozen time. Timers,
// `performance.now()` and animations keep running in real time, so a page
// never stalls. Moving the frozen time on (`time advance`, plan 11.6) also
// moves `performance.now()` on by as much, and runs the `setTimeout` and
// `setInterval` callbacks due by then, each once, in the order they are
// due.
//
// Paused (`time pause`, plan 14.2), timers and `requestAnimationFrame`
// callbacks are queued instead, and `performance.now()` stands still. The
// time then moves in one of two ways: "run" (`time run-for`/`pause-at`)
// fires every callback due in order, each tick of an interval and a frame
// every 16ms, with `Date` and `performance.now()` at each one's time; "jump"
// (`time advance` while paused) moves to the end and fires each callback
// due by then once.
//
// Called with the time (ms since the Unix epoch) and the mode ("frozen",
// "paused", "run" or "jump") at each new document, and again in the
// existing ones when either changes; `null` takes it out. What the last
// change fired is the clock's `last`.
(function (at, mode) {
  const key = Symbol.for("chrome-cdp.clock");
  const installed = globalThis[key];
  if (installed) {
    installed.set(at, mode);
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
  const clock = {
    last: { timers: 0, frames: 0, capped: false },
    set(time, mode) {
      clock.last = { timers: 0, frames: 0, capped: false };
      if (time === null) {
        timers.resume();
        uninstall();
        return;
      }
      const moved = time - current;
      if (mode === "frozen") {
        timers.resume();
        current = time;
        if (moved > 0) timers.advance(moved);
        return;
      }
      timers.pause();
      if (moved > 0 && (mode === "run" || mode === "jump")) {
        const start = current;
        const tick = (elapsed) => {
          current = start + elapsed;
        };
        clock.last = mode === "run" ? timers.run(moved, tick) : timers.jump(moved, tick);
      }
      current = time;
    },
  };
  Object.defineProperty(globalThis, key, { configurable: true, value: clock });
  if (mode !== "frozen") timers.pause();

  // Wraps setTimeout, setInterval and performance.now so the time can
  // jump or stop: each timer knows when it is due (in performance.now()
  // time), and a jump re-arms the pending ones for what is left. A timer's
  // id is the id of a real timer (the first one behind it, or one made and
  // cleared at once while paused); real ids are never reused, so
  // clearTimeout tells ours from the page's own.
  function wrapTimers(timersKey) {
    const realSetTimeout = globalThis.setTimeout;
    const realClearTimeout = globalThis.clearTimeout;
    const realSetInterval = globalThis.setInterval;
    const realRequestFrame = globalThis.requestAnimationFrame;
    const realCancelFrame = globalThis.cancelAnimationFrame;
    const perf = globalThis.performance;
    const realNow = perf.now.bind(perf);
    // After this many callbacks in one run, the rest stay queued: a
    // callback that sets itself again at once would never let it end.
    const RUN_LIMIT = 10000;
    const FRAME_MS = 16;
    let skew = 0;
    // performance.now() while paused.
    let stopped = null;
    // Where paused frames are counted from.
    let frameOrigin = 0;
    // Callbacks due at the same time run in the order they were set.
    let order = 0;
    const pending = new Map();
    // The requestAnimationFrame callbacks queued while paused, by id.
    const frames = new Map();
    const vnow = () => (stopped === null ? realNow() + skew : stopped);
    // A document the clock came to after it had started may have timers
    // of its own the clock cannot stop.
    const late =
      typeof document !== "undefined" &&
      document.readyState !== "loading" &&
      location.href !== "about:blank";

    function arm(timer, delay) {
      if (stopped !== null) return;
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
      const timer = {
        callback,
        args,
        interval: repeat ? delay : null,
        due: vnow() + delay,
        order: order++,
      };
      if (stopped === null) {
        arm(timer, delay);
        timer.id = timer.real;
      } else {
        timer.id = realSetTimeout(() => {}, 0);
        realClearTimeout(timer.id);
      }
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
    // The first frame after `time`.
    function nextFrame(time) {
      return frameOrigin + (Math.floor((time - frameOrigin) / FRAME_MS) + 1) * FRAME_MS;
    }
    function requestAnimationFrame(callback) {
      // An id of the browser's own, so cancelAnimationFrame tells them apart.
      const id = realRequestFrame.call(globalThis, () => {});
      realCancelFrame.call(globalThis, id);
      frames.set(id, { callback, due: nextFrame(vnow()) });
      return id;
    }
    function cancelAnimationFrame(id) {
      if (!frames.delete(id)) realCancelFrame.call(globalThis, id);
    }
    // Runs a callback; what it throws is reported as the browser would,
    // and does not stop the others.
    function call(callback, self, args) {
      try {
        callback.apply(self, args);
      } catch (error) {
        if (typeof globalThis.reportError === "function") globalThis.reportError(error);
      }
    }
    // The pending timer due first, if it is due by `end`.
    function firstTimer(end) {
      let first = null;
      for (const timer of pending.values()) {
        if (timer.due > end) continue;
        if (!first || timer.due < first.due || (timer.due === first.due && timer.order < first.order)) {
          first = timer;
        }
      }
      return first;
    }
    function firstFrame(end) {
      let first = null;
      for (const frame of frames.values()) {
        if (frame.due <= end && (first === null || frame.due < first)) first = frame.due;
      }
      return first;
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
      late,
      advance(ms) {
        skew += ms;
        const due = [...pending.values()].sort((a, b) => a.due - b.due);
        for (const timer of due) {
          realClearTimeout(timer.real);
          arm(timer, timer.due - vnow());
        }
      },
      // Stops the timers where they are, and queues frames; `rAF` is
      // wrapped only while paused, so animations are left alone otherwise.
      pause() {
        if (stopped !== null) return;
        stopped = vnow();
        frameOrigin = stopped;
        for (const timer of pending.values()) realClearTimeout(timer.real);
        if (typeof realRequestFrame === "function") {
          globalThis.requestAnimationFrame = requestAnimationFrame;
          globalThis.cancelAnimationFrame = cancelAnimationFrame;
        }
      },
      // Timers run in real time again, from where they stopped; the queued
      // frames go to the browser (under new ids).
      resume() {
        if (stopped === null) return;
        skew = stopped - realNow();
        stopped = null;
        for (const timer of pending.values()) arm(timer, timer.due - vnow());
        if (typeof realRequestFrame === "function") {
          globalThis.requestAnimationFrame = realRequestFrame;
          globalThis.cancelAnimationFrame = realCancelFrame;
          for (const frame of frames.values()) realRequestFrame.call(globalThis, frame.callback);
          frames.clear();
        }
      },
      // Moves on by `ms` while paused, firing every callback due on the
      // way at its time; `tick` gets the time moved so far before each.
      run(ms, tick) {
        const start = stopped;
        const end = start + ms;
        const fired = { timers: 0, frames: 0, capped: false };
        let calls = 0;
        for (;;) {
          const timer = firstTimer(end);
          const frame = firstFrame(end);
          if (!timer && frame === null) break;
          if (calls >= RUN_LIMIT) {
            fired.capped = true;
            break;
          }
          if (timer && (frame === null || timer.due <= frame)) {
            stopped = Math.max(stopped, timer.due);
            tick(stopped - start);
            if (timer.interval === null) {
              pending.delete(timer.id);
            } else {
              timer.due += Math.max(timer.interval, 1);
              timer.order = order++;
            }
            call(timer.callback, globalThis, timer.args);
            fired.timers++;
            calls++;
          } else {
            stopped = Math.max(stopped, frame);
            tick(stopped - start);
            const batch = [...frames].filter(([, f]) => f.due <= frame);
            for (const [id] of batch) frames.delete(id);
            for (const [, f] of batch) call(f.callback, globalThis, [stopped]);
            fired.frames++;
            calls += batch.length;
          }
        }
        stopped = end;
        tick(ms);
        return fired;
      },
      // Moves on by `ms` while paused, then fires each callback that was
      // due by then once, in order, and the queued frames once.
      jump(ms, tick) {
        const end = stopped + ms;
        stopped = end;
        tick(ms);
        const fired = { timers: 0, frames: 0, capped: false };
        const due = [...pending.values()]
          .filter((t) => t.due <= end)
          .sort((a, b) => a.due - b.due || a.order - b.order);
        for (const timer of due) {
          if (!pending.has(timer.id)) continue;
          if (timer.interval === null) {
            pending.delete(timer.id);
          } else {
            timer.due = end + timer.interval;
          }
          call(timer.callback, globalThis, timer.args);
          fired.timers++;
        }
        const batch = [...frames].filter(([, f]) => f.due <= end);
        for (const [id] of batch) frames.delete(id);
        for (const [, f] of batch) call(f.callback, globalThis, [end]);
        if (batch.length) fired.frames++;
        return fired;
      },
    };
    Object.defineProperty(globalThis, timersKey, { configurable: true, value: timers });
    return timers;
  }
})

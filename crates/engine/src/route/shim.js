// Mocked WebSockets and EventSources (`chrome-cdp mock use`, plan 11.9).
//
// Fetch interception sees neither a WebSocket nor a stream, so this
// replaces `WebSocket` and `EventSource` with stand-ins that ask the
// daemon (over the binding, hidden here) what each one gets: a mock
// (`[[websocket]]`, or an `sse` answer), a failure (unmatched in a mock
// set's scope), or the real thing, which the stand-in then wraps. The
// daemon answers through `globalThis[Symbol.for('chrome-cdp.mock')]`.
(() => {
  const binding = globalThis.__BINDING__;
  const hook = Symbol.for('chrome-cdp.mock');
  if (typeof binding !== 'function') return;
  delete globalThis.__BINDING__;
  if (globalThis[hook]) {
    // Here before (mock sets were turned off and on again): its
    // stand-ins take new sockets again.
    globalThis[hook]({ on: true });
    return;
  }
  const post = (message) => binding(JSON.stringify(message));
  const RealWebSocket = globalThis.WebSocket;
  const RealEventSource = globalThis.EventSource;
  const live = new Map();
  let last = 0;

  // Binary messages cross the binding as base64.
  const toBase64 = (bytes) => {
    let text = '';
    for (let i = 0; i < bytes.length; i += 0x8000) {
      text += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
    }
    return btoa(text);
  };
  const fromBase64 = (base64) => Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
  const isBinary = (data) =>
    data instanceof ArrayBuffer || ArrayBuffer.isView(data) || (typeof Blob === 'function' && data instanceof Blob);
  // The bytes as they are at `send`, as a real socket takes them.
  const bytesOf = (data) => {
    if (data instanceof ArrayBuffer) return Promise.resolve(new Uint8Array(data.slice(0)));
    if (ArrayBuffer.isView(data)) {
      return Promise.resolve(new Uint8Array(data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength)));
    }
    return data.arrayBuffer().then((buffer) => new Uint8Array(buffer));
  };

  const syntaxError = (what, url) =>
    new DOMException(`Failed to construct '${what}': The URL '${url}' is invalid.`, 'SyntaxError');

  // `onopen` and friends, on top of addEventListener.
  const handlers = (proto, types) => {
    for (const type of types) {
      const key = Symbol(type);
      Object.defineProperty(proto, `on${type}`, {
        configurable: true,
        enumerable: true,
        get() {
          return this[key] ? this[key].fn : null;
        },
        set(fn) {
          if (this[key]) this.removeEventListener(type, this[key].listener);
          this[key] = null;
          if (typeof fn === 'function') {
            const listener = (event) => fn.call(this, event);
            this[key] = { fn, listener };
            this.addEventListener(type, listener);
          }
        },
      });
    }
  };

  const constants = (cls, names) => {
    names.forEach((name, value) => {
      for (const target of [cls, cls.prototype]) {
        Object.defineProperty(target, name, { value, enumerable: true });
      }
    });
  };

  class WebSocket extends EventTarget {
    #id;
    #url;
    #protocols;
    #state = 0;
    #protocol = '';
    #binaryType = 'blob';
    #real = null;
    // Sends in order: a Blob is read before it goes, and later ones wait.
    #sending = Promise.resolve();

    constructor(url, protocols) {
      super();
      if (arguments.length === 0) {
        throw new TypeError("Failed to construct 'WebSocket': 1 argument required, but only 0 present.");
      }
      let parsed;
      try {
        parsed = new URL(String(url), location.href);
      } catch {
        throw syntaxError('WebSocket', url);
      }
      if (parsed.protocol === 'http:') parsed.protocol = 'ws:';
      if (parsed.protocol === 'https:') parsed.protocol = 'wss:';
      if (!['ws:', 'wss:'].includes(parsed.protocol) || parsed.hash) {
        throw syntaxError('WebSocket', url);
      }
      this.#url = parsed.href;
      this.#protocols =
        protocols === undefined ? [] : Array.isArray(protocols) ? protocols.map(String) : [String(protocols)];
      this.#id = ++last;
      live.set(this.#id, (message) => this.#deliver(message));
      post({ op: 'ws-open', id: this.#id, url: this.#url, protocols: this.#protocols });
    }

    get url() {
      return this.#url;
    }
    get readyState() {
      return this.#real ? this.#real.readyState : this.#state;
    }
    get protocol() {
      return this.#real ? this.#real.protocol : this.#protocol;
    }
    get extensions() {
      return this.#real ? this.#real.extensions : '';
    }
    get bufferedAmount() {
      return this.#real ? this.#real.bufferedAmount : 0;
    }
    get binaryType() {
      return this.#binaryType;
    }
    set binaryType(type) {
      if (type !== 'blob' && type !== 'arraybuffer') return;
      this.#binaryType = type;
      if (this.#real) this.#real.binaryType = type;
    }

    send(data) {
      if (this.#real) return this.#real.send(data);
      if (this.#state === 0) {
        throw new DOMException("Failed to execute 'send' on 'WebSocket': Still in CONNECTING state.", 'InvalidStateError');
      }
      if (this.#state !== 1) return;
      const id = this.#id;
      if (!isBinary(data)) {
        const text = String(data);
        this.#sending = this.#sending.then(() => post({ op: 'ws-send', id, data: text }));
        return;
      }
      const bytes = bytesOf(data);
      this.#sending = this.#sending
        .then(async () => post({ op: 'ws-send', id, base64: toBase64(await bytes) }))
        .catch(() => {});
    }

    close(code, reason) {
      if (code !== undefined && code !== 1000 && !(code >= 3000 && code <= 4999)) {
        throw new DOMException(
          `Failed to execute 'close' on 'WebSocket': The close code must be either 1000, or between 3000 and 4999. ${code} is neither.`,
          'InvalidAccessError',
        );
      }
      if (this.#real) return this.#real.close(code, reason);
      if (this.#state >= 2) return;
      this.#state = 2;
      post({ op: 'ws-close', id: this.#id, code, reason });
      setTimeout(() => this.#closed(code ?? 1005, reason ?? '', true), 0);
    }

    #closed(code, reason, wasClean) {
      if (this.#state === 3) return;
      this.#state = 3;
      live.delete(this.#id);
      this.dispatchEvent(new CloseEvent('close', { code, reason, wasClean }));
    }

    #deliver(message) {
      if (message.decision === 'real') {
        if (this.#state >= 2) return;
        const real = new RealWebSocket(this.#url, this.#protocols);
        real.binaryType = this.#binaryType;
        this.#real = real;
        live.delete(this.#id);
        for (const type of ['open', 'message', 'error', 'close']) {
          real.addEventListener(type, (event) => {
            let copy;
            if (type === 'message') {
              copy = new MessageEvent('message', { data: event.data, origin: event.origin });
            } else if (type === 'close') {
              copy = new CloseEvent('close', { code: event.code, reason: event.reason, wasClean: event.wasClean });
            } else {
              copy = new Event(type);
            }
            this.dispatchEvent(copy);
          });
        }
        return;
      }
      if (message.decision === 'fail') {
        if (this.#state === 3) return;
        this.#state = 3;
        live.delete(this.#id);
        this.dispatchEvent(new Event('error'));
        this.dispatchEvent(new CloseEvent('close', { code: 1006, reason: '', wasClean: false }));
        return;
      }
      if (message.decision === 'mock') {
        if (this.#state !== 0) return;
        this.#state = 1;
        this.#protocol = message.protocol || '';
        this.dispatchEvent(new Event('open'));
        return;
      }
      if (message.text !== undefined) {
        if (this.#state !== 1) return;
        const origin = new URL(this.#url).origin;
        this.dispatchEvent(new MessageEvent('message', { data: message.text, origin }));
        return;
      }
      if (message.base64 !== undefined) {
        if (this.#state !== 1) return;
        const bytes = fromBase64(message.base64);
        const data = this.#binaryType === 'arraybuffer' ? bytes.buffer : new Blob([bytes]);
        const origin = new URL(this.#url).origin;
        this.dispatchEvent(new MessageEvent('message', { data, origin }));
        return;
      }
      if (message.close) {
        this.#closed(message.close.code, message.close.reason, true);
      }
    }
  }
  constants(WebSocket, ['CONNECTING', 'OPEN', 'CLOSING', 'CLOSED']);
  handlers(WebSocket.prototype, ['open', 'message', 'error', 'close']);

  class EventSource extends EventTarget {
    #id;
    #url;
    #init;
    #state = 0;
    #real = null;
    #types = new Set();
    #lastEventId = '';

    constructor(url, init) {
      super();
      if (arguments.length === 0) {
        throw new TypeError("Failed to construct 'EventSource': 1 argument required, but only 0 present.");
      }
      let parsed;
      try {
        parsed = new URL(String(url), location.href);
      } catch {
        throw syntaxError('EventSource', url);
      }
      this.#url = parsed.href;
      this.#init = init;
      this.#id = ++last;
      live.set(this.#id, (message) => this.#deliver(message));
      post({ op: 'sse-open', id: this.#id, url: this.#url });
    }

    get url() {
      return this.#url;
    }
    get readyState() {
      return this.#real ? this.#real.readyState : this.#state;
    }
    get withCredentials() {
      return Boolean(this.#init && this.#init.withCredentials);
    }

    addEventListener(type, listener, options) {
      super.addEventListener(type, listener, options);
      this.#forward(String(type));
    }

    close() {
      if (this.#real) return this.#real.close();
      if (this.#state === 2) return;
      this.#state = 2;
      live.delete(this.#id);
      post({ op: 'sse-close', id: this.#id });
    }

    // Events of `type` from the real EventSource, once per type.
    #forward(type) {
      if (this.#types.has(type)) return;
      this.#types.add(type);
      if (!this.#real) return;
      this.#real.addEventListener(type, (event) => {
        const copy =
          event instanceof MessageEvent
            ? new MessageEvent(type, { data: event.data, origin: event.origin, lastEventId: event.lastEventId })
            : new Event(type);
        this.dispatchEvent(copy);
      });
    }

    #deliver(message) {
      if (message.decision === 'real') {
        if (this.#state === 2) return;
        this.#real = new RealEventSource(this.#url, this.#init);
        live.delete(this.#id);
        const types = [...this.#types, 'open', 'message', 'error'];
        this.#types.clear();
        for (const type of types) this.#forward(type);
        return;
      }
      if (message.decision === 'fail') {
        if (this.#state === 2) return;
        this.#state = 2;
        live.delete(this.#id);
        this.dispatchEvent(new Event('error'));
        return;
      }
      if (message.decision === 'mock') {
        if (this.#state !== 0) return;
        this.#state = 1;
        this.dispatchEvent(new Event('open'));
        return;
      }
      if (message.event && this.#state === 1) {
        const event = message.event;
        if (event.id !== undefined && event.id !== null) this.#lastEventId = event.id;
        if (event.data === undefined || event.data === null) return;
        this.dispatchEvent(
          new MessageEvent(event.event || 'message', {
            data: event.data,
            origin: new URL(this.#url).origin,
            lastEventId: this.#lastEventId,
          }),
        );
      }
    }
  }
  constants(EventSource, ['CONNECTING', 'OPEN', 'CLOSED']);
  handlers(EventSource.prototype, ['open', 'message', 'error']);

  const replace = (name, cls) => {
    if (cls.name !== name) Object.defineProperty(cls, 'name', { value: name });
    Object.defineProperty(globalThis, name, { value: cls, configurable: true, writable: true });
  };
  if (typeof RealWebSocket === 'function') replace('WebSocket', WebSocket);
  if (typeof RealEventSource === 'function') replace('EventSource', EventSource);
  Object.defineProperty(globalThis, hook, {
    value: (message) => {
      if (message.off) {
        // Mocks are off: new sockets are real again.
        if (typeof RealWebSocket === 'function') replace('WebSocket', RealWebSocket);
        if (typeof RealEventSource === 'function') replace('EventSource', RealEventSource);
        return true;
      }
      if (message.on) {
        if (typeof RealWebSocket === 'function') replace('WebSocket', WebSocket);
        if (typeof RealEventSource === 'function') replace('EventSource', EventSource);
        return true;
      }
      const deliver = live.get(message.id);
      if (!deliver) return false;
      deliver(message);
      return true;
    },
  });
})();

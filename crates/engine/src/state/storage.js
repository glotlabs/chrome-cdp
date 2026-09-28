// An origin's storage, read and written the way Playwright's storageState
// has it (plan 07, milestone 7.4). Evaluates to an object with `collect`,
// `restore` and `seed`; it runs in chrome-cdp's isolated world, which shares
// the page's storage but none of its globals.
//
// IndexedDB values that JSON can hold as they are go in `value`/`key`;
// others (Dates, Maps, typed arrays, NaN, …) in `valueEncoded`/`keyEncoded`,
// in Playwright's serialisation (`{d: iso}`, `{a: [...], id}`, …), so files
// move between the two tools.
(() => {
  const kinds = {
    i8: Int8Array,
    ui8: Uint8Array,
    ui8c: Uint8ClampedArray,
    i16: Int16Array,
    ui16: Uint16Array,
    i32: Int32Array,
    ui32: Uint32Array,
    f32: Float32Array,
    f64: Float64Array,
    bi64: BigInt64Array,
    bui64: BigUint64Array,
  };
  const tag = (v) => Object.prototype.toString.call(v);

  function toBase64(bytes) {
    let binary = '';
    for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
    return btoa(binary);
  }

  function fromBase64(base64) {
    const binary = atob(base64);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
    return bytes;
  }

  function encode(value, seen) {
    if (value === undefined || typeof value === 'symbol') return { v: 'undefined' };
    if (value === null) return { v: 'null' };
    if (Number.isNaN(value)) return { v: 'NaN' };
    if (value === Infinity) return { v: 'Infinity' };
    if (value === -Infinity) return { v: '-Infinity' };
    if (Object.is(value, -0)) return { v: '-0' };
    if (typeof value === 'boolean' || typeof value === 'number' || typeof value === 'string') {
      return value;
    }
    if (typeof value === 'bigint') return { bi: value.toString() };
    if (typeof value !== 'object') return { v: 'undefined' };
    if (value instanceof Error) {
      return { e: { n: value.name, m: value.message, s: value.stack || '' } };
    }
    if (tag(value) === '[object Date]') return { d: value.toJSON() };
    if (tag(value) === '[object URL]') return { u: value.toJSON() };
    if (tag(value) === '[object RegExp]') return { r: { p: value.source, f: value.flags } };
    for (const [k, ctor] of Object.entries(kinds)) {
      if (value instanceof ctor) {
        return { ta: { b: toBase64(new Uint8Array(value.buffer, value.byteOffset, value.byteLength)), k } };
      }
    }
    if (value instanceof ArrayBuffer) return { ab: { b: toBase64(new Uint8Array(value)) } };
    const known = seen.map.get(value);
    if (known) return { ref: known };
    const id = ++seen.last;
    seen.map.set(value, id);
    if (value instanceof Map) {
      return { m: [...value].map(([k, v]) => ({ k: encode(k, seen), v: encode(v, seen) })), id };
    }
    if (value instanceof Set) return { s: [...value].map((v) => encode(v, seen)), id };
    if (Array.isArray(value)) return { a: value.map((v) => encode(v, seen)), id };
    return { o: Object.keys(value).map((k) => ({ k, v: encode(value[k], seen) })), id };
  }

  function decode(value, refs = new Map()) {
    if (value === undefined || value === null || typeof value !== 'object') return value;
    if ('ref' in value) return refs.get(value.ref);
    if ('v' in value) {
      return { null: null, NaN: NaN, Infinity: Infinity, '-Infinity': -Infinity, '-0': -0 }[value.v];
    }
    if ('d' in value) return new Date(value.d);
    if ('u' in value) return new URL(value.u);
    if ('bi' in value) return BigInt(value.bi);
    if ('e' in value) {
      const error = new Error(value.e.m);
      error.name = value.e.n;
      error.stack = value.e.s;
      return error;
    }
    if ('r' in value) return new RegExp(value.r.p, value.r.f);
    if ('ta' in value) return new kinds[value.ta.k](fromBase64(value.ta.b).buffer);
    if ('ab' in value) return fromBase64(value.ab.b).buffer;
    if ('a' in value) {
      const out = [];
      refs.set(value.id, out);
      for (const a of value.a) out.push(decode(a, refs));
      return out;
    }
    if ('o' in value) {
      const out = {};
      refs.set(value.id, out);
      for (const { k, v } of value.o) if (k !== '__proto__') out[k] = decode(v, refs);
      return out;
    }
    if ('m' in value) {
      const out = new Map();
      refs.set(value.id, out);
      for (const { k, v } of value.m) out.set(decode(k, refs), decode(v, refs));
      return out;
    }
    if ('s' in value) {
      const out = new Set();
      refs.set(value.id, out);
      for (const v of value.s) out.add(decode(v, refs));
      return out;
    }
    return value;
  }

  // Whether JSON holds `value` as it is: plain objects, arrays, strings,
  // finite numbers, booleans and null.
  function plain(value) {
    if (value === null || typeof value === 'string' || typeof value === 'boolean') return true;
    if (typeof value === 'number') return Number.isFinite(value) && !Object.is(value, -0);
    if (Array.isArray(value)) return value.every(plain);
    if (typeof value === 'object' && Object.getPrototypeOf(value) === Object.prototype) {
      return Object.values(value).every(plain);
    }
    return false;
  }

  function put(record, name, value) {
    if (plain(value)) record[name] = value;
    else record[name + 'Encoded'] = encode(value, { map: new Map(), last: 0 });
  }

  const request = (r) =>
    new Promise((resolve, reject) => {
      r.addEventListener('success', () => resolve(r.result));
      r.addEventListener('error', () => reject(r.error));
    });

  async function collectDB(info) {
    const db = await request(indexedDB.open(info.name));
    try {
      const names = [...db.objectStoreNames];
      if (!names.length) return { name: info.name, version: info.version, stores: [] };
      const tx = db.transaction(names, 'readonly');
      const stores = await Promise.all(
        names.map(async (name) => {
          const store = tx.objectStore(name);
          const keys = await request(store.getAllKeys());
          const values = await request(store.getAll());
          const records = keys.map((key, i) => {
            const record = {};
            if (store.keyPath === null) put(record, 'key', key);
            put(record, 'value', values[i]);
            return record;
          });
          const indexes = [...store.indexNames].map((indexName) => {
            const index = store.index(indexName);
            return {
              name: index.name,
              keyPath: typeof index.keyPath === 'string' ? index.keyPath : undefined,
              keyPathArray: Array.isArray(index.keyPath) ? index.keyPath : undefined,
              multiEntry: index.multiEntry,
              unique: index.unique,
            };
          });
          return {
            name,
            records,
            indexes,
            autoIncrement: store.autoIncrement,
            keyPath: typeof store.keyPath === 'string' ? store.keyPath : undefined,
            keyPathArray: Array.isArray(store.keyPath) ? store.keyPath : undefined,
          };
        }),
      );
      return { name: info.name, version: info.version, stores };
    } finally {
      db.close();
    }
  }

  // Replaces database `info`. The delete and the open are queued at once
  // and the records written in the upgrade transaction, so a page that
  // opens the database meanwhile waits until it is complete.
  function restoreDB(info) {
    indexedDB.deleteDatabase(info.name);
    const open = indexedDB.open(info.name, info.version);
    open.addEventListener('upgradeneeded', () => {
      for (const s of info.stores || []) {
        const store = open.result.createObjectStore(s.name, {
          autoIncrement: s.autoIncrement,
          keyPath: s.keyPathArray ?? s.keyPath,
        });
        for (const index of s.indexes || []) {
          store.createIndex(index.name, index.keyPathArray ?? index.keyPath, {
            unique: index.unique,
            multiEntry: index.multiEntry,
          });
        }
        for (const r of s.records || []) {
          store.add(r.value ?? decode(r.valueEncoded), r.key ?? decode(r.keyEncoded));
        }
      }
    });
    return request(open).then((db) => db.close());
  }

  function items(storage) {
    const out = [];
    for (let i = 0; i < storage.length; i++) {
      const name = storage.key(i);
      out.push({ name, value: storage.getItem(name) });
    }
    return out;
  }

  function fill(storage, list) {
    storage.clear();
    for (const { name, value } of list) storage.setItem(name, value);
  }

  // The storage of this document's origin; IndexedDB only when asked.
  async function collect(indexedDBToo) {
    const state = {
      origin: location.origin,
      localStorage: items(localStorage),
      sessionStorage: items(sessionStorage),
    };
    if (indexedDBToo) {
      const databases = await indexedDB.databases();
      state.indexedDB = await Promise.all(databases.filter((d) => d.name).map(collectDB));
    }
    return state;
  }

  // Replaces what `state` has: localStorage always (Playwright's files
  // always have it), sessionStorage and IndexedDB databases when given.
  function restoreSync(state) {
    fill(localStorage, state.localStorage || []);
    if (state.sessionStorage) fill(sessionStorage, state.sessionStorage);
  }

  async function restore(state) {
    restoreSync(state);
    await Promise.all((state.indexedDB || []).map(restoreDB));
  }

  // The seed a storage state leaves for an origin with no page open: gives
  // the first document of that origin its storage before the page's
  // scripts run, then tells chrome-cdp (binding `done`) to stop seeding.
  function seed(state, done) {
    if (location.origin !== state.origin) return;
    restoreSync(state);
    const report = globalThis[done];
    if (typeof report === 'function') report(state.origin);
    Promise.all((state.indexedDB || []).map(restoreDB)).catch((e) =>
      console.warn('chrome-cdp could not restore IndexedDB: ' + e),
    );
  }

  return { collect, restore, seed };
})()

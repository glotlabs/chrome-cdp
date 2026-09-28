// chrome-cdp test: determinism (plan 11.6). Runs in every document of a
// test before the page's own scripts. With a seed (two 32-bit halves),
// Math.random, crypto.getRandomValues and crypto.randomUUID give the same
// values in every run: each document starts the sequence again. With
// `still`, CSS animations end at once, transitions are off and the caret
// is hidden.
(function (seedLow, seedHigh, still) {
  try {
    if (seedLow !== null) seedRandom(seedLow, seedHigh);
    if (still) stopAnimations();
  } catch (e) {
    // Never break the page: a test without these is still a test.
  }

  function seedRandom(low, high) {
    // sfc32, warmed up so nearby seeds part ways.
    let a = low >>> 0, b = high >>> 0, c = 0x9e3779b9, d = 1;
    const next = () => {
      const t = (((a + b) >>> 0) + d) >>> 0;
      d = (d + 1) >>> 0;
      a = b ^ (b >>> 9);
      b = (c + (c << 3)) >>> 0;
      c = (c << 21) | (c >>> 11);
      c = (c + t) >>> 0;
      return t;
    };
    for (let i = 0; i < 15; i++) next();

    Math.random = function random() {
      return next() / 4294967296;
    };
    const crypto = globalThis.crypto;
    if (!crypto) return;
    const realGetRandomValues = crypto.getRandomValues.bind(crypto);
    const fill = (bytes) => {
      for (let i = 0; i < bytes.length; i++) bytes[i] = next() & 0xff;
    };
    crypto.getRandomValues = function getRandomValues(array) {
      const integers =
        ArrayBuffer.isView(array) &&
        !(array instanceof DataView) &&
        !(array instanceof Float32Array) &&
        !(array instanceof Float64Array);
      if (!integers || array.byteLength > 65536) {
        // The browser's own errors (TypeMismatchError, QuotaExceededError).
        return realGetRandomValues(array);
      }
      fill(new Uint8Array(array.buffer, array.byteOffset, array.byteLength));
      return array;
    };
    if (typeof crypto.randomUUID === "function") {
      crypto.randomUUID = function randomUUID() {
        const bytes = new Uint8Array(16);
        fill(bytes);
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
        return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
      };
    }
  }

  function stopAnimations() {
    // Animations run for 0s rather than not at all, so they end in their
    // final state and still fire animationend.
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(`*, *::before, *::after {
      transition: none !important;
      animation-duration: 0s !important;
      animation-delay: 0s !important;
      animation-iteration-count: 1 !important;
      caret-color: transparent !important;
      scroll-behavior: auto !important;
    }`);
    const adopt = () => {
      if (!document.adoptedStyleSheets.includes(sheet)) {
        document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
      }
    };
    adopt();
    // A page that sets adoptedStyleSheets itself may drop the sheet.
    document.addEventListener("DOMContentLoaded", adopt);
  }
})

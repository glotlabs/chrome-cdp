// chrome-cdp's helper bundle (plan 04, milestone 4.4): loaded once into
// chrome-cdp's isolated world of every document (with
// Page.addScriptToEvaluateOnNewDocument) as `globalThis.__ccdp`, where
// page scripts cannot reach or change it. Built into `ccdp.js` by
// `cargo xtask build-js`; the engine embeds that file.

import { one, query, describe } from './locator';
import { walk } from './walker';

declare global {
  // eslint-disable-next-line no-var
  var __ccdp: typeof api | undefined;
}

const api = { version: 1, query, one, describe, walk };

if (!globalThis.__ccdp) globalThis.__ccdp = api;

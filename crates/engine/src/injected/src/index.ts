// chrome-cdp's helper bundle (plan 04, milestone 4.4): loaded once into
// chrome-cdp's isolated world of every document (with
// Page.addScriptToEvaluateOnNewDocument) as `globalThis.__ccdp`, where
// page scripts cannot reach or change it. It holds the locator engine,
// the DOM walker and what the read commands (4.5) take from elements.
// Built into `ccdp.js` by `cargo xtask build-js`; the engine embeds that
// file.

import { one, query, describe } from './locator';
import { html, markdown, text, tidy, value } from './read';
import { walk } from './walker';
import { visible } from './dom';

declare global {
  // eslint-disable-next-line no-var
  var __ccdp: typeof api | undefined;
}

const api = { version: 1, query, one, describe, walk, visible, text, tidy, html, value, markdown };

if (!globalThis.__ccdp) globalThis.__ccdp = api;

// chrome-cdp's helper bundle (plan 04, milestone 4.4): loaded once into
// chrome-cdp's isolated world of every document (with
// Page.addScriptToEvaluateOnNewDocument) as `globalThis.__ccdp`, where
// page scripts cannot reach or change it. It holds the locator engine,
// the DOM walker, what the read commands (4.5) take from elements, the
// actionability checks (5.1), what mouse actions ask the page (5.2), and
// what keyboard and form actions ask it (5.3), where focus went for
// the post-action report (5.5), what `expect` checks (plan 09), and the
// stable locators `history --as-scenario` writes for refs (plan 11), and
// what `why` explains, what `layout` checks, the forms `forms` and
// `fill-form` list, what `compare` names and what `explore` follows
// (plan 16), what `audit` reads of a document (plan 17), and the
// fingerprints `test` keeps of the elements its steps found, and the
// recorder `record start` runs in every document (plan 18).
// Built into `ccdp.js` by `cargo xtask build-js`; the engine embeds that
// file.

import { one, query, describe } from './locator';
import { html, markdown, text, tidy, value } from './read';
import { walk } from './walker';
import { contains, visible } from './dom';
import { checkActionable, covering } from './actionable';
import { draggable, scrollSettled, scrollState } from './pointer';
import { caretToEnd, checkState, fieldValue, fillPrepare, focused, hasFocus, reaches, selectOptions } from './form';
import { acted, focusMark, focusMoved } from './report';
import { expectElements, expectNamed, expectPage } from './expect';
import { a11y, a11yRefocus } from './a11y';
import { auditPage } from './audit';
import { stable } from './stable';
import { clipped, explainActionable, scrolledSince, scrollMark } from './why';
import {
  contentBox, hideText, layoutCheck, pageExtent, scrollPage, showText, stretchText, textNames, unstretchText,
} from './layout';
import { formsList, pickOption } from './forms';
import { elementsAt } from './compare';
import { exploreCandidates } from './explore';
import { fingerprint, healCandidates, healStable } from './fingerprint';
import { record, recordOff } from './record';

declare global {
  // eslint-disable-next-line no-var
  var __ccdp: typeof api | undefined;
}

const api = {
  version: 1,
  query,
  one,
  describe,
  walk,
  visible,
  contains,
  text,
  tidy,
  html,
  value,
  markdown,
  checkActionable,
  covering,
  scrollState,
  scrollSettled,
  draggable,
  focused,
  hasFocus,
  caretToEnd,
  fillPrepare,
  fieldValue,
  selectOptions,
  checkState,
  reaches,
  focusMark,
  acted,
  focusMoved,
  expectElements,
  expectPage,
  expectNamed,
  a11y,
  a11yRefocus,
  auditPage,
  stable,
  explainActionable,
  clipped,
  scrollMark,
  scrolledSince,
  layoutCheck,
  textNames,
  hideText,
  showText,
  stretchText,
  unstretchText,
  scrollPage,
  pageExtent,
  contentBox,
  formsList,
  pickOption,
  elementsAt,
  exploreCandidates,
  fingerprint,
  healCandidates,
  healStable,
  record,
  recordOff,
};

if (!globalThis.__ccdp) globalThis.__ccdp = api;

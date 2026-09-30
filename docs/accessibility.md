# Accessibility checks: `expect --a11y`

`chrome-cdp expect --a11y` checks the page and each of its iframes
(cross-site ones too) against a built-in set of accessibility rules, and
fails with the elements that break one, each with a ref:

```sh
chrome-cdp expect --a11y                  # every rule (WCAG level AA)
chrome-cdp expect --a11y --level A        # only the level A rules
chrome-cdp expect '#checkout' --a11y      # one element's subtree (no page rules)
```

```
error: expect failed: --a11y
  expected: no violations of 26 accessibility rules (WCAG level AA)
  actual: 2 violations of 2 rules
  aria-valid-attr-value: button "Bold" [e4] has an aria-* attribute with a wrong value: aria-pressed="yes": not "true", "false", "mixed" or "undefined" (WCAG 4.1.2 A)
  heading-order: heading "Five" [e7] skips a heading level: level 3 → 5 (WCAG 1.3.1 A)
  hint: chrome-cdp html e4 --outer   (the first one's markup, to fix)
```

It looks once, without retrying: `wait` for the page to settle first.
A rule lists 5 elements in the text; `--json` has the rest, with each
rule's WCAG criterion, level and count.

The rules are chrome-cdp's own, not axe-core (plan 09 explains why), and
use axe's rule ids, so results read the same. Where only Chrome knows the
answer, a rule asks Chrome's accessibility tree: whether a table is a
data table or a layout table, and what a heading's accessible name is.

## Rules

| Rule | WCAG | Level | Flags |
|---|---|---|---|
| `document-title` | 2.4.2 | A | a page with no `<title>` |
| `html-has-lang` | 3.1.1 | A | `<html>` with no `lang` |
| `image-alt` | 1.1.1 | A | an `<img>` with no `alt` and no name |
| `input-image-alt` | 1.1.1 | A | an `<input type=image>` with no alt text |
| `button-name` | 4.1.2 | A | a button with no name |
| `link-name` | 2.4.4 | A | a link with no name |
| `label` | 4.1.2 | A | a form field with no label |
| `frame-title` | 4.1.2 | A | an iframe with no title |
| `aria-hidden-focus` | 4.1.2 | A | a focusable element inside `aria-hidden="true"` |
| `aria-valid-role` | 4.1.2 | A | a `role` that is not an ARIA 1.2 role (DPUB and Graphics roles are fine; abstract roles are not) |
| `aria-valid-attr` | 4.1.2 | A | an `aria-*` attribute that does not exist (`aria-lable`) |
| `aria-valid-attr-value` | 4.1.2 | A | a value an attribute does not take (`aria-pressed="yes"`), or an id no element has in `aria-labelledby`, `aria-describedby`, `aria-controls` and the other id attributes (`aria-controls` of a collapsed control, `aria-expanded="false"`, may name what is not there yet) |
| `aria-required-attr` | 4.1.2 | A | a `role` without an attribute it needs (`role=checkbox` without `aria-checked`, `role=slider` without `aria-valuenow`, `role=combobox` without `aria-expanded`); native elements that have the state themselves pass |
| `aria-required-children` | 1.3.1 | A | a `role` that owns an element it does not allow (a `tablist` with a `button` in it); looks through `none`, `presentation` and generic elements; an empty one passes |
| `aria-required-parent` | 1.3.1 | A | a `role` outside the role it needs (a `tab` not in a `tablist`, an `<li>` not in a list) |
| `duplicate-id-referenced` | 4.1.2 | A | an element with the id of an earlier one, when a `<label for>` or an `aria-*` attribute refers to that id |
| `list` | 1.3.1 | A | a `<ul>` or `<ol>` with children other than `<li>`, `<script>` and `<template>` |
| `table-headers` | 1.3.1 | A | a data table (Chrome does not take it for a layout table) with no header cells |
| `heading-order` | 1.3.1 | A | a heading that skips a level going down (h2 → h4) |
| `empty-heading` | 1.3.1 | A | a heading with no name (by Chrome) |
| `landmark-one-main` | 1.3.1 | A | a page with no visible `main` landmark, or more than one |
| `meta-viewport` | 1.4.4 | AA | a viewport meta tag that stops zooming |
| `color-contrast` | 1.4.3 | AA | text with too little contrast against its declared background, faded by `opacity` (text over images and text with a `text-shadow` are skipped) |
| `autocomplete-valid` | 1.3.5 | AA | an `autocomplete` value HTML does not define (`nmae`, `work name`) |
| `target-size` | 2.5.8 | AA | a pointer target under 24×24 CSS px with another target within a 24px circle on its centre; a target in a line of text and a form control the browser sizes (its own font, not restyled) are exempt |
| `focus-visible` | 2.4.7 | AA | an element that looks the same with keyboard focus as without |

Page rules (`document-title`, `html-has-lang`, `landmark-one-main`,
`meta-viewport`) check the page's document only, not its iframes', and
not with a LOCATOR.

## `focus-visible` focuses elements

To see what focus changes, `focus-visible` focuses each focusable element
(at most 300) in turn, as a keyboard would, and compares its outline,
box shadow, borders, colours, underline, `::before` and `::after`, its
nearest ancestors (`:focus-within`) and its first descendants before and
after. The page's own `focus` and `blur` handlers run. Focus goes back
where it was afterwards. `--level A` leaves it out.

## Not axe-core

The rule set is shaped so that axe-core could run behind a flag later
(`--a11y-engine axe`); that waits on the project's license choice.

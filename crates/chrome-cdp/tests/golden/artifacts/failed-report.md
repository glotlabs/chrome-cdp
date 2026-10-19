# Failed: fails on a missing element

`scenarios/fail/fails.cdp:8`

## Failing step

`scenarios/fail/fails.cdp:9:3`:

    expect text=Nowhere visible --timeout <t>

```
error: expect failed: text=Nowhere visible
  expected: visible
  actual: no element matches (after <t>, <n> retries)
  hint: see <root>/e2e/.results/scenarios/fail/fails.cdp/fails-on-a-missing-element/snapshot.txt   (the page when the test failed; the test's page is closed: --debug keeps it open)
```

## Last steps

10 steps ran; the 3 before the failing one.

```
✓ scenarios/fail/fails.cdp:8 locale en-US (<t>) [setup]
✓ scenarios/fail/fails.cdp:8 tz UTC (<t>) [setup]
✓ scenarios/fail/fails.cdp:3:3 goto /e2e/shop.html (<t>) [before]
✗ scenarios/fail/fails.cdp:9:3 expect text=Nowhere visible --timeout <t> (<t>)
```

## Page

`snapshot -i` when it failed (4 of 4 lines):

```
page <tab> "Shop" <site>/e2e/shop.html
- form
  - textbox "New todo" [e1]
  - button "Add" [e2]
```

## Console errors

None.

## Failed and unmatched requests

None.

## Files

- <root>/e2e/.results/scenarios/fail/fails.cdp/fails-on-a-missing-element/failure.png: the page when it failed
- <root>/e2e/.results/scenarios/fail/fails.cdp/fails-on-a-missing-element/snapshot.txt: all of `snapshot -i`
- <root>/e2e/.results/scenarios/fail/fails.cdp/fails-on-a-missing-element/steps.jsonl: every step, with its output
- <root>/e2e/.results/scenarios/fail/fails.cdp/fails-on-a-missing-element/requests.txt: every request and the mock that answered it
- <root>/e2e/.results/scenarios/fail/fails.cdp/fails-on-a-missing-element/console.txt: the console of every tab

Run it again: `chrome-cdp test scenarios/fail/fails.cdp:8`

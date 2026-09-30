# Failed: buys a ticket

`scenarios/heal/tickets.cdp:4`

## Failing step

`scenarios/heal/tickets.cdp:6:3`:

    fill 'label="Name"' Ada

```
error: no element matches label="Name"
  waited: <t>
  likely the same element now: role=textbox[name="Full name"] (confidence 0.84: same testid, role, container and tag; similar name)
  hint: run it with --heal to use it (chrome-cdp test scenarios/heal/tickets.cdp:4 --heal), or change the locator
```

## Last steps

10 steps ran; the 3 before the failing one.

```
✓ scenarios/heal/tickets.cdp:4 locale en-US (<t>) [setup]
✓ scenarios/heal/tickets.cdp:4 tz UTC (<t>) [setup]
✓ scenarios/heal/tickets.cdp:5:3 goto /e2e/heal.html?v=${var:heal_version} (<t>)
✗ scenarios/heal/tickets.cdp:6:3 fill 'label="Name"' Ada (<t>)
```

## Likely the same element

The step's locator finds no element now (or several). The element its fingerprint (e2e/fingerprints) matches best:

    role=textbox[name="Full name"]

confidence 0.84 (high): same testid, role, container and tag; similar name.
Run the test with --heal to use it, or change the locator.

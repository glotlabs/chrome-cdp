<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp select`

Choose options of a &lt;select&gt; by value or label

```text
chrome-cdp select [OPTIONS] <LOCATOR> <VALUE>...
```

Arguments:

- `<LOCATOR>`: The &lt;select&gt;: a ref (e12) or a locator that matches one
- `<VALUE>`: Options' values or labels

Options:

- `--force`: Skip the checks (visible, stable, enabled)
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)

```text
Each value matches an option's value, else its label (then in any case);
several values need a <select multiple>. Waits until the options are
there, then selects them and fires input and change. For a custom
dropdown (role=combobox or listbox without a <select>), click it, then
click the option. Needs a running session.

Examples:
  chrome-cdp select e7 blue
  chrome-cdp select 'label=Country' 'Norway'
  chrome-cdp select '#toppings' cheese olives
```


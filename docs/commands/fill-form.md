<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp fill-form`

Fill a form from a JSON object in one step: keys name fields, values pick the action

```text
chrome-cdp fill-form [OPTIONS] [LOCATOR]
```

Arguments:

- `<LOCATOR>`: The form: a ref (e12) or a locator that matches it or an element in it; the page's only form if left out

Options:

- `--values <JSON>`: The values, a JSON object of field names to values; `-` reads it from stdin
- `--file <FILE>`: Read the values from this JSON file
- `--submit`: Then submit the form: click its first submit button, or press Enter in the last field filled
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)

```text
Each key names a field by its label (ignoring case, spaces at the ends
and a trailing * or :), else its name, id or placeholder, else a part of
its label only it has. The value picks the action: a string or number
fills a text field (or a date, colour, range), selects a <select>'s
option (an array for several), or checks the radio of a group with that
label or value; true or false checks or unchecks a checkbox or switch; a
path, or an array of paths, uploads to a file input (relative to this
directory); a custom combobox is filled, then the option with that name
is clicked when its listbox shows one; a listbox's options with those
names are clicked. Fields are filled in the page's order, each as its
own command does (waiting until it can act), and a field that appears
after an earlier choice (a region after a country) is matched too.
A value of the wrong type fails with BadArgs before anything is filled;
keys that match no field, or several, fail with NotFound after the others
are filled, naming the fields. Without a LOCATOR the page must have one
form (else Ambiguous, with the forms' refs). --submit then clicks the
form's first submit button, or presses Enter in the last field filled.
One action report covers the whole form. `forms` shows the names to use.
Needs a running session.

Examples:
  chrome-cdp fill-form --values '{"Email": "ada@example.com", "Password": "s3cret"}' --submit
  chrome-cdp fill-form e10 --file order.json
  chrome-cdp fill-form 'role=form[name="Checkout"]' --values - < order.json
```


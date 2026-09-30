<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp forms`

List the page's forms: each field's name, kind, state, value and validation message

```text
chrome-cdp forms [OPTIONS] [LOCATOR]
```

Arguments:

- `<LOCATOR>`: The form: a ref (e12) or a locator that matches it or an element in it; every form if left out

Options:

- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Lists every <form>, role=form and role=search, in every frame, and last
the fields outside any form; with a LOCATOR, only the form it is or is
inside (else the forms in it, else the fields in it). Each field is named
as the snapshot names it, with its ref, then its kind (an input's type:
email, password, date, submit), required, disabled, readonly, its value
(never a password's; a file input's files), the first 10 options of a
select, listbox or combobox, and why it is invalid: its validation
message (not for a required field that is only empty), else the text
aria-errormessage or aria-describedby points to. Radios are one line per
group. A form's header says where it submits and how many required
fields are empty; -q prints that count. Needs a running session.

Examples:
  chrome-cdp forms
  chrome-cdp forms e10
  chrome-cdp forms 'role=search' --json
```


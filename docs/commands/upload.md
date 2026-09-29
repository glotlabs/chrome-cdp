<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp upload`

Put files into a file input, or the file chooser a button opens

```text
chrome-cdp upload [OPTIONS] <LOCATOR> [FILE]...
```

Arguments:

- `<LOCATOR>`: The file input, its label, or a button that opens a file chooser: a ref (e12) or a locator that matches one
- `<FILE>`: The files to upload

Options:

- `--clear`: Leave the input with no files
- `--force`: Skip the checks (enabled; for a button also visible, stable and not covered)
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)

```text
A file input gets the files at once, even a hidden one (most upload
widgets hide theirs behind a button), and the page gets input and
change as after a user's choice; a <label> of one works too. Anything
else is clicked like click does, and the files go to the file chooser
that opens. Paths are relative to the current directory. --clear leaves
the input with no files. Needs a running session.

Examples:
  chrome-cdp upload e5 ./avatar.png
  chrome-cdp upload 'label=Attachments' a.pdf b.pdf
  chrome-cdp upload 'role=button[name="Choose files"]' report.csv
  chrome-cdp upload e5 --clear
```


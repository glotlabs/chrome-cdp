<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp upload`

Put files into a file input, the file chooser a button opens, or a drop zone

```text
chrome-cdp upload [OPTIONS] <LOCATOR> [FILE]...
```

Arguments:

- `<LOCATOR>`: The file input, its label, a button that opens a file chooser, or (with --drop) a drop zone: a ref (e12) or a locator that matches one
- `<FILE>`: The files to upload, or one directory for a folder input

Options:

- `--clear`: Leave the input with no files
- `--drop`: Drop the files on the element with drag events, for a drop zone with no file input
- `--force`: Skip the checks (enabled; for a button or drop zone also visible, stable and not covered)
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)

```text
A file input gets the files at once, even a hidden one (most upload
widgets hide theirs behind a button), and the page gets input and
change as after a user's choice; a <label> of one works too. Anything
else is clicked like click does, and the files go to the file chooser
that opens. --drop drops the files on the element instead, as a user
drags them in from the desktop (dragenter, dragover and drop, with the
files in dataTransfer.files), for drop zones with no file input; it
fails if the page takes no drop there. A folder input (webkitdirectory)
takes one directory instead: its files, the page seeing each one's path
in it (webkitRelativePath, e.g. photos/2024/a.jpg). Paths are relative
to the current directory. --clear leaves the input with no files. Needs
a running session.

Examples:
  chrome-cdp upload e5 ./avatar.png
  chrome-cdp upload 'label=Attachments' a.pdf b.pdf
  chrome-cdp upload 'label=Folder' ./photos        # a webkitdirectory input
  chrome-cdp upload 'role=button[name="Choose files"]' report.csv
  chrome-cdp upload 'text=Drop files here' photo.jpg --drop
  chrome-cdp upload e5 --clear
```


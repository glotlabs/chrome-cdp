<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp eval`

Run JavaScript in the page and print its value

```text
chrome-cdp eval [OPTIONS] <JS>
```

Arguments:

- `<JS>`: The script: an expression or statements; `-` reads it from stdin

Options:

- `--in <LOCATOR>`: Bind this element as `el` (a ref such as e12, or any locator); the script runs in its frame
- `--frame <FRAME>`: Run in a frame of the tab: its name attribute, a part of its URL, or its id
- `--world <WORLD>`: The page's own world (its globals), or chrome-cdp's isolated one (the same DOM, where page scripts cannot see or change what the script does) (one of `main`, `isolated`; default `main`)
- `--sourcemaps`: Resolve an exception's stack frames with source maps (always on for localhost pages)
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit). With --json, long strings and arrays in `data` are shortened instead (default `20000`)

```text
The script runs like a line typed into DevTools' console: statements are
fine and the last one's value is printed, `await` works at the top level,
and it counts as a user gesture. A promise is awaited; a function
(`() => …`, `el => …`) is called, with the --in element. Strings print as
they are; other values print as JSON (DOM nodes as `button#id.class`).
The script's `let`s and `const`s do not stay behind in the page. An
exception exits 1 with its message and stack. Needs a running session.

Examples:
  chrome-cdp eval document.title
  chrome-cdp eval 'document.querySelectorAll("a").length'
  chrome-cdp eval --in e12 'el.value'
  chrome-cdp eval --in '#price' 'getComputedStyle(el).color'
  chrome-cdp eval 'await fetch("/api/me").then((r) => r.json())'
  chrome-cdp eval --frame checkout 'location.href'
  chrome-cdp eval - <<'EOF'
    const rows = [...document.querySelectorAll('tr')];
    rows.map((r) => r.innerText)
  EOF
```


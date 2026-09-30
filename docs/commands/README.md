<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# Command reference

Every chrome-cdp command, by task, with its options and examples: the
same text as `chrome-cdp help <command>`. The guides in
[docs/](../README.md) explain how they fit together.

## Browser and sessions

| Command | What it does |
|---|---|
| [`open`](open.md) | Start the session's browser (or check the running one) and optionally open a URL |
| [`connect`](connect.md) | Attach the session to a browser that is already running |
| [`close`](close.md) | Close the session: its daemon, and the browser it launched |
| [`status`](status.md) | Show the session's state, browser, tabs and uptime |
| [`sessions`](sessions.md) | List the running sessions |
| [`extensions`](extensions.md) | List, load, reload or uninstall unpacked extensions; open an extension's popup; read or write its chrome.storage |
| [`install`](install.md) | Download Chrome for Testing into chrome-cdp's cache |
| [`uninstall`](uninstall.md) | Remove a browser installed by `chrome-cdp install` |

## Tabs and navigation

| Command | What it does |
|---|---|
| [`goto`](goto.md) | Load a URL in the current tab and wait for it |
| [`back`](back.md) | Go back one page in the tab's history |
| [`forward`](forward.md) | Go forward one page in the tab's history |
| [`reload`](reload.md) | Load the current page again |
| [`tabs`](tabs.md) | List the session's tabs; `*` marks the active one |
| [`tab`](tab.md) | Switch to a tab, or open or close one |
| [`frames`](frames.md) | List the tab's frames and workers as a tree |
| [`context`](context.md) | Make, list or close browser contexts: isolated cookies, storage, routes and mocks in one browser |
| [`wait`](wait.md) | Wait for the page: an element, text, the URL, network idle, a request, … |

## Reading the page

| Command | What it does |
|---|---|
| [`snapshot`](snapshot.md) | Show the page as an accessibility tree, with refs (e12) to act on |
| [`find`](find.md) | Search the page for elements by role, name or text; shows refs |
| [`locator`](locator.md) | Print the locator a test should use for one element (a ref's stable locator) |
| [`text`](text.md) | Print the visible text of the page, or of every element a locator matches |
| [`markdown`](markdown.md) | Print the page's readable content (or an element's) as Markdown |
| [`html`](html.md) | Print the HTML of the document or of an element, pretty-printed |
| [`attr`](attr.md) | Print an attribute of an element |
| [`value`](value.md) | Print a field's current value: input, textarea, select, editable text |
| [`count`](count.md) | Count the elements a locator matches (0 is not an error) |
| [`box`](box.md) | Print an element's box in the viewport: x, y, width and height |
| [`source`](source.md) | Print the file and line an element came from, and its components |

## Acting

| Command | What it does |
|---|---|
| [`click`](click.md) | Click an element: waits until it can be clicked, then a real mouse click |
| [`dblclick`](dblclick.md) | Double-click an element (click --count 2) |
| [`hover`](hover.md) | Move the mouse over an element (menus, tooltips) |
| [`tap`](tap.md) | Tap an element with a touch (needs touch emulation) |
| [`fill`](fill.md) | Replace a field's content with text, in one input event |
| [`type`](type.md) | Type text key by key, as a user does (fill is faster) |
| [`press`](press.md) | Press keys or shortcuts: Enter, Control+A, "Mod+A Backspace" |
| [`select`](select.md) | Choose options of a &lt;select&gt; by value or label |
| [`check`](check.md) | Check a checkbox, radio button or switch (no-op if it is already) |
| [`uncheck`](uncheck.md) | Uncheck a checkbox or switch (no-op if it is not checked) |
| [`focus`](focus.md) | Give an element focus (without clicking it) |
| [`blur`](blur.md) | Take focus away from an element (blur and change fire) |
| [`upload`](upload.md) | Put files into a file input, the file chooser a button opens, or a drop zone |
| [`drag`](drag.md) | Drag an element onto another: mouse events, or HTML5 drag and drop |
| [`scroll`](scroll.md) | Scroll the page or an element with the mouse wheel |
| [`mouse`](mouse.md) | Low-level mouse events at viewport coordinates |
| [`keyboard`](keyboard.md) | Hold keys down across commands (Shift while clicking, …) |
| [`dialog`](dialog.md) | Answer the JavaScript dialog (confirm, prompt, …) the page waits on |
| [`clipboard`](clipboard.md) | Read or write the clipboard (text, HTML or a PNG), through the page |

## Verifying

| Command | What it does |
|---|---|
| [`expect`](expect.md) | Assert something about the page, retrying until it holds: an element's state, text, value, count, … |
| [`expect-snapshot`](expect-snapshot.md) | Compare the page's snapshot with a stored text baseline; a diff shows what changed |
| [`expect-screenshot`](expect-screenshot.md) | Compare a screenshot with a stored PNG baseline, pixel by pixel (experimental) |
| [`mark`](mark.md) | Name this point in the tab's console and network logs, for expect --no-errors --since |
| [`init`](init.md) | Set up a project for chrome-cdp: `init e2e` scaffolds E2E tests with mocks |
| [`test`](test.md) | Run the project's E2E scenarios, each test in a fresh browser context with its mocks |
| [`lint`](lint.md) | Check scenario and mock files for mistakes without running them |
| [`fmt`](fmt.md) | Format .cdp scenario files and mock files in place; --check only reports |
| [`history`](history.md) | The commands the session ran; --as-scenario writes them as a test |

## Observing and debugging

| Command | What it does |
|---|---|
| [`screenshot`](screenshot.md) | Save an image of the page, the whole page or one element |
| [`pdf`](pdf.md) | Print the page to a PDF file (headless browsers only) |
| [`console`](console.md) | Show what the page logged: console calls, uncaught exceptions and the browser's messages |
| [`errors`](errors.md) | Show the page's errors: uncaught exceptions, console.error and failed loads from its own origin |
| [`network`](network.md) | Show the page's requests, or one in full, or clear the log |
| [`downloads`](downloads.md) | List the files the session's pages downloaded |
| [`eval`](eval.md) | Run JavaScript in the page and print its value |
| [`perf`](perf.md) | Show how fast the page is: load timing, Web Vitals, blocking time, memory |
| [`coverage`](coverage.md) | Measure how much of the page's JavaScript and CSS is unused |
| [`har`](har.md) | Record the page's requests to a HAR file |
| [`trace`](trace.md) | Record a Chrome performance trace, for DevTools or Perfetto |
| [`highlight`](highlight.md) | Point out an element in the browser window, for a human watching |
| [`inspect`](inspect.md) | Open DevTools for the tab, in its own window (headed browsers) |

## Mocking and page scripts

| Command | What it does |
|---|---|
| [`route`](route.md) | Mock, block or change the page's requests |
| [`block`](block.md) | Block requests: by URL, or ads, images, fonts or media |
| [`routes`](routes.md) | List the routing rules and their hits, or the requests they saw |
| [`unroute`](unroute.md) | Remove routing rules |
| [`mock`](mock.md) | Answer the page's requests from mock files: use, list, log, explain |
| [`http`](http.md) | Send an HTTP request from the browser context, outside the page |
| [`init-script`](init-script.md) | Run scripts in every page before its own: add, list or remove |
| [`expose`](expose.md) | Give pages a function whose calls show up in the console |
| [`unexpose`](unexpose.md) | Take exposed functions away from pages |

## Emulation

| Command | What it does |
|---|---|
| [`emulate`](emulate.md) | Make pages look like a phone, tablet or desktop; list or reset |
| [`viewport`](viewport.md) | Set the viewport: WIDTHxHEIGHT, pixel ratio, mobile, touch |
| [`geo`](geo.md) | Set where navigator.geolocation says the device is |
| [`tz`](tz.md) | Set the time zone pages see (an IANA name) |
| [`locale`](locale.md) | Set the locale: Intl formats, navigator.language, Accept-Language |
| [`ua`](ua.md) | Set the user agent pages and servers see |
| [`media`](media.md) | Emulate CSS media: print, dark mode, reduced motion, forced colors |
| [`offline`](offline.md) | Take the network away (requests fail, navigator.onLine is false) |
| [`throttle`](throttle.md) | Slow the network: a preset, or latency and bandwidth |
| [`cpu-throttle`](cpu-throttle.md) | Slow the CPU down by a factor (4 = four times slower) |
| [`time`](time.md) | Freeze or pause the time pages see, move it on, or give the real one back |
| [`permissions`](permissions.md) | List, grant, deny or reset permissions (geolocation, camera, …) |

## Cookies and storage

| Command | What it does |
|---|---|
| [`cookies`](cookies.md) | List, set or clear the browser's cookies |
| [`storage`](storage.md) | Read or change a page's localStorage or sessionStorage |
| [`state`](state.md) | Save or load cookies and storage, e.g. a login (Playwright's format) |

## Scripts and raw CDP

| Command | What it does |
|---|---|
| [`batch`](batch.md) | Run commands from stdin, one per line, in one call |
| [`run`](run.md) | Run a .cdp script: commands one per line, with ${var} substitution |
| [`convert`](convert.md) | Translate a Playwright test file into a .cdp scenario (best effort) |
| [`cdp`](cdp.md) | Make one raw CDP call on the session's browser or current page |

## Help and tooling

| Command | What it does |
|---|---|
| [`help`](help.md) | Show help: an overview, every command by task, a command in full, or a topic |
| [`skill`](skill.md) | Print a SKILL.md that teaches an agent to use chrome-cdp, or install it |
| [`completions`](completions.md) | Print a shell completion script: bash, zsh, fish, elvish or powershell |
| [`schema`](schema.md) | Print the JSON Schema of a command's --json output |
| [`doctor`](doctor.md) | Check the setup: Chrome, its libraries and sandbox, the session dir, macOS quarantine and version skew; prints a fix for each problem |
| [`upgrade`](upgrade.md) | Replace this binary with the newest release (or --version's) |

## Global options

Every command takes these. Most also have an environment variable, and
the config files can set some of them (`chrome-cdp help config`).

- `--json`: Print one JSON object on stdout: {"ok":true,"data":…} or {"ok":false,"error":…} (env `CHROME_CDP_JSON`)
- `-q, --quiet`: Print only the essential result, for `$(…)` capture (env `CHROME_CDP_QUIET`)
- `--session <NAME>`: Browser session to use; each session has its own daemon and Chrome (default `default`; env `CHROME_CDP_SESSION`)
- `--timeout <DURATION>`: Give up after this long, e.g. 500ms, 30s, 2m. Default: 5s for `expect`; 10s for `wait`, actions, the read commands (`text`, `attr`, …) and `screenshot LOC`; 30s for navigation and everything else (env `CHROME_CDP_TIMEOUT`)
- `--tab <ID>`: The tab page commands act on, e.g. t2 (see `chrome-cdp tabs`). Default: the active tab (env `CHROME_CDP_TAB`)
- `--context <ID>`: The browser context to act in, e.g. c1 (see `chrome-cdp context`): its tabs, cookies, routes and mocks. Default: the session's own (env `CHROME_CDP_CONTEXT`)
- `-v, --verbose`: Log to stderr: -v info, -vv debug, -vvv trace. CHROME_CDP_LOG (e.g. cdp=trace) overrides it

## Help topics

`chrome-cdp help <topic>` explains an idea the commands share:

- `locators`: how commands name elements: refs, CSS, text=, role=, &gt;&gt; chains, frames
- `refs`: refs (e12): where they come from, how long they last, stale refs
- `waiting`: what commands wait for by themselves, and `wait` for the rest
- `errors`: error kinds, exit codes and hints
- `config`: config files, environment variables and which wins
- `scripts`: batch and run: many commands in one call

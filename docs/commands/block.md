<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp block`

Block requests: by URL, or ads, images, fonts or media

```text
chrome-cdp block [OPTIONS] [PATTERN]...
```

Arguments:

- `<PATTERN>`: URLs to block: globs (`**/*.mp4`), /regexes/, or parts of URLs

Options:

- `--images`: Block images
- `--fonts`: Block web fonts
- `--media`: Block audio and video
- `--ads`: Block well-known ad and tracking hosts

```text
Each PATTERN (as for `route`) and each preset becomes a rule that fails its
requests as blocked by the client. Presets: --images, --fonts, --media
(by resource type) and --ads (well-known ad and tracking hosts). Blocking
images, fonts and media makes heavy pages load faster. `routes` lists the
rules, `unroute` removes them.

Examples:
  chrome-cdp block --images --fonts
  chrome-cdp block --ads
  chrome-cdp block '**/*.mp4' cdn.example.com/tracker.js
```


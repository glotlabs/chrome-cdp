<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp issues`

Show Chrome's own findings about the page, as in DevTools' Issues panel: mixed content, CORS and CSP blocks, cookie problems, deprecations, form problems

```text
chrome-cdp issues [OPTIONS]
```

Options:

- `--since <WHEN>`: Where to start: `last` (what this command has not shown yet), an issue number such as 12 (0 for all), a duration such as 30s (raised in the last 30 seconds), or a mark (see `mark`) (default `last`)
- `--grep <REGEX>`: Only issues whose rule, message or URL matches this regular expression
- `--limit <N>`: Show at most this many issues, the newest (0 = all); with --all-tabs, per tab (default `50`)
- `-f, --follow`: Keep printing new issues as they come, until Ctrl-C (or --timeout, if given)
- `--min-severity <SEVERITY>`: The least severe issues shown (one of `error`, `warning`, `info`; default `warning`)
- `--all-tabs`: Every tab of the session (or context), each from its own cursor
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Every tab records Chrome's issues from the moment it opens, from its
frames (iframes of other sites too) and its workers. Chrome raises one per
occurrence; the same rule about the same thing is one issue with a count
(×4), within one page load. Like `console`, only issues this command has
not shown before are printed, and one raised again is shown again. Form
issues (labels, autocomplete) are checked when this runs.

Each line has a severity (E error, W warning, I info), a stable rule id
(`mixed-content-blocked`, `cookie-exclude-same-site-unspecified-treated-as-lax`),
a message, and what it is about: an element's ref, a script's line, or the
request's id in the network log (`network show <id>`). --json has each
issue's `key`, which stays the same across runs, ports and reloads.

Examples:
  chrome-cdp issues
  chrome-cdp issues --since 0 --min-severity info   # all of them, info too
  chrome-cdp issues --grep cookie --all-tabs
  chrome-cdp issues --follow --timeout 1m           # keep printing new ones
```


## Rules and severities

Each of Chrome's issue codes, the start of its rule ids (the reason
in Chrome's details follows, in kebab-case: `cookie-` +
`ExcludeSameSiteUnspecifiedTreatedAsLax` is
`cookie-exclude-same-site-unspecified-treated-as-lax`), and its
severity. A code this chrome-cdp does not know (a newer Chrome's)
is shown as `unknown-<code>`, severity info.

| Code | Rule | Severity | What |
|---|---|---|---|
| `CookieIssue` | `cookie-<reason>` | warning | a cookie was blocked, or will be |
| `MixedContentIssue` | `mixed-content-<reason>` | error | an http: resource on an https: page (error when blocked, else warning) |
| `BlockedByResponseIssue` | `blocked-by-response-<reason>` | error | a response's COEP, COOP or CORP header blocked a load |
| `HeavyAdIssue` | `heavy-ad-<reason>` | warning | an ad frame used too much network or CPU (warning when removed, else info) |
| `ContentSecurityPolicyIssue` | `content-security-policy-<reason>` | error | the page's CSP blocked something (warning when report-only) |
| `SharedArrayBufferIssue` | `shared-array-buffer-<reason>` | error | SharedArrayBuffer used without cross-origin isolation (warning when only warned) |
| `CorsIssue` | `cors-<reason>` | error | CORS blocked a request (warning when only warned) |
| `QuirksModeIssue` | `quirks-mode` | info | a document renders in quirks mode (no &lt;!DOCTYPE html&gt;); quirks-mode-limited for limited-quirks mode |
| `PartitioningBlobURLIssue` | `partitioning-blob-url-<reason>` | warning | a blob: URL was used across storage partitions |
| `NavigatorUserAgentIssue` | `navigator-user-agent` | info | a script reads the reduced navigator.userAgent |
| `GenericIssue` | `<reason>` | warning | form problems (labels, autocomplete, ids) and other findings, by reason |
| `DeprecationIssue` | `deprecation-<reason>` | warning | a deprecated API was used |
| `ClientHintIssue` | `client-hint-<reason>` | warning | a Client Hints meta tag is not valid |
| `FederatedAuthRequestIssue` | `federated-auth-request-<reason>` | info | a FedCM sign-in request failed |
| `BounceTrackingIssue` | `bounce-tracking` | info | Chrome will clear the storage of bounce-tracking sites |
| `CookieDeprecationMetadataIssue` | `cookie-deprecation-metadata` | info | third-party cookie deprecation metadata |
| `StylesheetLoadingIssue` | `stylesheet-loading-<reason>` | warning | a stylesheet failed to load, or an @import came too late |
| `FederatedAuthUserInfoRequestIssue` | `federated-auth-user-info-request-<reason>` | info | a FedCM user info request failed |
| `PropertyRuleIssue` | `property-rule-<reason>` | warning | an @property rule is not valid |
| `SharedDictionaryIssue` | `shared-dictionary-<reason>` | info | a compression dictionary could not be used or stored |
| `ElementAccessibilityIssue` | `element-accessibility-<reason>` | warning | an element has content its role does not allow (&lt;select&gt;, &lt;option&gt;, &lt;legend&gt;, &lt;summary&gt;) |
| `SRIMessageSignatureIssue` | `sri-message-signature-<reason>` | error | a response's signature did not verify |
| `UnencodedDigestIssue` | `unencoded-digest-<reason>` | warning | a response's Unencoded-Digest header is not valid |
| `ConnectionAllowlistIssue` | `connection-allowlist-<reason>` | warning | a Connection-Allowlist header is not valid, or blocked a connection |
| `UserReidentificationIssue` | `user-reidentification-<reason>` | warning | a load or canvas read was blocked or noised against fingerprinting |
| `PermissionElementIssue` | `permission-element-<reason>` | warning | a &lt;permission&gt; element is not valid (info when only warned) |
| `PerformanceIssue` | `performance-<reason>` | info | a slow API was used (document.cookie) |
| `SelectivePermissionsInterventionIssue` | `selective-permissions-intervention` | info | an ad script was denied a permission-gated API |
| `EmailVerificationRequestIssue` | `email-verification-request-<reason>` | info | an email verification request failed |
| `LazyLoadImageIssue` | `lazy-load-image` | info | a lazy-loaded image is in view as the page loads |

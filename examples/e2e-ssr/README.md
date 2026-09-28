# e2e-ssr: mocking a server-rendered app's API

A Next.js page that fetches its data on the server, where no browser can
intercept the requests. `chrome-cdp test` starts `mock serve` on port
4010 (`mock_server` in `chrome-cdp.toml`), and the app's server fetches
from it (`API_URL`). The page forwards the `x-chrome-cdp-test` header of
the request it renders, so each test gets its own mock state, as
browser-side mocks do (`app/page.jsx`).

```
npm ci && npm run build
chrome-cdp test
```

Unlike [`examples/e2e-shop`](../e2e-shop), the build is not committed, so
running it needs Node; CI builds it in the `e2e-ssr` job. See
[docs/e2e/beyond-fetch.md](../../docs/e2e/beyond-fetch.md).

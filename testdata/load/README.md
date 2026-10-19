# The load server and the load journeys

`server/` is the load server (plan 23): the built app of
`examples/e2e-shop` and its API on one origin, with latency and errors
you set per endpoint, so `chrome-cdp load`'s tests and benchmarks run
offline against a server whose behaviour they know.

```sh
cargo run --release -p load-server -- --port 4176
cargo run --release -p load-server -- --port 4176 \
  --latency /api/cart=200ms±50 --errors 'POST /api/orders=2%'
```

`GET /__load/stats` answers what the server saw per `METHOD /path`
(requests and injected errors), and `POST /__load/reset` starts over.
`load-server --help` lists the flags.

//! The real backend of `examples/e2e-shop` (plan 11, milestone 11.10):
//! the shop's built app (`dist/`) and its `/api`, on one origin, so the
//! example's scenarios can run against a real server with `chrome-cdp
//! test --backend local`. The example's mock sets (`e2e/mocks`) answer
//! the same requests with the same shapes; where the data differs (a
//! fourth product), the example's vars files say so.
//!
//! - `GET /api/products`, `GET /api/products/<id>` (404 if none);
//! - `GET /api/search?q=` (name contains `q`): `{"query", "results"}`;
//! - `POST /api/login` (`{"email", "password"}`; the password is
//!   `secret`, else 401) sets an `HttpOnly` `sid` cookie; `GET /api/me`
//!   is `{"user": {"email"}}` or `{"user": null}`; `POST /api/logout`;
//! - `/api/cart`: `GET`, `POST` (201) and `DELETE /api/cart/<id>`, one
//!   cart per `cart` cookie (set on the first cart request without one),
//!   so parallel tests in their own browser contexts have their own;
//! - `POST /api/orders` (signed in, else 401; 201) and
//!   `GET /api/orders/<id>`;
//! - `/api/live`, a WebSocket that says `{"type":"hello"}` and then
//!   nothing (live prices are pushed by the mocks only);
//! - any other path: the file under `dist/`, or `index.html` for a path
//!   whose last segment has no `.` (the app's own routes).
//!
//! The checkout's payment provider is a server of its own (see
//! `payments`), on another origin.
//!
//! State lives as long as the server; [`Shop::start`] starts afresh.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use bytes::Bytes;
use futures_util::{SinkExt as _, StreamExt as _};
use http_body_util::{BodyExt as _, Full};
use hyper::body::Incoming;
use hyper::header::{
    CONNECTION, COOKIE, HeaderValue, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY, SET_COOKIE, UPGRADE,
};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::Role;

/// The password every account has.
pub(crate) const PASSWORD: &str = "secret";

/// A running shop server. It stops with the process.
#[derive(Debug, Clone)]
pub struct Shop {
    url: String,
    payments: String,
}

impl Shop {
    /// Serves `dist` and the API on `http://localhost:<port>` (0 = any free
    /// port) and returns once it accepts connections.
    pub fn start(dist: PathBuf, port: u16) -> std::io::Result<Self> {
        Self::start_with(dist, port, 0)
    }

    /// [`Shop::start`], with the payment provider on
    /// `http://127.0.0.1:<payments_port>` (0 = any free port).
    pub fn start_with(dist: PathBuf, port: u16, payments_port: u16) -> std::io::Result<Self> {
        let payments = crate::payments::start(payments_port)?;
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))?;
        listener.set_nonblocking(true)?;
        let url = format!("http://localhost:{}", listener.local_addr()?.port());
        let routes = ShopRoutes::new(dist);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        std::thread::Builder::new()
            .name("shop".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                        return;
                    };
                    loop {
                        let Ok((stream, _)) = listener.accept().await else {
                            continue;
                        };
                        tokio::spawn(serve_connection(stream, routes.clone()));
                    }
                });
            })?;
        Ok(Self { url, payments })
    }

    /// `http://localhost:<port>`.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The payment provider: `http://127.0.0.1:<port>`.
    pub fn payments(&self) -> &str {
        &self.payments
    }
}

#[derive(Debug)]
struct Site {
    dist: PathBuf,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    /// Signed-in sessions: sid → email.
    sessions: HashMap<String, String>,
    /// Carts by their `cart` cookie.
    carts: HashMap<String, Vec<Value>>,
    orders: Vec<Value>,
    /// The last id handed out (sessions, carts, cart items, orders).
    last_id: u64,
}

impl State {
    fn next_id(&mut self) -> u64 {
        self.last_id += 1;
        self.last_id
    }
}

/// The catalogue: the mocks' three products and a fourth.
fn products() -> Vec<Value> {
    vec![
        json!({ "id": 1, "name": "Mechanical keyboard", "price": 89, "description": "Tactile switches and a steel frame." }),
        json!({ "id": 2, "name": "Wireless mouse", "price": 25, "description": "Six buttons, one charge a month." }),
        json!({ "id": 3, "name": "USB-C monitor", "price": 249, "description": "27 inches, one cable." }),
        json!({ "id": 4, "name": "Laptop stand", "price": 39, "description": "Aluminium, folds flat." }),
    ]
}

/// A product without its description, as lists show it.
fn summary(product: &Value) -> Value {
    json!({ "id": product["id"], "name": product["name"], "price": product["price"] })
}

/// The shop's app and API, without a server of their own: the load
/// server (`testdata/load/server`) answers with them behind its faults.
/// Clones share the state.
#[derive(Debug, Clone)]
pub struct ShopRoutes {
    site: Arc<Site>,
}

impl ShopRoutes {
    /// The routes of a fresh shop serving the built app in `dist`.
    pub fn new(dist: PathBuf) -> Self {
        Self {
            site: Arc::new(Site {
                dist,
                state: Mutex::new(State::default()),
            }),
        }
    }

    /// Answers a request as the shop does: `/api/live`, the API, else a
    /// file of the app.
    pub async fn respond(&self, mut req: Request<Incoming>) -> Response<Full<Bytes>> {
        let path = req.uri().path().to_owned();
        if path == "/api/live" {
            return live(&mut req);
        }
        if path.starts_with("/api/") {
            let query = req.uri().query().unwrap_or_default().to_owned();
            let (parts, body) = req.into_parts();
            let body = body
                .collect()
                .await
                .map(http_body_util::Collected::to_bytes)
                .unwrap_or_default();
            let cookies = parts
                .headers
                .get(COOKIE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            let mut state = self
                .site
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            return api(&mut state, &parts.method, &path, &query, cookies, &body);
        }
        file(&self.site.dist, &path)
    }
}

async fn serve_connection(stream: tokio::net::TcpStream, routes: ShopRoutes) {
    let service = service_fn(move |req: Request<Incoming>| {
        let routes = routes.clone();
        async move { Ok::<_, Infallible>(routes.respond(req).await) }
    });
    let _ = http1::Builder::new()
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades()
        .await;
}

/// Answers an `/api/` request.
fn api(
    state: &mut State,
    method: &Method,
    path: &str,
    query: &str,
    cookies: &str,
    body: &[u8],
) -> Response<Full<Bytes>> {
    let rest = path.trim_start_matches("/api/").trim_end_matches('/');
    let parts: Vec<&str> = rest.split('/').collect();
    let body: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let sid = cookie(cookies, "sid").filter(|s| state.sessions.contains_key(*s));
    match (method, parts.as_slice()) {
        (&Method::GET, ["products"]) => ok(&products().iter().map(summary).collect()),
        (&Method::GET, ["products", id]) => products()
            .into_iter()
            .find(|p| same_id(&p["id"], id))
            .map_or_else(|| not_found(&format!("no product {id}")), |p| ok(&p)),
        (&Method::GET, ["search"]) => {
            let q = crate::percent_decode(crate::param(query, "q").unwrap_or_default());
            let needle = q.to_lowercase();
            let results: Vec<Value> = products()
                .iter()
                .filter(|p| {
                    !needle.is_empty()
                        && p["name"]
                            .as_str()
                            .is_some_and(|n| n.to_lowercase().contains(&needle))
                })
                .map(summary)
                .collect();
            ok(&json!({ "query": q, "results": results }))
        }
        (&Method::POST, ["login"]) => {
            let email = body["email"].as_str().unwrap_or_default();
            if email.is_empty() || body["password"].as_str() != Some(PASSWORD) {
                return answer(
                    StatusCode::UNAUTHORIZED,
                    &json!({ "error": "Wrong email or password" }),
                );
            }
            let sid = format!("s{}", state.next_id());
            state.sessions.insert(sid.clone(), email.to_owned());
            with_cookie(
                ok(&json!({ "user": { "email": email } })),
                &format!("sid={sid}; Path=/; HttpOnly"),
            )
        }
        (&Method::POST, ["logout"]) => {
            if let Some(sid) = sid {
                state.sessions.remove(sid);
            }
            with_cookie(ok(&json!({ "ok": true })), "sid=; Path=/; Max-Age=0")
        }
        (&Method::GET, ["me"]) => match sid.and_then(|s| state.sessions.get(s)) {
            Some(email) => ok(&json!({ "user": { "email": email } })),
            None => ok(&json!({ "user": null })),
        },
        (_, ["cart", ..]) => cart(state, method, &parts[1..], cookies, &body),
        (&Method::POST, ["orders"]) => {
            if sid.is_none() {
                return answer(
                    StatusCode::UNAUTHORIZED,
                    &json!({ "error": "Sign in to check out" }),
                );
            }
            let order = json!({
                "id": state.next_id(),
                "items": body["items"],
                "total": body["total"],
            });
            state.orders.push(order.clone());
            answer(StatusCode::CREATED, &order)
        }
        (&Method::GET, ["orders", id]) => state
            .orders
            .iter()
            .find(|o| same_id(&o["id"], id))
            .map_or_else(|| not_found(&format!("no order {id}")), ok),
        _ => not_found(&format!("no route {method} {path}")),
    }
}

/// `/api/cart[/<id>]`: the cart of the `cart` cookie, made (and the cookie
/// set) when there is none.
fn cart(
    state: &mut State,
    method: &Method,
    rest: &[&str],
    cookies: &str,
    body: &Value,
) -> Response<Full<Bytes>> {
    let (key, new) =
        if let Some(key) = cookie(cookies, "cart").filter(|c| state.carts.contains_key(*c)) {
            (key.to_owned(), false)
        } else {
            let key = format!("c{}", state.next_id());
            state.carts.insert(key.clone(), Vec::new());
            (key, true)
        };
    let id = state.next_id();
    let items = state.carts.entry(key.clone()).or_default();
    let response = match (method, rest) {
        (&Method::GET, []) => ok(&Value::Array(items.clone())),
        (&Method::POST, []) => {
            let mut item = body.clone();
            if !item.is_object() {
                return answer(
                    StatusCode::BAD_REQUEST,
                    &json!({ "error": "a cart item is a JSON object" }),
                );
            }
            item["id"] = json!(id);
            items.push(item.clone());
            answer(StatusCode::CREATED, &item)
        }
        (&Method::DELETE, [id]) => match items.iter().position(|i| same_id(&i["id"], id)) {
            Some(at) => ok(&items.remove(at)),
            None => not_found(&format!("no cart item {id}")),
        },
        _ => answer(
            StatusCode::METHOD_NOT_ALLOWED,
            &json!({ "error": format!("{method} is not allowed here") }),
        ),
    };
    if new {
        with_cookie(response, &format!("cart={key}; Path=/"))
    } else {
        response
    }
}

/// Whether a JSON id is the id in a path.
fn same_id(json: &Value, id: &str) -> bool {
    json.as_u64().is_some_and(|n| id.parse() == Ok(n))
}

/// The value of cookie `name` in a `Cookie` header.
fn cookie<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header
        .split(';')
        .filter_map(|p| p.trim().split_once('='))
        .find(|(n, _)| *n == name)
        .map(|(_, v)| v)
}

fn with_cookie(mut response: Response<Full<Bytes>>, cookie: &str) -> Response<Full<Bytes>> {
    if let Ok(value) = HeaderValue::from_str(cookie) {
        response.headers_mut().append(SET_COOKIE, value);
    }
    response
}

fn ok(body: &Value) -> Response<Full<Bytes>> {
    answer(StatusCode::OK, body)
}

fn not_found(error: &str) -> Response<Full<Bytes>> {
    answer(StatusCode::NOT_FOUND, &json!({ "error": error }))
}

fn answer(status: StatusCode, body: &Value) -> Response<Full<Bytes>> {
    crate::response(status, "application/json", Bytes::from(body.to_string()))
}

/// A file of the app, or its `index.html` for the app's own routes.
fn file(dist: &Path, path: &str) -> Response<Full<Bytes>> {
    let relative = Path::new(path.trim_start_matches('/'));
    if relative
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return crate::not_found(path);
    }
    let mut file = dist.join(relative);
    if file.is_dir() {
        file.push("index.html");
    }
    let last = path.rsplit('/').next().unwrap_or_default();
    if !file.is_file() && !last.contains('.') {
        file = dist.join("index.html");
    }
    match std::fs::read(&file) {
        Ok(body) => crate::response(
            StatusCode::OK,
            crate::content_type(&file),
            Bytes::from(body),
        ),
        Err(_) => crate::not_found(path),
    }
}

/// `/api/live`: says hello, then waits for the page to close it.
fn live(req: &mut Request<Incoming>) -> Response<Full<Bytes>> {
    let Some(key) = req.headers().get(SEC_WEBSOCKET_KEY) else {
        return answer(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "/api/live needs a WebSocket upgrade" }),
        );
    };
    let accept = derive_accept_key(key.as_bytes());
    let upgrade = hyper::upgrade::on(req);
    tokio::spawn(async move {
        let Ok(upgraded) = upgrade.await else {
            return;
        };
        let mut ws =
            WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, None).await;
        if ws
            .send(Message::text(json!({ "type": "hello" }).to_string()))
            .await
            .is_err()
        {
            return;
        }
        while let Some(Ok(_)) = ws.next().await {}
    });
    let mut response = crate::response(
        StatusCode::SWITCHING_PROTOCOLS,
        "text/plain; charset=utf-8",
        Bytes::new(),
    );
    let headers = response.headers_mut();
    headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
    headers.insert(CONNECTION, HeaderValue::from_static("Upgrade"));
    if let Ok(accept) = HeaderValue::from_str(&accept) {
        headers.insert(SEC_WEBSOCKET_ACCEPT, accept);
    }
    response
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::needless_pass_by_value)]

    use http_body_util::BodyExt as _;
    use hyper::Method;
    use serde_json::{Value, json};

    use super::{State, api, file};

    /// Status, `Set-Cookie` and JSON body of an API answer.
    fn call(
        state: &mut State,
        method: Method,
        path: &str,
        cookies: &str,
        body: &Value,
    ) -> (u16, Option<String>, Value) {
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        let body = if body.is_null() {
            Vec::new()
        } else {
            body.to_string().into_bytes()
        };
        let response = api(state, &method, path, query, cookies, &body);
        let status = response.status().as_u16();
        let cookie = response
            .headers()
            .get("set-cookie")
            .map(|v| v.to_str().unwrap().to_owned());
        let bytes = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(response.into_body().collect())
            .unwrap()
            .to_bytes();
        (status, cookie, serde_json::from_slice(&bytes).unwrap())
    }

    #[test]
    fn products_search_login_cart_and_orders() {
        let mut s = State::default();
        let (_, _, list) = call(&mut s, Method::GET, "/api/products", "", &Value::Null);
        assert_eq!(list.as_array().unwrap().len(), 4);
        assert!(list[0].get("description").is_none());
        let (status, _, _) = call(&mut s, Method::GET, "/api/products/9", "", &Value::Null);
        assert_eq!(status, 404);
        let (_, _, found) = call(&mut s, Method::GET, "/api/search?q=Key", "", &Value::Null);
        assert_eq!(found["results"][0]["name"], "Mechanical keyboard");

        let wrong = json!({ "email": "ada@example.com", "password": "nope" });
        let (status, _, _) = call(&mut s, Method::POST, "/api/login", "", &wrong);
        assert_eq!(status, 401);
        let right = json!({ "email": "ada@example.com", "password": "secret" });
        let (_, cookie, _) = call(&mut s, Method::POST, "/api/login", "", &right);
        let sid = cookie.unwrap().split(';').next().unwrap().to_owned();
        let (_, _, me) = call(&mut s, Method::GET, "/api/me", &sid, &Value::Null);
        assert_eq!(me, json!({ "user": { "email": "ada@example.com" } }));
        let (_, _, me) = call(&mut s, Method::GET, "/api/me", "", &Value::Null);
        assert_eq!(me, json!({ "user": null }));

        // A cart per cookie.
        let item = json!({ "product_id": 1, "name": "Mechanical keyboard", "price": 89, "qty": 1 });
        let (status, cookie, added) = call(&mut s, Method::POST, "/api/cart", "", &item);
        assert_eq!(status, 201);
        let cart = cookie.unwrap().split(';').next().unwrap().to_owned();
        let (_, _, items) = call(&mut s, Method::GET, "/api/cart", &cart, &Value::Null);
        assert_eq!(items.as_array().unwrap().len(), 1);
        let (_, _, other) = call(&mut s, Method::GET, "/api/cart", "", &Value::Null);
        assert_eq!(other, json!([]));
        let path = format!("/api/cart/{}", added["id"]);
        let (status, _, _) = call(&mut s, Method::DELETE, &path, &cart, &Value::Null);
        assert_eq!(status, 200);

        // Orders need a signed-in user.
        let order = json!({ "items": [item], "total": 89 });
        let (status, _, _) = call(&mut s, Method::POST, "/api/orders", "", &order);
        assert_eq!(status, 401);
        let (status, _, placed) = call(&mut s, Method::POST, "/api/orders", &sid, &order);
        assert_eq!(status, 201);
        let path = format!("/api/orders/{}", placed["id"]);
        let (_, _, got) = call(&mut s, Method::GET, &path, "", &Value::Null);
        assert_eq!(got["total"], 89);
    }

    #[test]
    fn the_app_answers_its_own_routes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<p>shop</p>").unwrap();
        assert_eq!(file(dir.path(), "/cart").status(), 200);
        assert_eq!(file(dir.path(), "/products/2").status(), 200);
        assert_eq!(file(dir.path(), "/assets/gone.js").status(), 404);
        assert_eq!(file(dir.path(), "/../x").status(), 404);
    }
}

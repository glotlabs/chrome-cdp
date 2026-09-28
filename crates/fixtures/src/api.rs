//! `/e2e/api/…`: a small real backend behind the test runner's shop page
//! (`testdata/site/e2e/shop.html`), for runs against a real backend (plan
//! 11, milestone 11.11). The mock sets of `testdata/e2e` answer the same
//! paths in mock mode, with the same shapes.
//!
//! Its state belongs to the server: [`crate::Fixtures::start`] gets a
//! fresh one, seeded with the two todos the mocks seed.
//!
//! - `GET /e2e/api/price`: `{"price": 49}`;
//! - `GET /e2e/api/todos`: the todos; `POST` adds one (`{"title": …}`,
//!   201); `GET` and `DELETE /e2e/api/todos/<id>` one of them;
//! - `POST /e2e/api/login` (`{"email": …}`) sets an `HttpOnly` `sid`
//!   cookie, and `GET /e2e/api/me` answers the email for it, or 401.

use std::sync::Mutex;

use bytes::Bytes;
use http_body_util::Full;
use hyper::header::{CONTENT_TYPE, HeaderValue, SET_COOKIE};
use hyper::{Method, Response, StatusCode};
use serde_json::{Value, json};

/// The paths the API answers.
pub(crate) const PREFIX: &str = "/e2e/api/";

/// The API's state.
#[derive(Debug)]
pub(crate) struct Api {
    todos: Vec<Value>,
    next_id: u64,
    /// Logged-in sessions: (sid, email).
    sessions: Vec<(String, String)>,
}

impl Default for Api {
    fn default() -> Self {
        Self {
            todos: vec![
                json!({ "id": 1, "title": "Buy milk", "done": false }),
                json!({ "id": 2, "title": "Write tests", "done": true }),
            ],
            next_id: 3,
            sessions: Vec::new(),
        }
    }
}

/// Answers a request to the API: `cookie` is its `Cookie` header.
pub(crate) fn respond(
    api: &Mutex<Api>,
    method: &Method,
    path: &str,
    cookie: Option<&str>,
    body: &[u8],
) -> Response<Full<Bytes>> {
    let mut api = api
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let rest = path.trim_start_matches(PREFIX).trim_end_matches('/');
    let parts: Vec<&str> = rest.split('/').collect();
    match (method, parts.as_slice()) {
        (&Method::GET, ["price"]) => ok(&json!({ "price": 49 })),
        (&Method::GET, ["todos"]) => ok(&Value::Array(api.todos.clone())),
        (&Method::POST, ["todos"]) => {
            let Some(title) = serde_json::from_slice::<Value>(body)
                .ok()
                .and_then(|v| v["title"].as_str().map(str::to_owned))
            else {
                return answer(
                    StatusCode::BAD_REQUEST,
                    &json!({ "error": "a todo needs a title" }),
                );
            };
            let done = serde_json::from_slice::<Value>(body)
                .ok()
                .and_then(|v| v["done"].as_bool())
                .unwrap_or(false);
            let todo = json!({ "id": api.next_id, "title": title, "done": done });
            api.next_id += 1;
            api.todos.push(todo.clone());
            answer(StatusCode::CREATED, &todo)
        }
        (&Method::GET | &Method::DELETE, ["todos", id]) => {
            let Some(at) = api
                .todos
                .iter()
                .position(|t| t["id"].as_u64().is_some_and(|n| id.parse() == Ok(n)))
            else {
                return answer(
                    StatusCode::NOT_FOUND,
                    &json!({ "error": format!("no todo {id}") }),
                );
            };
            if method == Method::DELETE {
                ok(&api.todos.remove(at))
            } else {
                ok(&api.todos[at])
            }
        }
        (&Method::POST, ["login"]) => {
            let Some(email) = serde_json::from_slice::<Value>(body)
                .ok()
                .and_then(|v| v["email"].as_str().map(str::to_owned))
            else {
                return answer(
                    StatusCode::BAD_REQUEST,
                    &json!({ "error": "log in with an email" }),
                );
            };
            let sid = format!("s{}", api.sessions.len() + 1);
            api.sessions.push((sid.clone(), email.clone()));
            let mut response = ok(&json!({ "ok": true, "email": email }));
            if let Ok(value) = HeaderValue::from_str(&format!("sid={sid}; Path=/; HttpOnly")) {
                response.headers_mut().insert(SET_COOKIE, value);
            }
            response
        }
        (&Method::GET, ["me"]) => {
            let sid = cookie.and_then(|c| {
                c.split(';')
                    .filter_map(|p| p.trim().split_once('='))
                    .find(|(n, _)| *n == "sid")
                    .map(|(_, v)| v.to_owned())
            });
            match api.sessions.iter().find(|(s, _)| Some(s) == sid.as_ref()) {
                Some((_, email)) => ok(&json!({ "email": email })),
                None => answer(
                    StatusCode::UNAUTHORIZED,
                    &json!({ "error": "log in first" }),
                ),
            }
        }
        _ => answer(
            StatusCode::NOT_FOUND,
            &json!({ "error": format!("no route {method} {path}") }),
        ),
    }
}

fn ok(body: &Value) -> Response<Full<Bytes>> {
    answer(StatusCode::OK, body)
}

fn answer(status: StatusCode, body: &Value) -> Response<Full<Bytes>> {
    let mut response = crate::response(status, "application/json", Bytes::from(body.to_string()));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

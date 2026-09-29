//! The mock file format as written (TOML), and its JSON Schema.
//!
//! These types are only parsed; [`crate::load`] checks and compiles them.
//! Their doc comments are the schema's descriptions, so they are written
//! for the person editing a mock file.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ops::Range;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use toml::Spanned;

/// A value with where it is in the file. Its schema is the value's.
#[derive(Debug, Clone)]
pub(crate) struct At<T>(pub(crate) Spanned<T>);

impl<T> At<T> {
    pub(crate) fn span(&self) -> Range<usize> {
        self.0.span()
    }

    pub(crate) fn get(&self) -> &T {
        self.0.get_ref()
    }

    pub(crate) fn into_inner(self) -> T {
        self.0.into_inner()
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for At<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Spanned::deserialize(deserializer).map(At)
    }
}

impl<T: JsonSchema> JsonSchema for At<T> {
    fn inline_schema() -> bool {
        T::inline_schema()
    }

    fn schema_name() -> Cow<'static, str> {
        T::schema_name()
    }

    fn schema_id() -> Cow<'static, str> {
        T::schema_id()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        T::json_schema(generator)
    }
}

/// A chrome-cdp mock file: one mock set. Each `[[mock]]` answers the
/// requests it matches; within a file the first match wins, then the
/// `[[collection]]`s are tried, and sets applied later are tried first.
/// Data files (`json_file`, `body_file`, `file`, `seed`) are relative to
/// the mocks directory.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "chrome-cdp mock file")]
pub(crate) struct RawFile {
    /// Settings for the whole set.
    #[serde(default)]
    pub(crate) settings: Option<At<RawSettings>>,
    /// Named values and where they start, e.g. `logged_in = false`.
    /// Mocks require them (`when`) and change them (`set_state`); every
    /// set in use shares them, and `mock reset` starts them over.
    #[serde(default)]
    pub(crate) state: Option<At<BTreeMap<String, Value>>>,
    /// The mocks, tried in the order written.
    #[serde(default, rename = "mock")]
    pub(crate) mocks: Vec<At<RawMock>>,
    /// Fake REST resources kept in memory, tried after the set's mocks.
    #[serde(default, rename = "collection")]
    pub(crate) collections: Vec<At<RawCollection>>,
    /// Fake WebSocket servers: the page's `WebSocket`s to these URLs talk
    /// to the mocks instead of the network.
    #[serde(default, rename = "websocket")]
    pub(crate) websockets: Vec<At<RawWebSocket>>,
}

/// `[settings]`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "Settings")]
pub(crate) struct RawSettings {
    /// The requests the set owns: path globs (`/api/**`) or full-URL globs
    /// (`https://api.example.com/**`). An owned request no mock answers is
    /// handled by `unmatched`; other requests no mock answers go to the
    /// network.
    #[serde(default)]
    pub(crate) scope: Option<At<OneOrMany<String>>>,
    /// What an owned request that no mock answers gets: `fail` (a 501
    /// answer, and the request counts as unmatched), `passthrough` (it
    /// goes to the real server) or `abort` (a network error).
    #[serde(default)]
    pub(crate) unmatched: Option<Unmatched>,
    /// `auto`: answers carry CORS headers and preflights for mocked
    /// requests are allowed, so cross-origin API hosts work. `off`: CORS is
    /// left to the mocks' own headers.
    #[serde(default)]
    pub(crate) cors: Option<Cors>,
    /// The delay every answer of the set gets: `0ms`, `100ms`, or a range
    /// such as `50ms..200ms` (seeded, so reruns match). A mock's `delay`
    /// wins.
    #[serde(default)]
    pub(crate) latency: Option<At<String>>,
}

/// `[settings] unmatched`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Unmatched {
    #[default]
    Fail,
    Passthrough,
    Abort,
}

impl Unmatched {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fail => "fail",
            Self::Passthrough => "passthrough",
            Self::Abort => "abort",
        }
    }
}

/// `[settings] cors`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Cors {
    #[default]
    Auto,
    Off,
}

/// One value or a list of them.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
pub(crate) enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> OneOrMany<T> {
    pub(crate) fn into_vec(self) -> Vec<T> {
        match self {
            Self::One(one) => vec![one],
            Self::Many(many) => many,
        }
    }
}

/// `[[mock]]`: a request it matches and the answer.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "Mock")]
pub(crate) struct RawMock {
    /// What `mock list`, `mock log` and `mock explain` call it; `METHOD
    /// path` when not set.
    #[serde(default)]
    pub(crate) name: Option<String>,
    /// The requests it answers. Every condition given must hold.
    #[serde(rename = "match")]
    pub(crate) matcher: At<RawMatch>,
    /// The answer.
    #[serde(default)]
    pub(crate) respond: Option<At<RawRespond>>,
    /// Answers given in turn instead of `respond`: the first to the first
    /// request, the second to the next, … (`mock reset` starts over).
    #[serde(default)]
    pub(crate) sequence: Option<At<Vec<At<RawRespond>>>>,
    /// With `sequence`, what comes after its last answer:
    /// `repeat-last` (the default), `cycle` (from the first again) or
    /// `unmatched` (the mock stops matching, so the next one answers).
    #[serde(default)]
    pub(crate) after_sequence: Option<AfterSequence>,
    /// Named values (`[state]`) that must hold for the mock to match:
    /// `{ logged_in = true }`. `"*"` means any value but none; other
    /// values match as `match.json` does.
    #[serde(default)]
    pub(crate) when: Option<At<BTreeMap<String, Value>>>,
    /// Named values to change after answering: `{ logged_in = true,
    /// user = "{{json.email}}" }`. Strings may be templates.
    #[serde(default)]
    pub(crate) set_state: Option<At<BTreeMap<String, Value>>>,
    /// Named values to remove after answering (a `when` of `"*"` then no
    /// longer holds), e.g. `["user"]`.
    #[serde(default)]
    pub(crate) unset_state: Option<At<Vec<String>>>,
}

/// `[[websocket]]`: a fake WebSocket server. A page's `WebSocket` to a
/// URL it matches opens at once and talks to it: `on_open` sends its
/// messages, and each message the page sends gets the answer of the first
/// `on_message` rule that matches it. Scenarios push more with `mock ws
/// send`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "WebSocket")]
pub(crate) struct RawWebSocket {
    /// What `mock list` and `mock log` call it; `WS <url>` when not set.
    #[serde(default)]
    pub(crate) name: Option<String>,
    /// The sockets it answers: a glob on the URL's path (`/ws/**`,
    /// `/ws/rooms/:room`), or on the full URL (`wss://push.example.com/**`).
    pub(crate) url: At<String>,
    /// The subprotocol the socket reports (`ws.protocol`); the first one
    /// the page asked for when not set.
    #[serde(default)]
    pub(crate) protocol: Option<String>,
    /// What the server does once the socket is open, in order.
    #[serde(default)]
    pub(crate) on_open: Option<At<Vec<At<RawWsAction>>>>,
    /// Answers to the page's messages: the first rule whose `match` fits a
    /// message answers it; a message no rule fits gets no answer.
    #[serde(default)]
    pub(crate) on_message: Option<At<Vec<At<RawWsRule>>>>,
}

/// A WebSocket mock's action: send a message, or close the socket.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "WebSocketAction")]
pub(crate) struct RawWsAction {
    /// The message: a string is sent as it is, anything else as JSON text.
    /// Strings may be templates (`{{json.id}}` is the page's message's).
    #[serde(default)]
    pub(crate) send: Option<At<Value>>,
    /// A binary message instead: its bytes in base64 (`AQID`), a template
    /// too (`{{body}}` of a binary message is its base64).
    #[serde(default)]
    pub(crate) send_base64: Option<At<String>>,
    /// Close the socket with this code (1000, or 3000-4999).
    #[serde(default)]
    pub(crate) close: Option<At<i64>>,
    /// With `close`: the reason the page sees.
    #[serde(default)]
    pub(crate) reason: Option<String>,
    /// How long to wait first (`300ms`, `2s`), after the action before it.
    #[serde(default)]
    pub(crate) delay: Option<At<String>>,
}

/// An `on_message` rule: the messages it answers, and the answer.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "WebSocketRule")]
pub(crate) struct RawWsRule {
    /// Which messages: a table matches JSON messages as `match.json` does
    /// (a subset; `"*"` for any value), a string is a glob on the text.
    /// Every message when not set, binary ones too (only then).
    #[serde(default, rename = "match")]
    pub(crate) matcher: Option<At<Value>>,
    /// The answer: a string is sent as it is, anything else as JSON text.
    #[serde(default)]
    pub(crate) send: Option<At<Value>>,
    /// A binary answer instead: its bytes in base64 (`AQID`), a template
    /// too (`{{body}}` of a binary message is its base64).
    #[serde(default)]
    pub(crate) send_base64: Option<At<String>>,
    /// Close the socket with this code (1000, or 3000-4999) instead.
    #[serde(default)]
    pub(crate) close: Option<At<i64>>,
    /// With `close`: the reason the page sees.
    #[serde(default)]
    pub(crate) reason: Option<String>,
    /// How long to wait before answering.
    #[serde(default)]
    pub(crate) delay: Option<At<String>>,
}

/// `after_sequence`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AfterSequence {
    #[default]
    RepeatLast,
    Cycle,
    Unmatched,
}

impl AfterSequence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RepeatLast => "repeat-last",
            Self::Cycle => "cycle",
            Self::Unmatched => "unmatched",
        }
    }
}

/// `[[collection]]`: a fake REST resource. `GET path` lists the items,
/// `GET path/:id` gets one, `POST path` adds one, `PUT` and `PATCH
/// path/:id` replace and change one, and `DELETE path/:id` removes it.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "Collection")]
pub(crate) struct RawCollection {
    /// What `mock state`, `mock seed` and `expect mock-state` call it.
    pub(crate) name: At<String>,
    /// The list's path, e.g. `/api/todos`; items are at `/api/todos/:id`.
    pub(crate) path: At<String>,
    /// The items' id field; `id` when not set.
    #[serde(default)]
    pub(crate) id: Option<At<String>>,
    /// A data file (a JSON array of objects) with the items it starts
    /// with, relative to the mocks directory.
    #[serde(default)]
    pub(crate) seed: Option<At<String>>,
    /// The items it starts with, written here instead of a `seed` file.
    #[serde(default)]
    pub(crate) items: Option<At<Vec<Value>>>,
    /// How a POST without an id gets one: `increment` (the highest number
    /// plus one) or `uuid`. When not set: `increment` if the starting
    /// items' ids are numbers, else `uuid`.
    #[serde(default)]
    pub(crate) new_id: Option<NewId>,
    /// How `GET path` lists the items.
    #[serde(default)]
    pub(crate) list: Option<At<RawList>>,
}

/// `[[collection]] new_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum NewId {
    Increment,
    Uuid,
}

/// `[[collection]] list`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "CollectionList")]
pub(crate) struct RawList {
    /// Query parameters filter the items by the field of the same name
    /// (`?done=true`); parameters no item has are ignored.
    #[serde(default)]
    pub(crate) filter_query: Option<bool>,
    /// The query parameter with the page number (from 1); pages only when
    /// set.
    #[serde(default)]
    pub(crate) page_param: Option<String>,
    /// The query parameter with the page size.
    #[serde(default)]
    pub(crate) page_size_param: Option<String>,
    /// The page size when the request gives none; 20 when not set.
    #[serde(default)]
    pub(crate) page_size: Option<At<i64>>,
    /// Answer `{ "<wrap>": [items], "total": N }` instead of the bare
    /// array. Lists always carry an `X-Total-Count` header.
    #[serde(default)]
    pub(crate) wrap: Option<String>,
}

/// `match`: conditions on the request, all of which must hold.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "Match")]
pub(crate) struct RawMatch {
    /// The HTTP method, or a list of them (any case); any method when not
    /// set.
    #[serde(default)]
    pub(crate) method: Option<At<OneOrMany<String>>>,
    /// A glob on the URL's path: `/api/products/:id` captures `id`
    /// (`{{params.id}}`), `*` matches within a segment, `**` across
    /// segments. Any host.
    #[serde(default)]
    pub(crate) path: Option<At<String>>,
    /// A glob on the full URL without its query, e.g.
    /// `https://api.example.com/v1/**`.
    #[serde(default)]
    pub(crate) url: Option<At<String>>,
    /// A regular expression searched in the full URL (query included);
    /// named groups are parameters.
    #[serde(default)]
    pub(crate) regex: Option<At<String>>,
    /// Query parameters that must be present (others may be too); values
    /// are globs (`"*"` for any value).
    #[serde(default)]
    pub(crate) query: Option<BTreeMap<String, At<Scalar>>>,
    /// Request headers that must be present (any case); values are globs.
    #[serde(default)]
    pub(crate) headers: Option<BTreeMap<String, At<Scalar>>>,
    /// The JSON body must contain this: objects match a subset of keys,
    /// `"*"` matches any value and strings are globs. Arrays must match
    /// element by element unless `json_mode = "contains"`.
    #[serde(default)]
    pub(crate) json: Option<At<Value>>,
    /// How `json` matches arrays: `exact` (the default) or `contains` (each
    /// expected element matches some element).
    #[serde(default)]
    pub(crate) json_mode: Option<JsonMode>,
    /// Fields of a form body (`application/x-www-form-urlencoded`) that must
    /// be present; values are globs.
    #[serde(default)]
    pub(crate) form: Option<BTreeMap<String, At<Scalar>>>,
    /// The body as text: a glob, or `{ regex = "…" }`.
    #[serde(default)]
    pub(crate) body: Option<At<BodyMatch>>,
    /// A GraphQL request (a POST body, or a GET query string).
    #[serde(default)]
    pub(crate) graphql: Option<At<RawGraphql>>,
}

/// `json_mode`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum JsonMode {
    #[default]
    Exact,
    Contains,
}

/// A query, header or form value: text, a number or a boolean.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
pub(crate) enum Scalar {
    Text(String),
    Integer(i64),
    Float(f64),
    Bool(bool),
}

impl Scalar {
    pub(crate) fn text(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Integer(n) => n.to_string(),
            Self::Float(n) => n.to_string(),
            Self::Bool(b) => b.to_string(),
        }
    }
}

/// `match.body`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
pub(crate) enum BodyMatch {
    /// A glob on the whole body.
    Glob(String),
    /// A regular expression searched in the body.
    Regex {
        /// The regular expression.
        regex: String,
    },
}

/// `match.graphql`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "Graphql")]
pub(crate) struct RawGraphql {
    /// The operation's name: `operationName`, or the name in the query.
    #[serde(default)]
    pub(crate) operation: Option<String>,
    /// Variables the request must have, matched as `json` is.
    #[serde(default)]
    pub(crate) variables: Option<Value>,
}

/// `respond`: the answer.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "Respond")]
pub(crate) struct RawRespond {
    /// The HTTP status; 200 when not set.
    #[serde(default)]
    pub(crate) status: Option<At<i64>>,
    /// Response headers. Values may be templates.
    #[serde(default)]
    pub(crate) headers: Option<BTreeMap<String, At<String>>>,
    /// A JSON body; its strings may be templates.
    #[serde(default)]
    pub(crate) json: Option<At<Value>>,
    /// A JSON body from a data file (relative to the mocks directory). The
    /// path may be a template; a file that does not exist answers 404. Files
    /// named `*.tmpl.json` have their strings rendered as templates.
    #[serde(default)]
    pub(crate) json_file: Option<At<String>>,
    /// A text body (a template), `text/plain` unless `headers` says
    /// otherwise.
    #[serde(default)]
    pub(crate) body: Option<At<String>>,
    /// A body from a data file, typed by its extension. Files with
    /// `.tmpl.` in their name are rendered as templates.
    #[serde(default)]
    pub(crate) body_file: Option<At<String>>,
    /// A file served as it is, typed by its extension (images, fonts, …).
    #[serde(default)]
    pub(crate) file: Option<At<String>>,
    /// Server-Sent Events, sent in order (`text/event-stream`), each after
    /// its own `delay`; the stream then stays open. A page's `EventSource`
    /// and `mock serve` get them one by one; a `fetch` gets them all at
    /// once.
    #[serde(default)]
    pub(crate) sse: Option<At<Vec<At<RawSseEvent>>>>,
    /// How long to wait before answering (`300ms`, `2s`); replaces the
    /// set's latency.
    #[serde(default)]
    pub(crate) delay: Option<At<String>>,
    /// Fail the request with a network error instead: `failed`,
    /// `connectionreset`, `timedout`, `connectionrefused`, …
    #[serde(default)]
    pub(crate) abort: Option<At<String>>,
    /// Send the request to the real server.
    #[serde(default)]
    pub(crate) passthrough: Option<At<bool>>,
    /// With `passthrough`: deep-merge this into the real JSON response
    /// (`null` removes a key).
    #[serde(default)]
    pub(crate) patch: Option<At<Value>>,
}

/// A Server-Sent Event of `respond.sse`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "SseEvent")]
pub(crate) struct RawSseEvent {
    /// The event's data: a string is sent as it is (a line each), anything
    /// else as JSON text. Strings may be templates.
    #[serde(default)]
    pub(crate) data: Option<At<Value>>,
    /// The event's type (`addEventListener("price", …)`); `message` when
    /// not set.
    #[serde(default)]
    pub(crate) event: Option<At<String>>,
    /// The event's id (`lastEventId`).
    #[serde(default)]
    pub(crate) id: Option<At<String>>,
    /// The reconnection time the event sets, in ms.
    #[serde(default)]
    pub(crate) retry: Option<At<i64>>,
    /// A comment line (`: text`), which pages do not see.
    #[serde(default)]
    pub(crate) comment: Option<String>,
    /// How long to wait before sending it (`1s`), after the event before
    /// it.
    #[serde(default)]
    pub(crate) delay: Option<At<String>>,
}

/// The JSON Schema of mock files (`chrome-cdp schema mocks`).
pub fn schema() -> Value {
    schemars::schema_for!(RawFile).to_value()
}

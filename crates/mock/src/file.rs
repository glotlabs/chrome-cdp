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
/// requests it matches; within a file the first match wins, and sets
/// applied later are tried first. Data files (`json_file`, `body_file`,
/// `file`) are relative to the mocks directory.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "chrome-cdp mock file")]
pub(crate) struct RawFile {
    /// Settings for the whole set.
    #[serde(default)]
    pub(crate) settings: Option<At<RawSettings>>,
    /// The mocks, tried in the order written.
    #[serde(default, rename = "mock")]
    pub(crate) mocks: Vec<At<RawMock>>,
}

/// `[settings]`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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
pub(crate) struct RawMock {
    /// What `mock list`, `mock log` and `mock explain` call it; `METHOD
    /// path` when not set.
    #[serde(default)]
    pub(crate) name: Option<String>,
    /// The requests it answers. Every condition given must hold.
    #[serde(rename = "match")]
    pub(crate) matcher: At<RawMatch>,
    /// The answer.
    pub(crate) respond: Option<At<RawRespond>>,
}

/// `match`: conditions on the request, all of which must hold.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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

/// The JSON Schema of mock files (`chrome-cdp schema mocks`).
pub fn schema() -> Value {
    schemars::schema_for!(RawFile).to_value()
}

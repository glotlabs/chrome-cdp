//! Templates in mock files (`minijinja`): `{{params.id}}`, `{{query.q}}`,
//! `{{headers.authorization}}`, `{{json.user.email}}`, `{{uuid}}`,
//! `{{now}}`, `{{seq}}`, `{{faker.name}}` and `{{state.x}}`.
//!
//! Only strings with `{{` or `{%` are templates; everything else is taken
//! as written, so large data files cost nothing. An undefined value is an
//! error, never an empty string. A JSON string that is one expression and
//! nothing else (`"{{json.qty}}"`) keeps the expression's type, so a number
//! stays a number.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use minijinja::{Environment, UndefinedBehavior, Value};
use serde_json::Value as Json;

/// The template environment: strict about undefined values, no escaping.
pub(crate) fn env() -> &'static Environment<'static> {
    static ENV: OnceLock<Environment<'static>> = OnceLock::new();
    ENV.get_or_init(|| {
        let mut env = Environment::new();
        env.set_undefined_behavior(UndefinedBehavior::Strict);
        env
    })
}

/// Whether `text` is a template.
pub(crate) fn is_template(text: &str) -> bool {
    text.contains("{{") || text.contains("{%")
}

/// The expression of a string that is `{{ expr }}` and nothing else.
fn sole_expression(text: &str) -> Option<&str> {
    let inner = text.trim().strip_prefix("{{")?.strip_suffix("}}")?;
    (!inner.contains("{{") && !inner.contains("}}") && !inner.contains("{%"))
        .then_some(inner.trim())
}

/// Checks the syntax of template `text`.
pub(crate) fn check(text: &str) -> Result<(), String> {
    if !is_template(text) {
        return Ok(());
    }
    let result = match sole_expression(text) {
        Some(expr) => env().compile_expression(expr).map(drop),
        None => env().template_from_str(text).map(drop),
    };
    result.map_err(|e| describe(&e, text))
}

/// Checks every string in `value`.
pub(crate) fn check_json(value: &Json) -> Result<(), String> {
    match value {
        Json::String(s) => check(s),
        Json::Array(items) => items.iter().try_for_each(check_json),
        Json::Object(map) => map.values().try_for_each(check_json),
        _ => Ok(()),
    }
}

/// Renders template `text` to a string.
pub(crate) fn render(text: &str, ctx: &Value) -> Result<String, String> {
    if !is_template(text) {
        return Ok(text.to_owned());
    }
    env().render_str(text, ctx).map_err(|e| describe(&e, text))
}

/// Renders every string of `value`; a string that is one expression keeps
/// the expression's type.
pub(crate) fn render_json(value: &Json, ctx: &Value) -> Result<Json, String> {
    Ok(match value {
        Json::String(s) if is_template(s) => match sole_expression(s) {
            Some(expr) => {
                let result = env()
                    .compile_expression(expr)
                    .and_then(|e| e.eval(ctx))
                    .map_err(|e| describe(&e, s))?;
                if result.is_undefined() {
                    return Err(format!("`{s}`: `{expr}` is undefined"));
                }
                serde_json::to_value(&result).map_err(|e| format!("`{s}`: {e}"))?
            }
            None => Json::String(render(s, ctx)?),
        },
        Json::Array(items) => Json::Array(
            items
                .iter()
                .map(|v| render_json(v, ctx))
                .collect::<Result<_, _>>()?,
        ),
        Json::Object(map) => Json::Object(
            map.iter()
                .map(|(k, v)| Ok((k.clone(), render_json(v, ctx)?)))
                .collect::<Result<_, String>>()?,
        ),
        other => other.clone(),
    })
}

fn describe(e: &minijinja::Error, text: &str) -> String {
    let what = match e.detail() {
        Some(detail) => format!("{}: {detail}", e.kind()),
        None => e.kind().to_string(),
    };
    let shown: String = text.chars().take(80).collect();
    let more = if shown.len() < text.len() { "…" } else { "" };
    format!("template `{shown}{more}`: {what}")
}

/// What a template can use about the request and the moment.
#[derive(Debug, Default)]
pub(crate) struct Vars {
    pub(crate) method: String,
    pub(crate) url: String,
    pub(crate) path: String,
    pub(crate) params: BTreeMap<String, String>,
    pub(crate) query: BTreeMap<String, String>,
    /// Lower-case names.
    pub(crate) headers: BTreeMap<String, String>,
    pub(crate) body: String,
    /// The body, if it is JSON.
    pub(crate) json: Option<Json>,
    /// The body's fields, if it is a form.
    pub(crate) form: Option<BTreeMap<String, String>>,
    /// The mock's hits, this one included.
    pub(crate) seq: u64,
    /// Now (the frozen time, if the session froze it), ms since the epoch.
    pub(crate) now_ms: i64,
    /// Seeds `uuid` and `faker`.
    pub(crate) seed: u64,
    /// The named values (`[state]`), before this answer changes them.
    pub(crate) state: BTreeMap<String, Json>,
}

impl Vars {
    /// The template context.
    pub(crate) fn context(&self) -> Value {
        let mut rng = Rng::new(self.seed);
        let uuid = uuid(&mut rng);
        let faker = faker(&mut rng);
        let mut ctx: BTreeMap<&str, Value> = BTreeMap::from([
            ("method", Value::from(self.method.clone())),
            ("url", Value::from(self.url.clone())),
            ("path", Value::from(self.path.clone())),
            ("params", Value::from_serialize(&self.params)),
            ("query", Value::from_serialize(&self.query)),
            ("headers", Value::from_serialize(&self.headers)),
            ("body", Value::from(self.body.clone())),
            ("seq", Value::from(self.seq)),
            ("now", Value::from(iso8601(self.now_ms))),
            ("now_ms", Value::from(self.now_ms)),
            ("uuid", Value::from(uuid)),
            ("faker", Value::from_serialize(&faker)),
            ("state", Value::from_serialize(&self.state)),
        ]);
        if let Some(json) = &self.json {
            ctx.insert("json", Value::from_serialize(json));
        }
        if let Some(form) = &self.form {
            ctx.insert("form", Value::from_serialize(form));
        }
        Value::from_serialize(&ctx)
    }
}

/// SplitMix64: small, fast and seedable, so reruns give the same values.
#[derive(Debug, Clone)]
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..n` (`n` > 0).
    pub(crate) fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }

    fn pick<'a>(&mut self, list: &[&'a str]) -> &'a str {
        let i = usize::try_from(self.below(list.len() as u64)).unwrap_or(0);
        list[i]
    }
}

/// A version 4 UUID from `rng`.
pub(crate) fn uuid(rng: &mut Rng) -> String {
    let hi = rng.next_u64();
    let lo = rng.next_u64();
    let hi = (hi & 0xFFFF_FFFF_FFFF_0FFF) | 0x0000_0000_0000_4000;
    let lo = (lo & 0x3FFF_FFFF_FFFF_FFFF) | 0x8000_0000_0000_0000;
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        hi >> 32,
        (hi >> 16) & 0xFFFF,
        hi & 0xFFFF,
        lo >> 48,
        lo & 0xFFFF_FFFF_FFFF
    )
}

const FIRST: &[&str] = &[
    "Ada",
    "Alan",
    "Grace",
    "Linus",
    "Margaret",
    "Ken",
    "Barbara",
    "Dennis",
    "Frances",
    "Edsger",
    "Radia",
    "Tim",
    "Hedy",
    "John",
    "Katherine",
    "Niklaus",
];
const LAST: &[&str] = &[
    "Lovelace",
    "Turing",
    "Hopper",
    "Torvalds",
    "Hamilton",
    "Thompson",
    "Liskov",
    "Ritchie",
    "Allen",
    "Dijkstra",
    "Perlman",
    "Berners-Lee",
    "Lamarr",
    "Backus",
    "Johnson",
    "Wirth",
];
const CITIES: &[&str] = &[
    "Oslo",
    "Lisbon",
    "Kyoto",
    "Austin",
    "Nairobi",
    "Toronto",
    "Melbourne",
    "Valparaiso",
];
const COUNTRIES: &[&str] = &[
    "Norway",
    "Portugal",
    "Japan",
    "United States",
    "Kenya",
    "Canada",
    "Australia",
    "Chile",
];
const COMPANIES: &[&str] = &[
    "Acme Corp",
    "Globex",
    "Initech",
    "Umbrella",
    "Hooli",
    "Stark Industries",
    "Wayne Enterprises",
    "Tyrell",
];
const STREETS: &[&str] = &[
    "Main St", "Oak Ave", "Elm St", "Maple Rd", "Cedar Ln", "Park Ave", "Lake Dr", "Hill St",
];
const WORDS: &[&str] = &[
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet",
];

/// `faker`: one made-up person (and a few things around them) per answer,
/// so `{{faker.name}}` and `{{faker.email}}` agree.
fn faker(rng: &mut Rng) -> BTreeMap<&'static str, String> {
    let first = rng.pick(FIRST);
    let last = rng.pick(LAST);
    let user = format!("{}.{}", first, last.replace('-', "")).to_ascii_lowercase();
    BTreeMap::from([
        ("first_name", first.to_owned()),
        ("last_name", last.to_owned()),
        ("name", format!("{first} {last}")),
        ("username", user.clone()),
        ("email", format!("{user}@example.com")),
        ("city", rng.pick(CITIES).to_owned()),
        ("country", rng.pick(COUNTRIES).to_owned()),
        ("company", rng.pick(COMPANIES).to_owned()),
        (
            "street",
            format!("{} {}", 1 + rng.below(999), rng.pick(STREETS)),
        ),
        ("phone", format!("+1 555-01{:02}", rng.below(100))),
        ("word", rng.pick(WORDS).to_owned()),
        ("number", (1 + rng.below(1000)).to_string()),
    ])
}

/// `2026-01-01T10:00:00.000Z`.
pub(crate) fn iso8601(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let rest = ms.rem_euclid(86_400_000);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1000 % 60,
        rest % 1000
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn vars() -> Vars {
        Vars {
            method: "POST".into(),
            params: BTreeMap::from([("id".into(), "7".into())]),
            query: BTreeMap::from([("q".into(), "shoes".into())]),
            headers: BTreeMap::from([("x-token".into(), "abc".into())]),
            json: Some(json!({"qty": 3, "user": {"email": "a@example.com"}})),
            seq: 2,
            now_ms: 1_767_261_600_000,
            seed: 1,
            ..Vars::default()
        }
    }

    #[test]
    fn strings_render() {
        let ctx = vars().context();
        assert_eq!(render("id {{params.id}}", &ctx).unwrap(), "id 7");
        assert_eq!(render("{{query.q}}-{{seq}}", &ctx).unwrap(), "shoes-2");
        assert_eq!(render(r#"{{headers["x-token"]}}"#, &ctx).unwrap(), "abc");
        assert_eq!(
            render("{{json.user.email}}", &ctx).unwrap(),
            "a@example.com"
        );
        assert_eq!(render("{{now}}", &ctx).unwrap(), "2026-01-01T10:00:00.000Z");
        assert_eq!(render("plain {text}", &ctx).unwrap(), "plain {text}");
    }

    #[test]
    fn undefined_values_are_errors() {
        let ctx = vars().context();
        let err = render("{{params.nope}}", &ctx).unwrap_err();
        assert!(err.contains("undefined"), "{err}");
        let err = render_json(&json!({"a": "{{json.missing}}"}), &ctx).unwrap_err();
        assert!(err.contains("undefined"), "{err}");
        assert!(check("{{ oops").is_err());
        assert!(check("{% if %}").is_err());
        assert!(check("{{ params.id }} and {{ uuid }}").is_ok());
    }

    #[test]
    fn a_sole_expression_keeps_its_type() {
        let ctx = vars().context();
        let out = render_json(
            &json!({"qty": "{{json.qty}}", "text": "n={{json.qty}}", "user": "{{ json.user }}", "n": 1}),
            &ctx,
        )
        .unwrap();
        assert_eq!(
            out,
            json!({"qty": 3, "text": "n=3", "user": {"email": "a@example.com"}, "n": 1})
        );
    }

    #[test]
    fn random_values_follow_the_seed() {
        let a = render("{{uuid}} {{faker.name}} {{faker.email}}", &vars().context()).unwrap();
        let b = render("{{uuid}} {{faker.name}} {{faker.email}}", &vars().context()).unwrap();
        assert_eq!(a, b);
        let uuid = a.split(' ').next().unwrap();
        assert_eq!(uuid.len(), 36);
        assert_eq!(&uuid[14..15], "4");
        let other = Vars { seed: 2, ..vars() };
        assert_ne!(render("{{uuid}}", &other.context()).unwrap(), uuid);
    }

    #[test]
    fn iso_dates() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso8601(951_782_400_123), "2000-02-29T00:00:00.123Z");
        assert_eq!(iso8601(-1), "1969-12-31T23:59:59.999Z");
    }
}

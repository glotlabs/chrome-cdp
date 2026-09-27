use std::path::Path;

use super::*;

const BROWSER: &str = r#"{"domains": [
  {"domain": "Alpha", "description": "The alpha domain.",
   "types": [
     {"id": "Id", "type": "string"},
     {"id": "Mode", "type": "string", "enum": ["fast", "slow-ish"]},
     {"id": "Node", "type": "object", "properties": [
       {"name": "id", "$ref": "Id"},
       {"name": "parent", "$ref": "Node", "optional": true},
       {"name": "children", "type": "array", "items": {"$ref": "Node"}},
       {"name": "type", "type": "string"},
       {"name": "baseURL", "type": "string", "optional": true},
       {"name": "origin", "$ref": "Beta.Origin"}
     ]}
   ],
   "commands": [
     {"name": "doThing", "description": "Does a thing.", "experimental": true,
      "parameters": [
        {"name": "target", "$ref": "Id"},
        {"name": "speed", "type": "string", "enum": ["low", "unknown"], "optional": true}
      ],
      "returns": [{"name": "count", "type": "integer"}]},
     {"name": "enable"}
   ],
   "events": [
     {"name": "node", "parameters": [{"name": "value", "type": "number"}]},
     {"name": "fired"}
   ]},
  {"domain": "Beta",
   "types": [
     {"id": "Origin", "type": "string"},
     {"id": "Unused", "type": "integer"}
   ],
   "commands": [{"name": "ping"}]},
  {"domain": "Gamma", "commands": [
     {"name": "keep"}, {"name": "drop"}
   ], "events": [{"name": "gone"}]}
]}"#;

fn gen_with(domains: &str) -> Result<BTreeMap<String, String>, Error> {
    generate(Sources {
        browser_protocol: BROWSER,
        js_protocol: r#"{"domains": []}"#,
        domains,
        protocol_version: "# x\nrevision = 42\n",
    })
}

fn generated(domains: &str) -> BTreeMap<String, String> {
    gen_with(domains).unwrap()
}

#[test]
fn mod_rs_lists_modules_and_revision() {
    let files = generated("Alpha\n");
    let mod_rs = &files["mod.rs"];
    assert!(mod_rs.contains("pub const PROTOCOL_REVISION: u32 = 42;"));
    assert!(mod_rs.contains("pub mod alpha;\npub mod beta;\n"));
    assert!(!mod_rs.contains("gamma"));
}

#[test]
fn referenced_types_of_unlisted_domains_are_generated_alone() {
    let files = generated("Alpha\n");
    let beta = &files["beta.rs"];
    assert!(beta.contains("pub type Origin = String;"));
    assert!(!beta.contains("Unused"), "{beta}");
    assert!(!beta.contains("PingParams"), "{beta}");
    assert!(beta.contains("is not listed"));
    assert!(files["alpha.rs"].contains("pub origin: super::beta::Origin,"));
}

#[test]
fn partial_domains_generate_only_listed_items() {
    let files = generated("Gamma: keep\n");
    let gamma = &files["gamma.rs"];
    assert!(gamma.contains("pub struct KeepParams {}"));
    assert!(!gamma.contains("DropParams"));
    assert!(!gamma.contains("Gone"));
}

#[test]
fn unknown_allowlist_entries_are_errors() {
    let err = gen_with("Gamma: nope\n").unwrap_err();
    assert!(
        err.to_string().contains("no command or event `nope`"),
        "{err}"
    );
    let err = gen_with("Delta\n").unwrap_err();
    assert!(err.to_string().contains("unknown domain `Delta`"), "{err}");
}

#[test]
fn structs_have_serde_attributes_and_boxed_recursion() {
    let alpha = &generated("Alpha\n")["alpha.rs"];
    for want in [
        "#[serde(rename_all = \"camelCase\")]\npub struct Node {",
        "#[serde(default, skip_serializing_if = \"Option::is_none\")]\npub parent: Option<Box<Node>>,",
        "pub children: Vec<Node>,",
        "pub r#type: String,",
        "#[serde(rename = \"baseURL\")]\n#[serde(default, skip_serializing_if = \"Option::is_none\")]\npub base_url: Option<String>,",
    ] {
        assert!(alpha.contains(want), "missing {want:?} in\n{alpha}");
    }
}

#[test]
fn enums_get_a_catch_all_last_variant() {
    let alpha = &generated("Alpha\n")["alpha.rs"];
    assert!(
        alpha.contains(
            "pub enum Mode {\n#[serde(rename = \"fast\")]\nFast,\n#[serde(rename = \"slow-ish\")]\n\
             SlowIsh,\n/// A value this version of the protocol does not know (sent by a newer \
             Chrome).\n#[serde(other)]\nUnknown,\n}"
        ),
        "{alpha}"
    );
    // A protocol `unknown` value becomes the catch-all itself.
    assert!(
        alpha.contains(
            "pub enum DoThingSpeed {\n#[serde(rename = \"low\")]\nLow,\n\
             #[serde(rename = \"unknown\", other)]\nUnknown,\n}"
        ),
        "{alpha}"
    );
}

#[test]
fn commands_and_events_implement_the_traits() {
    let alpha = &generated("Alpha\n")["alpha.rs"];
    for want in [
        "/// Does a thing.\n///\n/// **Experimental.**\n///\n/// Method: `Alpha.doThing`.",
        "pub fn new(target: Id) -> Self {\nSelf { target, speed: None }\n}",
        "impl crate::Command for DoThingParams {\nconst METHOD: &'static str = \"Alpha.doThing\";\n\
         type Returns = DoThingReturns;\n}",
        "pub struct DoThingReturns {\npub count: i64,\n}",
        "Default, serde::Serialize, serde::Deserialize)]\n#[serde(rename_all = \"camelCase\")]\n\
         pub struct EnableParams {}",
        // `node` would collide with the `Node` type.
        "impl crate::Event for NodeEvent {\nconst NAME: &'static str = \"Alpha.node\";\n}",
        "impl crate::Event for Fired {",
    ] {
        assert!(alpha.contains(want), "missing {want:?} in\n{alpha}");
    }
}

#[test]
fn missing_revision_is_an_error() {
    let err = generate(Sources {
        browser_protocol: BROWSER,
        js_protocol: r#"{"domains": []}"#,
        domains: "Alpha\n",
        protocol_version: "rev = 1\n",
    })
    .unwrap_err();
    assert!(err.to_string().contains("revision"), "{err}");
}

/// `cargo xtask gen-cdp --check` also runs in CI, but catching drift in
/// `cargo test` gives a faster signal.
#[test]
fn committed_code_matches_the_generator() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let layout = Layout::new(&root);
    let files = generate_formatted(&layout).unwrap();
    let drift = check(&layout, &files).unwrap();
    assert!(
        drift.is_empty(),
        "generated CDP code is out of date; run `cargo xtask gen-cdp`:\n{drift}"
    );
}

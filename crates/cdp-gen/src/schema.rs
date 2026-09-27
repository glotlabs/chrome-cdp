//! The shape of `browser_protocol.json` / `js_protocol.json`.
//!
//! Only the keys the generator uses are modelled; unknown keys are ignored so
//! a newer protocol file still parses.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct Protocol {
    pub(crate) domains: Vec<Domain>,
}

#[derive(Debug, Deserialize)]
#[allow(clippy::struct_field_names)] // `domain` is the JSON key.
pub(crate) struct Domain {
    pub(crate) domain: String,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) experimental: bool,
    #[serde(default)]
    pub(crate) deprecated: bool,
    #[serde(default)]
    pub(crate) types: Vec<TypeDef>,
    #[serde(default)]
    pub(crate) commands: Vec<Command>,
    #[serde(default)]
    pub(crate) events: Vec<Event>,
}

/// A named type in a domain's `types` list.
#[derive(Debug, Deserialize)]
pub(crate) struct TypeDef {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) experimental: bool,
    #[serde(default)]
    pub(crate) deprecated: bool,
    #[serde(flatten)]
    pub(crate) ty: Type,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Command {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) experimental: bool,
    #[serde(default)]
    pub(crate) deprecated: bool,
    #[serde(default)]
    pub(crate) parameters: Vec<Property>,
    #[serde(default)]
    pub(crate) returns: Vec<Property>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Event {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) experimental: bool,
    #[serde(default)]
    pub(crate) deprecated: bool,
    #[serde(default)]
    pub(crate) parameters: Vec<Property>,
}

/// An object property, command parameter/return value, or event parameter.
#[derive(Debug, Deserialize)]
pub(crate) struct Property {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) optional: bool,
    #[serde(default)]
    pub(crate) experimental: bool,
    #[serde(default)]
    pub(crate) deprecated: bool,
    #[serde(flatten)]
    pub(crate) ty: Type,
}

/// A type expression: either a `$ref` to a named type or an inline type.
#[derive(Debug, Deserialize)]
pub(crate) struct Type {
    #[serde(rename = "$ref", default)]
    pub(crate) reference: Option<String>,
    #[serde(rename = "type", default)]
    pub(crate) kind: Option<String>,
    #[serde(rename = "enum", default)]
    pub(crate) enum_values: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) items: Option<Box<Type>>,
    #[serde(default)]
    pub(crate) properties: Option<Vec<Property>>,
}

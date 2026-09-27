//! Turns a selected [`Module`] into Rust source (unformatted; `rustfmt` runs
//! afterwards).

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use heck::{ToSnakeCase, ToUpperCamelCase};

use crate::Error;
use crate::schema::{Property, Type, TypeDef};
use crate::select::{Index, Module};

/// A named type: `(domain, id)`.
type TypeKey<'a> = (&'a str, &'a str);

/// Object type → object types it holds by value (not via `Vec`).
pub(crate) type Containment<'a> = HashMap<TypeKey<'a>, Vec<TypeKey<'a>>>;

/// Names the generated code uses unqualified; a protocol type with one of
/// these names would shadow them.
const RESERVED_TYPE_NAMES: &[&str] = &["Box", "Option", "Self", "String", "Vec"];

/// The Rust module name for a CDP domain (`DOMSnapshot` → `dom_snapshot`).
pub(crate) fn module_name(domain: &str) -> String {
    domain.to_snake_case()
}

/// What serde's `rename_all = "camelCase"` turns a snake_case field into.
fn serde_camel(snake: &str) -> String {
    let mut out = String::with_capacity(snake.len());
    let mut upper = false;
    for (i, c) in snake.chars().enumerate() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.push(c.to_ascii_uppercase());
            upper = false;
        } else if i == 0 {
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// A Rust field name for a protocol property name, and whether it needs an
/// explicit `#[serde(rename)]` to keep the wire name.
fn field_name(name: &str) -> (String, bool) {
    let snake = name.to_snake_case();
    let needs_rename = serde_camel(&snake) != name;
    let ident = match snake.as_str() {
        "self" | "super" | "crate" => format!("{snake}_"),
        s if is_keyword(s) => format!("r#{s}"),
        _ => snake,
    };
    // `name_` changes what serde sees, so it always needs the rename.
    (ident.clone(), needs_rename || ident.ends_with('_'))
}

fn is_keyword(s: &str) -> bool {
    matches!(
        s,
        "as" | "async"
            | "await"
            | "box"
            | "break"
            | "const"
            | "continue"
            | "do"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "final"
            | "fn"
            | "for"
            | "gen"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "macro"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "override"
            | "priv"
            | "pub"
            | "ref"
            | "return"
            | "static"
            | "struct"
            | "trait"
            | "true"
            | "try"
            | "type"
            | "typeof"
            | "unsafe"
            | "unsized"
            | "use"
            | "virtual"
            | "where"
            | "while"
            | "yield"
    )
}

fn variant_name(value: &str) -> String {
    match value.to_upper_camel_case() {
        v if v == "Self" => "SelfValue".to_owned(),
        v => v,
    }
}

/// Writes `///` lines for a description plus experimental/deprecated notes.
fn doc(out: &mut String, description: Option<&str>, experimental: bool, deprecated: bool) {
    let mut lines: Vec<&str> = Vec::new();
    if let Some(d) = description {
        // Trim indentation: rustdoc would read indented lines as doctests.
        lines.extend(d.trim().lines().map(str::trim));
    }
    let mut notes = Vec::new();
    if experimental {
        notes.push("**Experimental.**");
    }
    if deprecated {
        notes.push("**Deprecated.**");
    }
    let notes = notes.join(" ");
    if !notes.is_empty() {
        if !lines.is_empty() {
            lines.push("");
        }
        lines.push(&notes);
    }
    for line in lines {
        if line.is_empty() {
            out.push_str("///\n");
        } else {
            let _ = writeln!(out, "/// {line}");
        }
    }
}

/// A generated struct field (the Rust type excludes the `Option`).
struct Field {
    ident: String,
    ty: String,
    optional: bool,
}

/// A string enum declared inline on a property; emitted after its owner.
struct InlineEnum<'s> {
    name: String,
    values: &'s [String],
    description: Option<&'s str>,
}

struct Emitter<'a, 's> {
    index: &'s Index<'a>,
    domain: &'a str,
    out: String,
    /// Every item name declared in this module, to catch collisions.
    declared: HashSet<String>,
    pending: Vec<InlineEnum<'s>>,
    contains: &'s Containment<'a>,
}

impl<'a, 's> Emitter<'a, 's>
where
    'a: 's,
{
    fn declare(&mut self, name: &str) -> Result<(), Error> {
        if RESERVED_TYPE_NAMES.contains(&name) || !self.declared.insert(name.to_owned()) {
            return Err(Error(format!(
                "`{}` would declare `{name}` twice (or shadow a std type); \
                 teach cdp-gen a naming rule for it",
                self.domain
            )));
        }
        Ok(())
    }

    /// The Rust type for a protocol type expression. `owner` + `field` name
    /// any inline enum.
    fn rust_type(&mut self, owner: &str, field: &str, ty: &'s Type) -> Result<String, Error> {
        if let Some(reference) = &ty.reference {
            let (d, t) = self.index.resolve(self.domain, reference)?;
            return Ok(if d == self.domain {
                t.id.clone()
            } else {
                format!("super::{}::{}", module_name(d), t.id)
            });
        }
        let kind = ty.kind.as_deref().unwrap_or_default();
        Ok(match kind {
            "string" => match &ty.enum_values {
                Some(values) => {
                    let name = format!("{owner}{}", field.to_upper_camel_case());
                    self.pending.push(InlineEnum {
                        name: name.clone(),
                        values,
                        description: None,
                    });
                    name
                }
                None => "String".to_owned(),
            },
            "integer" => "i64".to_owned(),
            "number" => "f64".to_owned(),
            "boolean" => "bool".to_owned(),
            "any" => "serde_json::Value".to_owned(),
            "object" if ty.properties.is_none() => {
                "serde_json::Map<String, serde_json::Value>".to_owned()
            }
            "array" => {
                let items = ty.items.as_deref().ok_or_else(|| {
                    Error(format!(
                        "`{}.{owner}.{field}`: array without items",
                        self.domain
                    ))
                })?;
                format!("Vec<{}>", self.rust_type(owner, field, items)?)
            }
            other => {
                return Err(Error(format!(
                    "`{}.{owner}.{field}`: unsupported type `{other}`",
                    self.domain
                )));
            }
        })
    }

    /// True if a by-value field of type `to` inside `from` makes `from`
    /// infinitely sized, so the field must be boxed.
    fn needs_box(&self, from: TypeKey<'a>, to: TypeKey<'a>) -> bool {
        let mut seen = HashSet::new();
        let mut stack = vec![to];
        while let Some(node) = stack.pop() {
            if node == from {
                return true;
            }
            if seen.insert(node) {
                stack.extend(self.contains.get(&node).into_iter().flatten().copied());
            }
        }
        false
    }

    /// Emits a struct with one field per property.
    /// `self_key` is `Some` for named object types, to detect recursion.
    fn emit_struct(
        &mut self,
        name: &str,
        enum_prefix: &str,
        props: &'s [Property],
        self_key: Option<TypeKey<'a>>,
        derive_default: bool,
    ) -> Result<Vec<Field>, Error> {
        self.declare(name)?;
        let derives = if derive_default {
            "Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize"
        } else {
            "Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize"
        };
        let _ = writeln!(self.out, "#[derive({derives})]");
        self.out.push_str("#[serde(rename_all = \"camelCase\")]\n");
        if props.is_empty() {
            let _ = writeln!(self.out, "pub struct {name} {{}}\n");
            return Ok(Vec::new());
        }
        let _ = writeln!(self.out, "pub struct {name} {{");
        let mut fields: Vec<Field> = Vec::new();
        for p in props {
            let (field, rename) = field_name(&p.name);
            if fields.iter().any(|f| f.ident == field) {
                return Err(Error(format!(
                    "`{}.{name}` has two fields named `{field}`",
                    self.domain
                )));
            }
            let mut ty = self.rust_type(enum_prefix, &p.name, &p.ty)?;
            if let (Some(from), Some(reference)) = (self_key, &p.ty.reference) {
                let (d, t) = self.index.resolve(self.domain, reference)?;
                if self.needs_box(from, (d, &t.id)) {
                    ty = format!("Box<{ty}>");
                }
            }
            doc(
                &mut self.out,
                p.description.as_deref(),
                p.experimental,
                p.deprecated,
            );
            if rename {
                let _ = writeln!(self.out, "#[serde(rename = \"{}\")]", p.name);
            }
            if p.optional {
                self.out
                    .push_str("#[serde(default, skip_serializing_if = \"Option::is_none\")]\n");
                let _ = writeln!(self.out, "pub {field}: Option<{ty}>,");
            } else {
                let _ = writeln!(self.out, "pub {field}: {ty},");
            }
            fields.push(Field {
                ident: field,
                ty,
                optional: p.optional,
            });
        }
        self.out.push_str("}\n\n");
        Ok(fields)
    }

    fn emit_enum(&mut self, e: &InlineEnum<'s>) -> Result<(), Error> {
        self.declare(&e.name)?;
        let mut variants = HashSet::new();
        let mut unknown_value = None;
        let mut body = String::new();
        for value in e.values {
            let variant = variant_name(value);
            if !variants.insert(variant.clone()) {
                return Err(Error(format!(
                    "`{}.{}`: two values map to variant `{variant}`",
                    self.domain, e.name
                )));
            }
            if variant == "Unknown" {
                unknown_value = Some(value);
            } else {
                let _ = writeln!(body, "#[serde(rename = \"{value}\")]\n{variant},");
            }
        }
        // `#[serde(other)]` must be on the last variant.
        match unknown_value {
            // The protocol's own `unknown` also catches unknown values.
            Some(value) => {
                let _ = writeln!(body, "#[serde(rename = \"{value}\", other)]\nUnknown,");
            }
            None => body.push_str(
                "/// A value this version of the protocol does not know (sent by a newer \
                 Chrome).\n#[serde(other)]\nUnknown,\n",
            ),
        }
        doc(&mut self.out, e.description, false, false);
        self.out.push_str(
            "#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, \
             serde::Deserialize)]\n",
        );
        let _ = writeln!(self.out, "pub enum {} {{\n{body}}}\n", e.name);
        Ok(())
    }

    fn flush_enums(&mut self) -> Result<(), Error> {
        for e in std::mem::take(&mut self.pending) {
            self.emit_enum(&e)?;
        }
        Ok(())
    }

    fn emit_type(&mut self, t: &'a TypeDef) -> Result<(), Error> {
        let kind = t.ty.kind.as_deref().unwrap_or_default();
        match (&t.ty.properties, &t.ty.enum_values) {
            (Some(props), _) if kind == "object" => {
                doc(
                    &mut self.out,
                    t.description.as_deref(),
                    t.experimental,
                    t.deprecated,
                );
                self.emit_struct(&t.id, &t.id, props, Some((self.domain, &t.id)), false)?;
            }
            (_, Some(values)) if kind == "string" => {
                let e = InlineEnum {
                    name: t.id.clone(),
                    values,
                    description: None,
                };
                doc(
                    &mut self.out,
                    t.description.as_deref(),
                    t.experimental,
                    t.deprecated,
                );
                self.emit_enum(&e)?;
            }
            _ => {
                self.declare(&t.id)?;
                let ty = self.rust_type(&t.id, "", &t.ty)?;
                doc(
                    &mut self.out,
                    t.description.as_deref(),
                    t.experimental,
                    t.deprecated,
                );
                let _ = writeln!(self.out, "pub type {} = {ty};\n", t.id);
            }
        }
        self.flush_enums()
    }
}

/// Maps each named object type to the object types it holds by value, for
/// deciding where recursion needs a `Box`.
pub(crate) fn containment<'a>(index: &Index<'a>) -> Result<Containment<'a>, Error> {
    let mut map = Containment::new();
    for (&(d, id), t) in &index.types {
        for p in t.ty.properties.iter().flatten() {
            if let Some(reference) = &p.ty.reference {
                let (rd, rt) = index.resolve(d, reference)?;
                if rt.ty.properties.is_some() {
                    map.entry((d, id)).or_default().push((rd, &rt.id));
                }
            }
        }
    }
    Ok(map)
}

/// Generates the source of one domain module.
pub(crate) fn module<'a>(
    index: &Index<'a>,
    contains: &Containment<'a>,
    m: &Module<'a>,
) -> Result<String, Error> {
    let domain: &'a str = &m.domain.domain;
    let mut e = Emitter {
        index,
        domain,
        out: String::new(),
        declared: HashSet::new(),
        pending: Vec::new(),
        contains,
    };

    let mut header = String::new();
    let _ = writeln!(header, "`{domain}` domain.");
    if let Some(d) = &m.domain.description {
        let _ = writeln!(header, "\n{}", d.trim());
    }
    if m.domain.experimental {
        header.push_str("\n**Experimental.**\n");
    }
    if m.domain.deprecated {
        header.push_str("\n**Deprecated.**\n");
    }
    if !m.listed {
        let _ = writeln!(
            header,
            "\nOnly the types other generated domains use: `{domain}` is not listed in \
             `crates/cdp/protocol/domains.txt`."
        );
    }
    for line in header.trim_end().lines() {
        let line = line.trim();
        if line.is_empty() {
            e.out.push_str("//!\n");
        } else {
            let _ = writeln!(e.out, "//! {line}");
        }
    }
    e.out
        .push_str("\n// @generated by `cargo xtask gen-cdp`. Do not edit.\n\n");

    for t in &m.types {
        e.emit_type(t)?;
    }

    // Declare type names first so event names can avoid them.
    let type_names: HashSet<&str> = m.types.iter().map(|t| t.id.as_str()).collect();

    for c in &m.commands {
        let base = c.name.to_upper_camel_case();
        let params = format!("{base}Params");
        let returns = format!("{base}Returns");
        let method = format!("{domain}.{}", c.name);
        let all_optional = c.parameters.iter().all(|p| p.optional);

        doc(
            &mut e.out,
            c.description.as_deref(),
            c.experimental,
            c.deprecated,
        );
        let _ = writeln!(e.out, "///\n/// Method: `{method}`.");
        let fields = e.emit_struct(&params, &base, &c.parameters, None, all_optional)?;
        e.flush_enums()?;
        if !all_optional {
            emit_constructor(&mut e.out, &params, &fields);
        }

        let _ = writeln!(e.out, "/// What `{method}` returns.");
        e.emit_struct(&returns, &base, &c.returns, None, false)?;
        e.flush_enums()?;

        let _ = writeln!(
            e.out,
            "impl crate::Command for {params} {{\n\
             const METHOD: &'static str = \"{method}\";\n\
             type Returns = {returns};\n}}\n"
        );
    }

    for ev in &m.events {
        let mut name = ev.name.to_upper_camel_case();
        if type_names.contains(name.as_str()) {
            name.push_str("Event");
        }
        let full = format!("{domain}.{}", ev.name);
        doc(
            &mut e.out,
            ev.description.as_deref(),
            ev.experimental,
            ev.deprecated,
        );
        let _ = writeln!(e.out, "///\n/// Event: `{full}`.");
        e.emit_struct(&name, &name, &ev.parameters, None, false)?;
        e.flush_enums()?;
        let _ = writeln!(
            e.out,
            "impl crate::Event for {name} {{\nconst NAME: &'static str = \"{full}\";\n}}\n"
        );
    }
    Ok(e.out)
}

/// `new(required...)` for a params struct; optional fields start as `None`.
fn emit_constructor(out: &mut String, name: &str, fields: &[Field]) {
    let args: Vec<String> = fields
        .iter()
        .filter(|f| !f.optional)
        .map(|f| format!("{}: {}", f.ident, f.ty))
        .collect();
    let inits: Vec<String> = fields
        .iter()
        .map(|f| {
            if f.optional {
                format!("{}: None", f.ident)
            } else {
                f.ident.clone()
            }
        })
        .collect();
    let _ = writeln!(
        out,
        "impl {name} {{\n\
         /// Sets the required fields; optional ones start as `None`.\n\
         pub fn new({}) -> Self {{\nSelf {{ {} }}\n}}\n}}\n",
        args.join(", "),
        inits.join(", ")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_names_are_snake_case() {
        assert_eq!(module_name("DOMSnapshot"), "dom_snapshot");
        assert_eq!(module_name("CSS"), "css");
        assert_eq!(module_name("IO"), "io");
        assert_eq!(module_name("Page"), "page");
    }

    #[test]
    fn field_names_keep_the_wire_name() {
        assert_eq!(
            field_name("backendNodeId"),
            ("backend_node_id".into(), false)
        );
        assert_eq!(field_name("baseURL"), ("base_url".into(), true));
        assert_eq!(field_name("type"), ("r#type".into(), false));
        assert_eq!(field_name("self"), ("self_".into(), true));
    }

    #[test]
    fn serde_camel_matches_serde() {
        assert_eq!(serde_camel("frame_id"), "frameId");
        assert_eq!(serde_camel("base_url"), "baseUrl");
        assert_eq!(serde_camel("x"), "x");
    }

    #[test]
    fn variant_names_avoid_self() {
        assert_eq!(variant_name("Self"), "SelfValue");
        assert_eq!(variant_name("insecure-broken"), "InsecureBroken");
        assert_eq!(
            variant_name("RenderFrameHostReused_SameSite"),
            "RenderFrameHostReusedSameSite"
        );
    }

    #[test]
    fn docs_drop_indentation() {
        let mut out = String::new();
        doc(&mut out, Some("A.\n    code?\n"), true, false);
        assert_eq!(out, "/// A.\n/// code?\n///\n/// **Experimental.**\n");
    }
}

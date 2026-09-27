//! Picks what to generate: the allow-listed domains, their commands and
//! events, and every named type those reach (from any domain).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::Error;
use crate::schema::{Command, Domain, Event, Type, TypeDef};

/// One line of `domains.txt`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Allowed {
    pub(crate) domain: String,
    /// `None` means the whole domain; `Some` lists commands and events.
    pub(crate) only: Option<Vec<String>>,
}

/// Parses `domains.txt`: one domain per line, `#` comments, and an optional
/// `: name, name` suffix that limits the domain to those commands/events.
pub(crate) fn parse_allowlist(text: &str) -> Result<Vec<Allowed>, Error> {
    let mut out: Vec<Allowed> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let at = |msg: String| Error(format!("domains.txt:{}: {msg}", i + 1));
        let (domain, only) = match line.split_once(':') {
            None => (line, None),
            Some((domain, items)) => {
                let items: Vec<String> = items
                    .split(',')
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .collect();
                if items.is_empty() {
                    return Err(at(format!("`{domain}:` lists no commands or events")));
                }
                (domain.trim(), Some(items))
            }
        };
        if !domain.chars().all(|c| c.is_ascii_alphanumeric()) || domain.is_empty() {
            return Err(at(format!("`{domain}` is not a domain name")));
        }
        if out.iter().any(|a| a.domain == domain) {
            return Err(at(format!("`{domain}` is listed twice")));
        }
        out.push(Allowed {
            domain: domain.to_owned(),
            only,
        });
    }
    Ok(out)
}

/// What one generated module contains, in protocol order.
#[derive(Debug)]
pub(crate) struct Module<'a> {
    pub(crate) domain: &'a Domain,
    /// Listed in `domains.txt` (as opposed to generated for its types only).
    pub(crate) listed: bool,
    pub(crate) types: Vec<&'a TypeDef>,
    pub(crate) commands: Vec<&'a Command>,
    pub(crate) events: Vec<&'a Event>,
}

/// Lookup of every domain and named type across both protocol files.
#[derive(Debug)]
pub(crate) struct Index<'a> {
    pub(crate) domains: HashMap<&'a str, &'a Domain>,
    pub(crate) types: HashMap<(&'a str, &'a str), &'a TypeDef>,
}

impl<'a> Index<'a> {
    pub(crate) fn new(domains: &'a [Domain]) -> Result<Self, Error> {
        let mut index = Index {
            domains: HashMap::new(),
            types: HashMap::new(),
        };
        for d in domains {
            if index.domains.insert(&d.domain, d).is_some() {
                return Err(Error(format!("domain `{}` is defined twice", d.domain)));
            }
            for t in &d.types {
                index.types.insert((&d.domain, &t.id), t);
            }
        }
        Ok(index)
    }

    /// Resolves a `$ref` written inside `domain` to `(domain, id)`.
    pub(crate) fn resolve(
        &self,
        domain: &'a str,
        reference: &'a str,
    ) -> Result<(&'a str, &'a TypeDef), Error> {
        let (d, id) = reference.split_once('.').unwrap_or((domain, reference));
        self.types
            .get(&(d, id))
            .map(|t| (d, *t))
            .ok_or_else(|| Error(format!("`{domain}` refers to unknown type `{reference}`")))
    }
}

/// Selects the modules to generate, keyed by domain name.
pub(crate) fn select<'a>(
    index: &Index<'a>,
    allowed: &[Allowed],
) -> Result<BTreeMap<&'a str, Module<'a>>, Error> {
    let mut modules: BTreeMap<&'a str, Module<'a>> = BTreeMap::new();
    // Named types to generate, found by walking `$ref`s from the roots.
    let mut reached: BTreeSet<(&'a str, &'a str)> = BTreeSet::new();
    let mut work: Vec<(&'a str, &'a Type)> = Vec::new();

    for a in allowed {
        let domain = *index
            .domains
            .get(a.domain.as_str())
            .ok_or_else(|| Error(format!("domains.txt lists unknown domain `{}`", a.domain)))?;
        let name = domain.domain.as_str();
        let wanted = |item: &str| a.only.as_ref().is_none_or(|o| o.iter().any(|x| x == item));
        if let Some(only) = &a.only {
            for item in only {
                let exists = domain.commands.iter().any(|c| &c.name == item)
                    || domain.events.iter().any(|e| &e.name == item);
                if !exists {
                    return Err(Error(format!(
                        "domains.txt: `{name}` has no command or event `{item}`"
                    )));
                }
            }
        }
        let module = Module {
            domain,
            listed: true,
            types: Vec::new(),
            commands: domain.commands.iter().filter(|c| wanted(&c.name)).collect(),
            events: domain.events.iter().filter(|e| wanted(&e.name)).collect(),
        };
        for c in &module.commands {
            work.extend(c.parameters.iter().map(|p| (name, &p.ty)));
            work.extend(c.returns.iter().map(|p| (name, &p.ty)));
        }
        for e in &module.events {
            work.extend(e.parameters.iter().map(|p| (name, &p.ty)));
        }
        if a.only.is_none() {
            for t in &domain.types {
                reached.insert((name, &t.id));
                work.push((name, &t.ty));
            }
        }
        modules.insert(name, module);
    }

    while let Some((domain, ty)) = work.pop() {
        if let Some(reference) = &ty.reference {
            let (d, t) = index.resolve(domain, reference)?;
            if reached.insert((d, &t.id)) {
                work.push((d, &t.ty));
            }
        }
        if let Some(items) = &ty.items {
            work.push((domain, items));
        }
        for p in ty.properties.iter().flatten() {
            work.push((domain, &p.ty));
        }
    }

    for &(d, _) in &reached {
        let domain = index.domains[d];
        modules.entry(d).or_insert_with(|| Module {
            domain,
            listed: false,
            types: Vec::new(),
            commands: Vec::new(),
            events: Vec::new(),
        });
    }
    for (name, module) in &mut modules {
        module.types = module
            .domain
            .types
            .iter()
            .filter(|t| reached.contains(&(*name, t.id.as_str())))
            .collect();
    }
    Ok(modules)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_parses_full_and_partial_domains() {
        let list = parse_allowlist("# c\nPage\n\nCSS: enable, getX # why\n").unwrap();
        assert_eq!(
            list,
            vec![
                Allowed {
                    domain: "Page".into(),
                    only: None
                },
                Allowed {
                    domain: "CSS".into(),
                    only: Some(vec!["enable".into(), "getX".into()])
                },
            ]
        );
    }

    #[test]
    fn allowlist_rejects_duplicates_and_junk() {
        let dup = parse_allowlist("Page\nPage\n").unwrap_err();
        assert!(dup.0.contains("domains.txt:2"), "{dup}");
        assert!(parse_allowlist("Pa ge\n").is_err());
        assert!(parse_allowlist("CSS:\n").is_err());
    }
}

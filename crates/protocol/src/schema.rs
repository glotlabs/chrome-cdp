//! JSON Schemas of each command's `--json` output (plan 08 §2).
//!
//! Each command's `data` is a type here (or a documented `Value`), and
//! [`COMMANDS`] lists every command with the schema of its envelope.
//! `chrome-cdp schema <command>` prints it. The schemas are stable from v1:
//! add fields, never remove or retype them.
//!
//! A new command adds its data type below and one [`CommandSchema`] entry;
//! the CLI's checklist test fails for a command that has none.

use schemars::{JsonSchema, Schema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{ErrorInfo, Truncation};

/// One command's entry in the schema registry.
#[derive(Debug, Clone, Copy)]
pub struct CommandSchema {
    /// The command as typed after `chrome-cdp`, e.g. `tab close`.
    pub name: &'static str,
    /// What `data` holds, in a few words.
    pub summary: &'static str,
    schema: fn() -> Schema,
}

impl CommandSchema {
    /// The JSON Schema of this command's `--json` envelope, titled
    /// `chrome-cdp <name> --json`.
    pub fn schema(&self) -> Value {
        let mut schema = (self.schema)().to_value();
        if let Some(root) = schema.as_object_mut() {
            root.insert(
                "title".into(),
                Value::String(format!("chrome-cdp {} --json", self.name)),
            );
        }
        schema
    }
}

/// Every command with a `--json` form, by name.
pub const COMMANDS: &[CommandSchema] = &[
    CommandSchema {
        name: "version",
        summary: "chrome-cdp --version: version, git commit and CDP protocol revision",
        schema: || schema_for!(Envelope<VersionData>),
    },
    CommandSchema {
        name: "schema",
        summary: "a command's JSON Schema, or the list of commands",
        schema: || schema_for!(Envelope<SchemaData>),
    },
    CommandSchema {
        name: "install",
        summary: "the browser installed, or with --list the installed and available ones",
        schema: || schema_for!(Envelope<InstallData>),
    },
    CommandSchema {
        name: "uninstall",
        summary: "the browser removed",
        schema: || schema_for!(Envelope<UninstallData>),
    },
    CommandSchema {
        name: "__direct cdp",
        summary: "the raw CDP result object (temporary; plan 02's cdp returns the same)",
        schema: || schema_for!(Envelope<Map<String, Value>>),
    },
];

/// The registry entry for `name` (`"tab close"`), if any.
pub fn command(name: &str) -> Option<&'static CommandSchema> {
    COMMANDS.iter().find(|c| c.name == name)
}

/// `data` of `chrome-cdp --version --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VersionData {
    /// The chrome-cdp version, e.g. `0.1.0`.
    pub version: String,
    /// The git commit it was built from (12 hex digits), or `unknown`.
    pub git_sha: String,
    /// The Chromium revision of the vendored CDP protocol.
    pub protocol_revision: u32,
}

/// `data` of `chrome-cdp schema --json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SchemaData {
    /// `chrome-cdp schema <command>`: that command's JSON Schema.
    Schema(Map<String, Value>),
    /// `chrome-cdp schema`: the commands that have one.
    List {
        /// Every command, in registry order.
        commands: Vec<SchemaListEntry>,
    },
}

/// One line of `chrome-cdp schema`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SchemaListEntry {
    /// The command, as passed to `chrome-cdp schema`.
    pub name: String,
    /// What its `data` holds.
    pub summary: String,
}

/// `data` of `chrome-cdp install --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum InstallData {
    /// `chrome-cdp install`: the browser now in the cache.
    Installed(InstalledBrowser),
    /// `chrome-cdp install --list`.
    List(InstallList),
}

/// A browser `chrome-cdp install` put in the cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstalledBrowser {
    /// `<channel>-<version>-<platform>`; `chrome-cdp uninstall` takes it.
    pub id: String,
    /// `stable`, `beta`, `dev`, `canary`, or `pinned` for a `--version` no
    /// channel is on.
    pub channel: String,
    /// E.g. `154.0.8037.57`.
    pub version: String,
    /// The Chrome for Testing platform, e.g. `linux64` or `mac-arm64`.
    pub platform: String,
    /// `chrome` or `chrome-headless-shell`.
    pub product: String,
    /// The binary to launch.
    pub path: String,
    /// Whether this run downloaded it; `false` when it was already there.
    pub downloaded: bool,
    /// The bytes downloaded (0 when nothing was).
    pub download_size: u64,
    /// System packages `--with-deps` installed.
    #[serde(default)]
    pub packages_installed: Vec<String>,
    /// The AppArmor profile `--with-deps` wrote so Chrome's sandbox works
    /// (Ubuntu 23.10+); absent when none was needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apparmor_profile: Option<String>,
}

/// `data` of `chrome-cdp install --list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstallList {
    /// This machine's Chrome for Testing platform; `null` where there is no
    /// build.
    pub platform: Option<String>,
    /// Complete installs for this platform, in the order discovery prefers
    /// them.
    pub installed: Vec<InstallListEntry>,
    /// Each channel's current version. Empty when the index cannot be
    /// fetched (a warning says why).
    pub available: Vec<AvailableBrowser>,
}

/// One install in `chrome-cdp install --list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstallListEntry {
    /// `<channel>-<version>-<platform>`.
    pub id: String,
    /// `stable`, `beta`, `dev`, `canary` or `pinned`.
    pub channel: String,
    /// E.g. `154.0.8037.57`.
    pub version: String,
    /// `chrome` and/or `chrome-headless-shell`.
    pub products: Vec<String>,
    /// The binary launched from this install (full Chrome if present).
    pub path: String,
}

/// A channel's current version in `chrome-cdp install --list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AvailableBrowser {
    /// `stable`, `beta`, `dev` or `canary`.
    pub channel: String,
    /// E.g. `154.0.8037.57`.
    pub version: String,
    /// The install id when this version is installed.
    pub installed: Option<String>,
}

/// `data` of `chrome-cdp uninstall --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UninstallData {
    /// The install id removed.
    pub id: String,
    /// The directory removed.
    pub dir: String,
}

/// The `--json` envelope (see [`crate::Envelope`]) with `data: T`.
#[derive(JsonSchema)]
#[serde(untagged)]
#[allow(dead_code)] // Only its schema is used.
enum Envelope<T> {
    /// The command succeeded.
    Ok(OkEnvelope<T>),
    /// The command failed.
    Err(ErrEnvelope),
}

#[derive(JsonSchema)]
#[allow(dead_code)]
struct OkEnvelope<T> {
    /// Always `true`.
    #[schemars(extend("const" = true))]
    ok: bool,
    /// The command's result.
    data: T,
    /// Non-fatal notes.
    #[serde(default)]
    warnings: Vec<String>,
    /// Page events observed while the command ran (dialogs, navigations, …).
    #[serde(default)]
    events: Vec<Value>,
    /// Values in `data` shortened to keep the output bounded; `--max-chars`
    /// (or the command's own flag, named in the warning) shows more.
    #[serde(default)]
    truncated: Vec<Truncation>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
struct ErrEnvelope {
    /// Always `false`.
    #[schemars(extend("const" = false))]
    ok: bool,
    /// What went wrong and what to run next.
    error: ErrorInfo,
    /// Non-fatal notes.
    #[serde(default)]
    warnings: Vec<String>,
    /// Page events observed while the command ran.
    #[serde(default)]
    events: Vec<Value>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{COMMANDS, VersionData, command};
    use crate::{Envelope, ErrorInfo, ErrorKind, Output};

    #[test]
    fn names_are_unique() {
        for (i, c) in COMMANDS.iter().enumerate() {
            assert!(
                COMMANDS[i + 1..].iter().all(|d| d.name != c.name),
                "{}",
                c.name
            );
        }
    }

    #[test]
    fn envelope_schema_describes_both_outcomes() {
        let schema = command("version").unwrap().schema();
        assert_eq!(schema["title"], json!("chrome-cdp version --json"));
        let text = schema.to_string();
        for needle in [
            r#""const":true"#,
            r#""const":false"#,
            r#""protocol_revision""#,
            r#""DialogOpen""#,
            r#""truncated""#,
        ] {
            assert!(text.contains(needle), "{needle} missing from {text}");
        }
        assert!(command("nope").is_none());
    }

    /// A tiny structural check: every `required` property of the matching
    /// branch is present. Enough to catch a renamed field.
    #[test]
    fn real_envelopes_have_the_required_fields() {
        let schema = command("version").unwrap().schema();
        let data = VersionData {
            version: "0.1.0".into(),
            git_sha: "abc".into(),
            protocol_revision: 1,
        };
        let ok = Envelope::from_output(Output::new(serde_json::to_value(&data).unwrap(), ""));
        let ok = serde_json::to_value(ok).unwrap();
        let err =
            serde_json::to_value(Envelope::err(ErrorInfo::new(ErrorKind::Timeout, "x"))).unwrap();
        let defs = &schema["$defs"];
        for (value, def) in [(&ok, "OkEnvelope"), (&err, "ErrEnvelope")] {
            let (_, branch) = defs
                .as_object()
                .unwrap()
                .iter()
                .find(|(k, _)| k.starts_with(def))
                .unwrap();
            for field in branch["required"].as_array().unwrap() {
                let field = field.as_str().unwrap();
                assert!(value.get(field).is_some(), "{def} needs {field}: {value}");
            }
        }
        for field in ["version", "git_sha", "protocol_revision"] {
            assert!(ok["data"].get(field).is_some());
        }
    }
}

//! How long each journey's iteration took in the last load run, by
//! backend (plan 23.3), in `<artifacts>/load/journeys.json`: `load --plan`
//! multiplies an open model's arrival rate by it for the users the run
//! is expected to reach, and the run starts that many users' Chromes at
//! once.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file's name in the load runs' folder.
pub(super) const FILE: &str = "journeys.json";

/// A journey's length in a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Length {
    /// The mean, from an iteration's start to its end, in ms.
    pub(super) mean_ms: f64,
    /// Over how many iterations.
    pub(super) iterations: u64,
    /// The run it is from.
    pub(super) run: String,
}

/// Journey lengths, by backend, then by journey (`file > test name`).
#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct Lengths {
    #[serde(flatten)]
    backends: BTreeMap<String, BTreeMap<String, Length>>,
    #[serde(skip)]
    path: PathBuf,
}

impl Lengths {
    /// The lengths in the load runs' folder `dir`; none when there is no
    /// readable file.
    pub(super) fn read(dir: &Path) -> Self {
        let path = dir.join(FILE);
        let mut lengths: Self = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        lengths.path = path;
        lengths
    }

    /// Journey `key`'s length on `backend`.
    pub(super) fn get(&self, backend: &str, key: &str) -> Option<&Length> {
        self.backends.get(backend)?.get(key)
    }

    /// Sets journey `key`'s length on `backend`.
    pub(super) fn set(&mut self, backend: &str, key: &str, length: Length) {
        self.backends
            .entry(backend.to_owned())
            .or_default()
            .insert(key.to_owned(), length);
    }

    /// Writes the file; a run is not worse for failing to.
    pub(super) fn write(&self) {
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&self.path, text + "\n");
        }
    }
}

/// A journey's key: `file > test name`, which a line moving does not
/// change.
pub(super) fn key(file: &str, name: &str) -> String {
    format!("{file} > {name}")
}

#[cfg(test)]
mod tests {
    use super::{Length, Lengths, key};

    #[test]
    fn lengths_are_kept_by_backend_and_journey() {
        let dir = tempfile::tempdir().unwrap();
        let mut lengths = Lengths::read(dir.path());
        assert!(lengths.get("local", "a").is_none());
        let length = Length {
            mean_ms: 1234.5,
            iterations: 10,
            run: "abc".into(),
        };
        lengths.set("local", &key("e2e/shop.cdp", "browse"), length.clone());
        lengths.write();
        let read = Lengths::read(dir.path());
        assert_eq!(read.get("local", "e2e/shop.cdp > browse"), Some(&length));
        assert!(read.get("staging", "e2e/shop.cdp > browse").is_none());
    }
}

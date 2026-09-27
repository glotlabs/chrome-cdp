//! The Chrome for Testing release index (plan 02, milestone 2.2): its JSON
//! endpoints, and picking the download for a channel or version, a product
//! and a platform. No I/O here; [`crate::install`] fetches the files.

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;

/// The Chrome for Testing JSON endpoints. `CHROME_CDP_CFT_URL` replaces it
/// (a mirror serves the same two files).
pub const DEFAULT_INDEX_URL: &str = "https://googlechromelabs.github.io/chrome-for-testing";

/// The current version of each channel.
pub(crate) const LAST_KNOWN_GOOD: &str = "last-known-good-versions-with-downloads.json";

/// Every version ever published, oldest first.
pub(crate) const KNOWN_GOOD: &str = "known-good-versions-with-downloads.json";

/// A Chrome release channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Channel {
    Stable,
    Beta,
    Dev,
    Canary,
}

impl Channel {
    /// Every channel, most stable first.
    pub const ALL: [Self; 4] = [Self::Stable, Self::Beta, Self::Dev, Self::Canary];

    /// `stable`, as used in install ids and on the command line.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
            Self::Dev => "dev",
            Self::Canary => "canary",
        }
    }

    /// `Stable`, the key in the index.
    fn index_key(self) -> &'static str {
        match self {
            Self::Stable => "Stable",
            Self::Beta => "Beta",
            Self::Dev => "Dev",
            Self::Canary => "Canary",
        }
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What to download: full Chrome, or the smaller `chrome-headless-shell`
/// (headless only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Product {
    Chrome,
    HeadlessShell,
}

impl Product {
    /// The key in the index's `downloads`, and the prefix of the zip's top
    /// directory: `chrome`, `chrome-headless-shell`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::HeadlessShell => "chrome-headless-shell",
        }
    }

    /// The directory the zip unpacks to, e.g. `chrome-linux64`. It sits
    /// directly in the install dir.
    pub fn dir(self, platform: &str) -> String {
        format!("{}-{platform}", self.as_str())
    }

    /// The binary to launch, relative to the install dir.
    pub fn binary(self, platform: &str) -> String {
        let dir = self.dir(platform);
        match self {
            Self::Chrome if platform.starts_with("mac") => format!(
                "{dir}/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"
            ),
            Self::Chrome => format!("{dir}/chrome"),
            Self::HeadlessShell => format!("{dir}/chrome-headless-shell"),
        }
    }

    /// For messages: `Chrome for Testing`, `chrome-headless-shell`.
    pub fn title(self) -> &'static str {
        match self {
            Self::Chrome => "Chrome for Testing",
            Self::HeadlessShell => "chrome-headless-shell",
        }
    }
}

/// One downloadable build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub product: Product,
    pub platform: String,
    pub url: String,
}

/// A channel's current version, from [`LAST_KNOWN_GOOD`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelVersion {
    pub channel: Channel,
    pub version: String,
    entry: Entry,
}

impl ChannelVersion {
    /// This channel's build of `product` for `platform`, if there is one.
    pub fn release(&self, product: Product, platform: &str) -> Option<Release> {
        self.entry.release(product, platform)
    }
}

/// The index has no usable answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IndexError {
    #[error("the Chrome for Testing index is not valid: {0}")]
    Invalid(String),
    #[error("the Chrome for Testing index has no {0} channel")]
    NoChannel(Channel),
    #[error("Chrome for Testing has no version {wanted}")]
    UnknownVersion {
        wanted: String,
        /// The newest version in the index.
        newest: Option<String>,
    },
    #[error("Chrome for Testing {version} has no {} build for {platform}", product.as_str())]
    NoBuild {
        version: String,
        product: Product,
        platform: String,
    },
}

#[derive(Debug, Deserialize)]
struct LastKnownGood {
    channels: BTreeMap<String, Entry>,
}

#[derive(Debug, Deserialize)]
struct KnownGood {
    versions: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Entry {
    version: String,
    #[serde(default)]
    downloads: BTreeMap<String, Vec<Download>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Download {
    platform: String,
    url: String,
}

impl Entry {
    fn release(&self, product: Product, platform: &str) -> Option<Release> {
        let download = self
            .downloads
            .get(product.as_str())?
            .iter()
            .find(|d| d.platform == platform)?;
        Some(Release {
            version: self.version.clone(),
            product,
            platform: platform.to_owned(),
            url: download.url.clone(),
        })
    }
}

/// Parses [`LAST_KNOWN_GOOD`]: the current version of each channel it
/// lists, in [`Channel::ALL`] order.
pub fn parse_channels(json: &str) -> Result<Vec<ChannelVersion>, IndexError> {
    let index: LastKnownGood =
        serde_json::from_str(json).map_err(|e| IndexError::Invalid(e.to_string()))?;
    Ok(Channel::ALL
        .into_iter()
        .filter_map(|channel| {
            let entry = index.channels.get(channel.index_key())?.clone();
            Some(ChannelVersion {
                channel,
                version: entry.version.clone(),
                entry,
            })
        })
        .collect())
}

/// The current `channel` build of `product` for `platform`.
pub fn channel_release(
    channels: &[ChannelVersion],
    channel: Channel,
    product: Product,
    platform: &str,
) -> Result<Release, IndexError> {
    let current = channels
        .iter()
        .find(|c| c.channel == channel)
        .ok_or(IndexError::NoChannel(channel))?;
    current
        .release(product, platform)
        .ok_or_else(|| IndexError::NoBuild {
            version: current.version.clone(),
            product,
            platform: platform.to_owned(),
        })
}

/// Picks from [`KNOWN_GOOD`] the newest version matching `wanted` that has
/// a build of `product` for `platform`. `wanted` is a full version
/// (`154.0.8037.57`) or a prefix of whole parts (`154`, `154.0.8037`).
pub fn version_release(
    json: &str,
    wanted: &str,
    product: Product,
    platform: &str,
) -> Result<Release, IndexError> {
    let index: KnownGood =
        serde_json::from_str(json).map_err(|e| IndexError::Invalid(e.to_string()))?;
    let prefix = parts(wanted);
    let mut matching: Vec<&Entry> = index
        .versions
        .iter()
        .filter(|e| {
            let version = parts(&e.version);
            !prefix.is_empty() && version.len() >= prefix.len() && version.starts_with(&prefix)
        })
        .collect();
    matching.sort_by_key(|e| parts(&e.version));
    let Some(newest_matching) = matching.last() else {
        return Err(IndexError::UnknownVersion {
            wanted: wanted.to_owned(),
            newest: index
                .versions
                .iter()
                .max_by_key(|e| parts(&e.version))
                .map(|e| e.version.clone()),
        });
    };
    matching
        .iter()
        .rev()
        .find_map(|e| e.release(product, platform))
        .ok_or_else(|| IndexError::NoBuild {
            version: newest_matching.version.clone(),
            product,
            platform: platform.to_owned(),
        })
}

/// `154.0.8037.57` → `[154, 0, 8037, 57]`; anything that is not dotted
/// numbers → empty.
pub(crate) fn parts(version: &str) -> Vec<u64> {
    version
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<_, _>>()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        Channel, IndexError, Product, channel_release, parse_channels, parts, version_release,
    };

    fn url(version: &str, platform: &str, product: &str) -> String {
        format!("https://cft.test/{version}/{platform}/{product}-{platform}.zip")
    }

    fn downloads(version: &str, platforms: &[&str], shell: bool) -> String {
        let list = |product: &str| {
            platforms
                .iter()
                .map(|p| {
                    format!(
                        r#"{{"platform":"{p}","url":"{}"}}"#,
                        url(version, p, product)
                    )
                })
                .collect::<Vec<_>>()
                .join(",")
        };
        let mut out = format!(r#""chrome":[{}],"chromedriver":[]"#, list("chrome"));
        if shell {
            out += r#","chrome-headless-shell":["#;
            out += &list("chrome-headless-shell");
            out += "]";
        }
        out
    }

    const ALL: &[&str] = &["linux-arm64", "linux64", "mac-arm64", "mac-x64", "win64"];

    fn last_known_good() -> String {
        let channel = |name: &str, version: &str| {
            format!(
                r#""{name}":{{"channel":"{name}","version":"{version}","revision":"1","downloads":{{{}}}}}"#,
                downloads(version, ALL, true)
            )
        };
        format!(
            r#"{{"timestamp":"2026-09-27T00:00:00Z","channels":{{{},{},{},{}}}}}"#,
            channel("Stable", "154.0.8037.57"),
            channel("Beta", "155.0.8059.12"),
            channel("Dev", "156.0.8072.0"),
            channel("Canary", "156.0.8075.0"),
        )
    }

    fn known_good() -> String {
        let versions = [
            // Old enough to have no headless shell and no arm64 Linux.
            ("113.0.5672.0", &["linux64", "mac-x64"][..], false),
            (
                "152.0.7900.1",
                &["linux64", "mac-arm64", "mac-x64"][..],
                true,
            ),
            ("154.0.8037.3", ALL, true),
            ("154.0.8037.57", ALL, true),
            ("154.0.8037.100", &["mac-arm64"][..], true),
            ("156.0.8075.0", ALL, true),
        ];
        let entries: Vec<String> = versions
            .iter()
            .map(|(v, platforms, shell)| {
                format!(
                    r#"{{"version":"{v}","revision":"1","downloads":{{{}}}}}"#,
                    downloads(v, platforms, *shell)
                )
            })
            .collect();
        format!(r#"{{"timestamp":"x","versions":[{}]}}"#, entries.join(","))
    }

    #[test]
    fn channels_from_the_last_known_good_index() {
        let channels = parse_channels(&last_known_good()).unwrap();
        let names: Vec<_> = channels
            .iter()
            .map(|c| (c.channel, c.version.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                (Channel::Stable, "154.0.8037.57"),
                (Channel::Beta, "155.0.8059.12"),
                (Channel::Dev, "156.0.8072.0"),
                (Channel::Canary, "156.0.8075.0"),
            ]
        );
        let release =
            channel_release(&channels, Channel::Beta, Product::HeadlessShell, "linux64").unwrap();
        assert_eq!(release.version, "155.0.8059.12");
        assert_eq!(
            release.url,
            url("155.0.8059.12", "linux64", "chrome-headless-shell")
        );
        assert_eq!(
            channel_release(&channels, Channel::Stable, Product::Chrome, "linux-riscv64"),
            Err(IndexError::NoBuild {
                version: "154.0.8037.57".into(),
                product: Product::Chrome,
                platform: "linux-riscv64".into()
            })
        );
        let partial = parse_channels(r#"{"channels":{"Stable":{"version":"1.2.3.4"}}}"#).unwrap();
        assert_eq!(
            channel_release(&partial, Channel::Dev, Product::Chrome, "linux64"),
            Err(IndexError::NoChannel(Channel::Dev))
        );
        assert!(matches!(
            parse_channels("<html>"),
            Err(IndexError::Invalid(_))
        ));
    }

    #[test]
    fn versions_match_whole_parts_newest_first() {
        let json = known_good();
        let pick = |wanted, product, platform| {
            version_release(&json, wanted, product, platform).map(|r| r.version)
        };
        assert_eq!(
            pick("154.0.8037.57", Product::Chrome, "linux64").unwrap(),
            "154.0.8037.57"
        );
        // 154.0.8037.100 is newer but has no linux64 build.
        assert_eq!(
            pick("154", Product::Chrome, "linux64").unwrap(),
            "154.0.8037.57"
        );
        assert_eq!(
            pick("154.0.8037", Product::Chrome, "mac-arm64").unwrap(),
            "154.0.8037.100"
        );
        // Whole parts only: 154.0.8037.5 is not a prefix of …57.
        assert_eq!(
            pick("154.0.8037.5", Product::Chrome, "linux64"),
            Err(IndexError::UnknownVersion {
                wanted: "154.0.8037.5".into(),
                newest: Some("156.0.8075.0".into())
            })
        );
        assert!(matches!(
            pick("latest", Product::Chrome, "linux64"),
            Err(IndexError::UnknownVersion { .. })
        ));
        assert_eq!(
            pick("113", Product::HeadlessShell, "linux64"),
            Err(IndexError::NoBuild {
                version: "113.0.5672.0".into(),
                product: Product::HeadlessShell,
                platform: "linux64".into()
            })
        );
        assert_eq!(
            pick("152", Product::Chrome, "linux-arm64")
                .unwrap_err()
                .to_string(),
            "Chrome for Testing 152.0.7900.1 has no chrome build for linux-arm64"
        );
    }

    #[test]
    fn product_layout() {
        assert_eq!(Product::Chrome.binary("linux64"), "chrome-linux64/chrome");
        assert_eq!(
            Product::Chrome.binary("mac-arm64"),
            "chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for \
             Testing"
        );
        assert_eq!(
            Product::HeadlessShell.binary("linux-arm64"),
            "chrome-headless-shell-linux-arm64/chrome-headless-shell"
        );
        assert_eq!(parts("154.0.8037.57"), [154, 0, 8037, 57]);
        assert!(parts("154.x").is_empty());
    }
}

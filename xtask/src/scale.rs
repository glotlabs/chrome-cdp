//! `cargo xtask scale-suite`: the scale suite of plan 22.1, a project of
//! N tests (1,000 by default) in `testdata/scale/` against e2e-shop's
//! built app and mocks, for tuning the runner on a suite of realistic
//! size. The mix is about 60% short tests (one `goto`, two `expect`s),
//! 30% medium (sign in, act, check) and 10% long (checkout); a quarter of
//! the files have an `@before-all`.
//!
//! The output depends only on N: the same N writes the same files. It is
//! git-ignored, not committed.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use super::Result;

/// Where the suite goes, from the workspace root.
pub(crate) const DIR: &str = "testdata/scale";
/// The e2e-shop project it copies the app and mocks from.
const SHOP: &str = "examples/e2e-shop";
/// What it copies from e2e-shop: the built app, the mocks, the API
/// they are checked against, the vars and the shared steps.
const COPIED: [&str; 5] = [
    "dist",
    "api",
    "e2e/mocks",
    "e2e/vars/mock.toml",
    "e2e/steps",
];
/// The port the app is served on (e2e-shop's own is 4173).
pub(crate) const PORT: u16 = 4183;
pub(crate) const DEFAULT_TESTS: usize = 1_000;
/// The fewest and most tests a file has.
const FILE_TESTS: (usize, usize) = (5, 40);

/// The products of e2e-shop's mocks: id, name, price in dollars.
const PRODUCTS: [(u32, &str, u32); 3] = [
    (1, "Mechanical keyboard", 89),
    (2, "Wireless mouse", 25),
    (3, "USB-C monitor", 249),
];

pub(crate) fn run(args: &[String]) -> Result {
    let usage = || "usage: cargo xtask scale-suite [--tests N]".to_owned();
    let tests = match args {
        [] => DEFAULT_TESTS,
        [flag, n] if flag == "--tests" => n
            .parse()
            .ok()
            .filter(|&n| n > 0)
            .ok_or_else(|| format!("--tests takes a positive number, got `{n}`"))?,
        _ => return Err(usage()),
    };
    let root = super::workspace_root();
    let files = generate(&root, tests)?;
    eprintln!(
        "wrote {tests} tests in {files} files to {DIR}; run them with `chrome-cdp test` there"
    );
    Ok(())
}

/// Writes the suite of `tests` tests into `root/testdata/scale`, after
/// removing what an earlier run wrote. Returns how many scenario files.
pub(crate) fn generate(root: &Path, tests: usize) -> Result<usize> {
    let dir = root.join(DIR);
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|e| format!("remove {}: {e}", dir.display()))?;
    }
    for path in COPIED {
        copy(&root.join(SHOP).join(path), &dir.join(path))?;
    }
    write(&dir.join("chrome-cdp.toml"), &config())?;
    write(&dir.join("README.md"), README)?;
    let scenarios = dir.join("e2e/scenarios");
    let files = scenarios_for(tests);
    for (i, text) in files.iter().enumerate() {
        write(&scenarios.join(format!("s{:04}.cdp", i + 1)), text)?;
    }
    Ok(files.len())
}

const README: &str = "\
# The scale suite

Written by `cargo xtask scale-suite` (plan 22.1): do not edit, and do not
commit. `cargo xtask bench scale` runs it; see docs/benchmarks.md.
";

fn config() -> String {
    format!(
        "\
# Written by `cargo xtask scale-suite`: the scale suite (plan 22.1).
[e2e]
base_url = \"http://localhost:{PORT}\"
tests = [\"e2e/scenarios/**/*.cdp\"]
mocks = \"e2e/mocks\"
default_mocks = [\"base\", \"third-party/payments\"]
openapi = \"api/openapi.yaml\"
vars = \"e2e/vars/mock.toml\"
artifacts = \"e2e/.results\"

[e2e.web_server]
static = \"dist\"
url = \"http://localhost:{PORT}\"
"
    )
}

/// How long a test is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Short,
    Medium,
    Long,
}

/// The scenario files of a suite of `tests` tests, in order.
fn scenarios_for(tests: usize) -> Vec<String> {
    let mut rng = Rng(0x22_0001);
    // Exact shares, then shuffled: 10% long, 30% medium, the rest short.
    let long = tests / 10;
    let medium = tests * 3 / 10;
    let mut kinds: Vec<Kind> = std::iter::repeat_n(Kind::Long, long)
        .chain(std::iter::repeat_n(Kind::Medium, medium))
        .chain(std::iter::repeat_n(Kind::Short, tests - long - medium))
        .collect();
    for i in (1..kinds.len()).rev() {
        kinds.swap(i, rng.below(i + 1));
    }
    let mut files = Vec::new();
    let mut next = 0;
    while next < kinds.len() {
        let size = FILE_TESTS.0 + rng.below(FILE_TESTS.1 - FILE_TESTS.0 + 1);
        let end = (next + size).min(kinds.len());
        let before_all = rng.below(4) == 0;
        let mut text = format!("# Scale suite file {}: generated.\n", files.len() + 1);
        if before_all {
            // A warm-up that hands the tests a file, as a real suite's
            // sign-in would.
            text.push_str(
                "\n@before-all\n  goto /\n  expect 'role=heading[name=\"Products\"]' visible\n  state save shop.json\n\n@before\n  state load shop.json\n",
            );
        }
        for (n, &kind) in kinds[next..end].iter().enumerate() {
            let number = next + n + 1;
            text.push('\n');
            text.push_str(&test(kind, number, &mut rng));
        }
        files.push(text);
        next = end;
    }
    files
}

/// Test `number` of the suite.
fn test(kind: Kind, number: usize, rng: &mut Rng) -> String {
    let (id, name, price) = PRODUCTS[rng.below(PRODUCTS.len())];
    let mut t = String::new();
    let variant = rng.below(5);
    let title = match kind {
        Kind::Short => "short",
        Kind::Medium => "medium",
        Kind::Long => "long",
    };
    let _ = writeln!(t, "@test \"{title} {number}\"");
    let steps: Vec<String> = match (kind, variant) {
        (Kind::Short, 0) => vec![
            "goto /".into(),
            "expect 'role=heading[name=\"Products\"]' visible".into(),
            "expect 'role=list[name=\"Products\"] >> role=listitem' count ${var:products}".into(),
        ],
        (Kind::Short, 1) => vec![
            format!("goto /products/{id}"),
            format!("expect role=heading text '{name}'"),
            format!("expect testid=price text ${price}.00"),
        ],
        (Kind::Short, 2) => vec![
            "goto /cart".into(),
            "expect 'text=Your cart is empty.' visible".into(),
            "expect 'role=link[name=\"Cart (0)\"]' visible".into(),
        ],
        (Kind::Short, 3) => vec![
            "goto /search?q=zzz".into(),
            "expect 'text=No products match “zzz”.' visible".into(),
            "expect 'role=link[name=\"Sign in\"]' visible".into(),
        ],
        (Kind::Short, _) => vec![
            "goto /login".into(),
            "expect 'role=heading[name=\"Sign in\"]' visible".into(),
            "expect label=Email visible".into(),
        ],
        (Kind::Medium, 0 | 1) => vec![
            sign_in(number),
            format!("goto /products/{id}"),
            "click 'role=button[name=\"Add to cart\"]'".into(),
            "expect role=status text 'Added to cart'".into(),
            "expect 'role=link[name=\"Cart (1)\"]' visible".into(),
        ],
        (Kind::Medium, 2) => vec![
            sign_in(number),
            "click 'role=button[name=\"Sign out\"]'".into(),
            "expect 'role=link[name=\"Sign in\"]' visible".into(),
            "expect request POST /api/logout".into(),
        ],
        (Kind::Medium, 3) => vec![
            sign_in(number),
            "reload".into(),
            format!("expect 'text=Signed in as user{number}@example.com' visible"),
        ],
        (Kind::Medium, _) => vec![
            sign_in(number),
            "fill 'role=searchbox[name=\"Search\"]' ${var:search}".into(),
            "press Enter".into(),
            "expect 'role=list[name=\"Results\"] >> role=listitem' count 1".into(),
            "expect text=${var:keyboard} visible".into(),
        ],
        (Kind::Long, _) => {
            let mut steps = vec![sign_in(number)];
            let (other, _, other_price) = PRODUCTS[(id as usize) % PRODUCTS.len()];
            let mut total = price;
            let mut items = 1;
            steps.extend(add_to_cart(id));
            // Every other checkout has two products.
            if variant.is_multiple_of(2) {
                steps.extend(add_to_cart(other));
                total += other_price;
                items += 1;
            }
            let noun = if items == 1 { "item" } else { "items" };
            steps.extend([
                "goto /checkout".into(),
                format!("expect 'text={items} {noun}, total ${total}.00' visible"),
                "click 'role=button[name=\"Place order\"]'".into(),
                "expect 'role=heading[name=\"Thank you!\"]' visible".into(),
                "expect --url **/orders/*".into(),
                "expect 'role=link[name=\"Cart (0)\"]' visible".into(),
            ]);
            steps
        }
    };
    for step in steps {
        let _ = writeln!(t, "  {step}");
    }
    t
}

fn sign_in(number: usize) -> String {
    format!("@include ../steps/sign-in.cdp email=user{number}@example.com")
}

fn add_to_cart(id: u32) -> [String; 3] {
    [
        format!("goto /products/{id}"),
        "click 'role=button[name=\"Add to cart\"]'".into(),
        "expect role=status text 'Added to cart'".into(),
    ]
}

/// xorshift64: the same sequence for the same seed, everywhere.
struct Rng(u64);

impl Rng {
    /// A number in `0..n`.
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        usize::try_from(self.0 % n as u64).unwrap_or(0)
    }
}

fn copy(from: &Path, to: &Path) -> Result {
    if from.is_dir() {
        fs::create_dir_all(to).map_err(|e| format!("create {}: {e}", to.display()))?;
        let entries = fs::read_dir(from).map_err(|e| format!("read {}: {e}", from.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("read {}: {e}", from.display()))?;
            copy(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        fs::copy(from, to)
            .map(drop)
            .map_err(|e| format!("copy {} to {}: {e}", from.display(), to.display()))
    }
}

fn write(path: &Path, text: &str) -> Result {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(files: &[String]) -> (usize, usize, usize) {
        let count = |kind: &str| {
            files
                .iter()
                .map(|f| f.matches(&format!("@test \"{kind} ")).count())
                .sum()
        };
        (count("short"), count("medium"), count("long"))
    }

    #[test]
    fn the_mix_is_sixty_thirty_ten() {
        let files = scenarios_for(1_000);
        assert_eq!(kinds(&files), (600, 300, 100));
        let sizes: Vec<usize> = files.iter().map(|f| f.matches("@test ").count()).collect();
        // Every file but the last is within the range.
        assert!(
            sizes[..sizes.len() - 1]
                .iter()
                .all(|&n| (FILE_TESTS.0..=FILE_TESTS.1).contains(&n)),
            "{sizes:?}"
        );
        let with_hooks = files.iter().filter(|f| f.contains("@before-all")).count();
        assert!(with_hooks > 0 && with_hooks < files.len(), "{with_hooks}");
        assert_eq!(kinds(&scenarios_for(7)), (5, 2, 0));
    }

    #[test]
    fn the_same_n_writes_the_same_suite() {
        assert_eq!(scenarios_for(250), scenarios_for(250));
        assert_ne!(scenarios_for(250), scenarios_for(251));
        let tmp = std::env::temp_dir().join(format!("xtask-scale-{}", std::process::id()));
        let root = super::super::workspace_root();
        for path in COPIED {
            copy(&root.join(SHOP).join(path), &tmp.join(SHOP).join(path)).unwrap();
        }
        let files = generate(&tmp, 30).unwrap();
        let dir = tmp.join(DIR);
        assert!(dir.join("dist/index.html").is_file());
        assert!(dir.join("e2e/mocks/base.toml").is_file());
        assert!(dir.join("e2e/steps/sign-in.cdp").is_file());
        let written = fs::read_dir(dir.join("e2e/scenarios")).unwrap().count();
        assert_eq!(written, files);
        fs::remove_dir_all(&tmp).unwrap();
    }
}

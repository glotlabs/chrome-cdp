//! Locators: how commands name elements (plan 04, milestone 4.4).
//!
//! [`parse`] holds the grammar, one for every command: refs, CSS (piercing
//! open shadow roots), XPath, `text=`, `role=`, `label=`, `placeholder=`,
//! `alt=`, `title=`, `testid=`, `>>` chains with `nth=`, `visible=`,
//! `has-text=` and `has-not-text=` filters, `has=`, `has-not=`, `and=` and
//! `or=` with inner locators, and `frame=` scoping. [`resolve`] finds what a
//! locator matches: the engine resolves the leading ref or `frame=` steps
//! to a frame (and an element to start from), and the injected bundle
//! (`injected/src/locator.ts`) evaluates the rest in that frame's isolated
//! world.
//!
//! Actions want exactly one element ([`Engine::locate_one`](crate::Engine)
//! fails with `Ambiguous`, listing candidates with new refs); read-only
//! commands take every match.

mod parse;
mod resolve;
mod stable;

pub use parse::{DEFAULT_TESTID_ATTRIBUTE, Locator, parse_locator};
pub use resolve::LocateOptions;
pub(crate) use resolve::{Called, FrameOnly, Place, Want, bad_locator, read_listed, release};
pub use stable::is_ref;

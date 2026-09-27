//! Input and actionability (plan 05).
//!
//! Milestone 5.1 is the actionability pipeline ([`actionability`]): before
//! an action touches an element, it waits until the element is there,
//! visible, stable, enabled (and editable) and not covered, scrolls it into
//! view, and finds the point to act at in the tab's viewport. Later
//! milestones add the actions on top: mouse and pointer (5.2), keyboard and
//! text (5.3), files (5.4) and the post-action report (5.5).

mod actionability;

pub use actionability::{ActionPoint, Checks};

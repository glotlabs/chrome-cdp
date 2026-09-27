//! The tree both snapshot engines produce, and what the pipeline knows
//! about roles.
//!
//! An engine returns the page as [`Node`]s: one per element or text that
//! is not hidden, with the ARIA role and accessible name already worked
//! out, iframes holding their document's nodes. Everything after that
//! (pruning, refs, rendering, diffs) is shared, so the two engines differ
//! only in what they read from the page.

use std::fmt;

/// Identifies a DOM node across snapshots: the frame whose document holds
/// it and its `backendNodeId` (unique within a renderer process, so the
/// frame is part of the key).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct NodeKey {
    pub(crate) frame: String,
    pub(crate) backend: i64,
}

/// `aria-checked` and friends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Checked {
    True,
    False,
    Mixed,
}

/// The states a snapshot line can show (plan 04, 4.1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct States {
    pub(crate) checked: Option<Checked>,
    pub(crate) expanded: Option<bool>,
    pub(crate) selected: bool,
    pub(crate) disabled: bool,
    pub(crate) readonly: bool,
    pub(crate) required: bool,
    pub(crate) invalid: bool,
    pub(crate) focused: bool,
    pub(crate) pressed: bool,
}

impl States {
    /// The words for the line, in the documented order.
    pub(crate) fn words(&self) -> Vec<&'static str> {
        let mut words = Vec::new();
        match self.checked {
            Some(Checked::True) => words.push("checked"),
            Some(Checked::False) => words.push("unchecked"),
            Some(Checked::Mixed) => words.push("mixed"),
            None => {}
        }
        match self.expanded {
            Some(true) => words.push("expanded"),
            Some(false) => words.push("collapsed"),
            None => {}
        }
        let flags = [
            (self.selected, "selected"),
            (self.disabled, "disabled"),
            (self.readonly, "readonly"),
            (self.required, "required"),
            (self.invalid, "invalid"),
            (self.focused, "focused"),
            (self.pressed, "pressed"),
        ];
        words.extend(flags.iter().filter(|(on, _)| *on).map(|(_, w)| *w));
        words
    }
}

/// One node of a page, as an engine read it or as the pipeline shaped
/// it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Node {
    /// The ARIA role, lower case; [`TEXT`] for text, [`NONE`] for a node
    /// with no role of its own (its children are hoisted).
    pub(crate) role: String,
    /// The accessible name, or the text of a text node.
    pub(crate) name: String,
    pub(crate) value: Option<String>,
    /// A heading's level.
    pub(crate) level: Option<u32>,
    /// A link's target, relative when on the frame's origin.
    pub(crate) href: Option<String>,
    pub(crate) password: bool,
    /// A file input: `upload` sets it, a click opens a file chooser.
    pub(crate) file: bool,
    pub(crate) states: States,
    /// Acts on clicks without being a control: a `div` with `onclick`,
    /// `tabindex` or `cursor: pointer` (the heuristic of 4.3).
    pub(crate) clickable: bool,
    /// Its content scrolls (`overflow: auto|scroll` and larger than its
    /// box).
    pub(crate) scrollable: bool,
    /// Laid out inline: its text runs on with its neighbours'. Block
    /// boundaries keep text apart when a node is hoisted.
    pub(crate) inline: bool,
    pub(crate) key: Option<NodeKey>,
    pub(crate) children: Vec<Node>,
    // Set by the pipeline.
    /// Gets a ref.
    pub(crate) reffable: bool,
    /// Its ref number (`12` for `e12`).
    pub(crate) reference: Option<u32>,
    /// A placeholder for `n` more nodes of `role` left out of a long list.
    pub(crate) more: Option<u32>,
    /// How many nodes below it `--depth` left out.
    pub(crate) below: Option<u32>,
}

/// The role of a text node.
pub(crate) const TEXT: &str = "text";

/// The role of a node that only groups its children.
pub(crate) const NONE: &str = "none";

/// The role a non-semantic element that acts on clicks is shown with.
pub(crate) const CLICKABLE: &str = "clickable";

/// The role of an iframe (its document's nodes are its children).
pub(crate) const IFRAME: &str = "iframe";

impl Node {
    /// A text node.
    pub(crate) fn text(text: impl Into<String>, key: Option<NodeKey>) -> Self {
        Self {
            role: TEXT.to_owned(),
            name: text.into(),
            inline: true,
            key,
            ..Self::default()
        }
    }

    pub(crate) fn is_text(&self) -> bool {
        self.role == TEXT
    }

    /// How many nodes are below it.
    pub(crate) fn descendants(&self) -> usize {
        self.children.iter().map(|c| 1 + c.descendants()).sum()
    }

    /// Visits it and every node below it, parents first.
    pub(crate) fn walk<'a>(&'a self, visit: &mut impl FnMut(&'a Node)) {
        visit(self);
        for child in &self.children {
            child.walk(visit);
        }
    }
}

impl fmt::Display for NodeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.frame, self.backend)
    }
}

/// Roles an agent acts on: they get refs.
pub(crate) fn is_interactive(role: &str) -> bool {
    matches!(
        role,
        "button"
            | "link"
            | "textbox"
            | "searchbox"
            | "checkbox"
            | "radio"
            | "switch"
            | "combobox"
            | "listbox"
            | "option"
            | "menuitem"
            | "menuitemcheckbox"
            | "menuitemradio"
            | "tab"
            | "slider"
            | "spinbutton"
            | "colorwell"
            | "treeitem"
            | CLICKABLE
            | IFRAME
    )
}

/// Roles `--interactive` keeps as context around the refs.
pub(crate) fn is_landmark(role: &str) -> bool {
    matches!(
        role,
        "banner"
            | "navigation"
            | "main"
            | "contentinfo"
            | "complementary"
            | "search"
            | "form"
            | "region"
            | "dialog"
            | "alertdialog"
            | IFRAME
    )
}

/// Roles that only group or format content: without a name they are
/// dropped and their children hoisted.
pub(crate) fn is_structural(role: &str) -> bool {
    matches!(
        role,
        NONE | "generic"
            | "presentation"
            | "paragraph"
            | "group"
            | "label"
            | "legend"
            | "section"
            | "strong"
            | "emphasis"
            | "code"
            | "mark"
            | "insertion"
            | "deletion"
            | "subscript"
            | "superscript"
            | "time"
            | "abbr"
            | "ruby"
            | "pre"
            | "figcaption"
            | "caption"
            | "sectionheader"
            | "sectionfooter"
            | "rowgroup"
    )
}

/// Roles whose name is their content (ARIA "name from content"): when
/// named, children that hold nothing to act on only repeat the name.
pub(crate) fn name_from_content(role: &str) -> bool {
    matches!(
        role,
        "button"
            | "link"
            | "heading"
            | "tab"
            | "option"
            | "menuitem"
            | "menuitemcheckbox"
            | "menuitemradio"
            | "treeitem"
            | "checkbox"
            | "radio"
            | "switch"
            | "cell"
            | "gridcell"
            | "columnheader"
            | "rowheader"
            | "tooltip"
            | "img"
            | CLICKABLE
    )
}

/// Roles whose children are their editable content: the value says it.
pub(crate) fn is_text_field(role: &str) -> bool {
    matches!(role, "textbox" | "searchbox" | "spinbutton" | "slider")
}

/// Roles whose value a line shows.
pub(crate) fn shows_value(role: &str) -> bool {
    matches!(
        role,
        "textbox" | "searchbox" | "combobox" | "slider" | "spinbutton" | "colorwell"
    )
}

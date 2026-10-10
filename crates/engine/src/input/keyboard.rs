//! The keyboard (plan 05, milestone 5.3): `press` and `type`; `keyboard
//! down|up` (plan 14, milestone 14.3) hold keys across commands.
//!
//! Keys go where focus is, as a user's do: `--in <loc>` focuses an element
//! first (after the pipeline, without the hit test: no mouse is used).
//! `press` sends each chord's key downs and ups ([`keys::chord_events`]);
//! `type` presses the layout's key for each character it has one for and
//! inserts the rest as text (`Input.insertText`, what an IME does), so
//! emoji and CJK arrive whole.
//!
//! Keys `keyboard down` holds are kept per tab (across navigations, gone
//! with the tab), as the mouse's buttons are. Held modifiers go with every
//! later key, mouse, touch and drag event of the tab;
//! `fill` sets text without them.

use std::fmt::Write as _;
use std::time::Duration;

use cdp::domains::{browser as cdp_browser, dom, input, runtime};
use protocol::schema::{ActedElement, KeyboardData, KeysData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};
use tokio::time::Instant;

use super::keys::{self, Key, KeyEvent, Mod};
use super::report::{FOCUS_DEPTH, FOCUS_FRAME_WAIT};
use super::{Actor, Checks, Ready, describe};
use crate::context::Scoped as _;
use crate::errors::from_cdp;
use crate::frames::World;
use crate::injected::{self, GUARD};
use crate::request::{Action, KeyboardOp, Modifier};
use crate::snapshot::{NodeKey, Target};

/// Checks a `keyboard` command's key: one known key, not a chord.
pub(super) fn check(op: &KeyboardOp) -> Result<(), ErrorInfo> {
    let (KeyboardOp::Down { key: name } | KeyboardOp::Up { key: Some(name) }) = op else {
        return Ok(());
    };
    let command = format!("keyboard {}", op.command());
    let chords = match keys::parse(name) {
        Ok(chords) => chords,
        Err(message) => {
            let info = ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("{message}; name keys like Shift, Control, Mod, Enter, a or A"),
            );
            return Err(match keys::suggest(name) {
                Some(fixed) => info
                    .note("did you mean", &fixed)
                    .hint(Hint::run(Cmd::new(&command).arg(fixed))),
                None => info.hint(Hint::run(Cmd::new(&command).arg("Shift"))),
            });
        }
    };
    if let [chord] = chords.as_slice()
        && let [_] = chord.as_slice()
    {
        return Ok(());
    }
    let first = chords
        .first()
        .and_then(|chord| chord.first())
        .cloned()
        .unwrap_or_default();
    Err(ErrorInfo::new(
        ErrorKind::BadArgs,
        format!("{command} takes one key, not `{name}`"),
    )
    .hint(
        Hint::run(Cmd::new(&command).arg(first))
            .why("one key per command")
            .or(format!("{}   (a whole chord)", Cmd::new("press").arg(name))),
    ))
}

impl Actor<'_> {
    /// Whether the browser runs on macOS (asked once per browser): `Mod`
    /// is Meta there, and key downs carry the editing commands macOS runs
    /// for them. The browser's user agent says, not the host's platform
    /// (a `connect`ed browser may run elsewhere).
    pub(super) async fn mac(&self) -> Result<bool, ErrorInfo> {
        if let Some(mac) = self.running.mac.get() {
            return Ok(*mac);
        }
        let version = self
            .running
            .conn
            .browser()
            .with_timeout(self.timeout)
            .call(cdp_browser::GetVersionParams::default())
            .await
            .map_err(|e| from_cdp(&e, self.session()))?;
        let mac = version.user_agent.contains("Macintosh");
        let _ = self.running.mac.set(mac);
        Ok(mac)
    }

    /// `modifiers` with `Mod` resolved by the browser's platform.
    pub(super) async fn resolve_modifiers(
        &self,
        modifiers: &[Modifier],
    ) -> Result<Vec<Modifier>, ErrorInfo> {
        if !modifiers.contains(&Modifier::Mod) {
            return Ok(modifiers.to_vec());
        }
        let native = if self.mac().await? {
            Modifier::Meta
        } else {
            Modifier::Control
        };
        let mut out = Vec::new();
        for m in modifiers {
            let m = if *m == Modifier::Mod { native } else { *m };
            if !out.contains(&m) {
                out.push(m);
            }
        }
        Ok(out)
    }

    /// Sends one key event to the tab.
    pub(super) async fn key_event(&self, event: &KeyEvent) -> Result<(), ErrorInfo> {
        let key = &event.key;
        let kind = if !event.down {
            input::DispatchKeyEventType::KeyUp
        } else if key.text.is_empty() {
            input::DispatchKeyEventType::RawKeyDown
        } else {
            input::DispatchKeyEventType::KeyDown
        };
        let mut params = input::DispatchKeyEventParams::new(kind);
        params.modifiers = Some(event.modifiers);
        params.key = Some(key.key.to_owned());
        params.code = Some(key.code.to_owned());
        params.windows_virtual_key_code = Some(i64::from(key.key_code));
        params.location = Some(i64::from(key.location));
        if event.down {
            if !key.text.is_empty() {
                params.text = Some(key.text.to_owned());
                params.unmodified_text = Some(key.text.to_owned());
            }
            params.is_keypad = Some(key.is_keypad());
            params.auto_repeat = Some(false);
            if !event.commands.is_empty() {
                params.commands = Some(event.commands.iter().map(|&c| c.to_owned()).collect());
            }
        }
        self.page
            .session
            .with_timeout(self.timeout)
            .call(params)
            .await
            .map_err(|e| from_cdp(&e, self.session()))?;
        Ok(())
    }

    /// The modifiers `keyboard down` holds in the tab.
    pub(super) fn held_modifiers(&self) -> Vec<Mod> {
        self.running
            .tabs
            .keys(self.page.id, |keys| {
                keys.iter().filter_map(Key::modifier).collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }

    /// The `modifiers` bits of what `keyboard down` holds in the tab, for
    /// mouse and touch events.
    pub(super) fn held_bits(&self) -> i64 {
        keys::bits(&self.held_modifiers())
    }

    /// Presses and releases `key` alone (with the modifiers `keyboard
    /// down` holds; another than Shift makes it type nothing).
    async fn tap_key(&self, mut key: Key) -> Result<(), ErrorInfo> {
        let held = self.held_modifiers();
        if held.iter().any(|m| *m != Mod::Shift) {
            key.text = "";
        }
        for down in [true, false] {
            let event = KeyEvent {
                down,
                key,
                modifiers: keys::bits(&held),
                commands: Vec::new(),
            };
            self.key_event(&event).await?;
        }
        Ok(())
    }

    /// Inserts `text` where focus is, as an IME commits it.
    pub(super) async fn insert_text(&self, text: &str) -> Result<(), ErrorInfo> {
        self.page
            .session
            .with_timeout(self.timeout)
            .call(input::InsertTextParams::new(text.to_owned()))
            .await
            .map_err(|e| from_cdp(&e, self.session()))?;
        Ok(())
    }

    /// Presses the chords of `keys` (as `press` takes them).
    pub(super) async fn press_chords(&self, keys: &str) -> Result<Vec<String>, ErrorInfo> {
        let chords = keys::parse(keys).map_err(|m| ErrorInfo::new(ErrorKind::BadArgs, m))?;
        let mac = self.mac().await?;
        let held = self.held_modifiers();
        let mut pressed = Vec::new();
        for chord in &chords {
            let events = keys::chord_events(chord, mac, &held)
                .map_err(|m| ErrorInfo::new(ErrorKind::BadArgs, m))?;
            for event in &events {
                self.key_event(event).await?;
            }
            let names: Vec<&str> = chord
                .iter()
                .map(|name| match name.as_str() {
                    name if keys::is_mod(name) && mac => "Meta",
                    name if keys::is_mod(name) => "Control",
                    name => name,
                })
                .collect();
            pressed.push(names.join("+"));
        }
        Ok(pressed)
    }

    /// Focuses `target` if it does not have focus yet; returns whether it
    /// had to. Fails when it cannot take focus.
    pub(super) async fn focus_on(
        &self,
        target: &Target,
        element: &ActedElement,
    ) -> Result<bool, ErrorInfo> {
        if self.has_focus(target).await? {
            return Ok(false);
        }
        let focused = target
            .context
            .session
            .with_timeout(self.timeout)
            .call(dom::FocusParams {
                object_id: Some(target.object_id.clone()),
                ..dom::FocusParams::default()
            })
            .await;
        if focused.is_err() || !self.has_focus(target).await? {
            return Err(self.cannot_focus(element));
        }
        Ok(true)
    }

    /// Whether `target` (or something in it) has focus.
    async fn has_focus(&self, target: &Target) -> Result<bool, ErrorInfo> {
        self.call(
            target,
            "function () { return globalThis.__ccdp.hasFocus(this); }",
            Vec::new(),
        )
        .await
        .map(|v| v.as_bool() == Some(true))
    }

    /// `element` does not take focus (a `<div>` without `tabindex`).
    fn cannot_focus(&self, element: &ActedElement) -> ErrorInfo {
        let described = describe(element);
        let hint = match &element.reference {
            Some(reference) => Hint::run(Cmd::new("click").arg(reference).scoped(self.session()))
                .why("its page may take keys after a click"),
            None => Hint::run(Cmd::new("snapshot").raw("-i").scoped(self.session()))
                .why("lists what can take focus"),
        };
        ErrorInfo::new(
            ErrorKind::NotEnabled,
            format!("{described} cannot take focus, so keys would not go to it"),
        )
        .hint(hint)
        .details(serde_json::json!({ "element": element, "check": "focus" }))
    }

    /// Where keys go: `locator`'s element once it passes `checks` and has
    /// focus (with whether it had to be focused), or what has focus now.
    async fn keys_target(
        &self,
        locator: Option<&str>,
        checks: Checks,
    ) -> Result<(Option<ActedElement>, Option<Ready>, bool), ErrorInfo> {
        let Some(locator) = locator else {
            return Ok((self.focused_element().await?, None, false));
        };
        let parsed = self.parse(locator)?;
        let retry = |l: &str| self.retry(l);
        let ready = self.ready(&parsed, checks, &retry).await?;
        match self.focus_on(&ready.ready.target, &ready.element).await {
            Ok(moved) => Ok((Some(ready.element.clone()), Some(ready), moved)),
            Err(err) => {
                ready.release();
                Err(err)
            }
        }
    }

    /// The element that has focus in the tab (inside shadow roots, and
    /// down into iframes, out-of-process ones too; the `<iframe>` when its
    /// document has nothing focused or cannot be asked), named; `None`
    /// when nothing has.
    async fn focused_element(&self) -> Result<Option<ActedElement>, ErrorInfo> {
        const FOCUSED: &str = "globalThis.__ccdp.focused()";
        let Some(mut target) = self.frame_element(None, FOCUSED, self.timeout).await? else {
            return Ok(None);
        };
        for _ in 0..FOCUS_DEPTH {
            let inner = self
                .engine
                .content_frame(&target.context, &target.object_id, self.timeout)
                .await
                .ok()
                .flatten();
            let Some(inner) = inner else { break };
            match self
                .frame_element(Some(&inner), FOCUSED, FOCUS_FRAME_WAIT)
                .await
            {
                Ok(Some(next)) => {
                    target.release();
                    target = next;
                }
                _ => break,
            }
        }
        let named = self.name(&target).await;
        target.release();
        named.map(Some)
    }

    /// The element `expression` (which may use `__ccdp`) gives in the
    /// isolated world of frame `frame` (the main frame if `None`; its
    /// context waited for at most `wait`), as a handle; `None` for
    /// `null`.
    pub(super) async fn frame_element(
        &self,
        frame: Option<&str>,
        expression: &str,
        wait: Duration,
    ) -> Result<Option<Target>, ErrorInfo> {
        let (frame, context) = self
            .engine
            .frame_and_context(self.running, self.page, frame, World::Isolated, wait)
            .await?;
        let session = context.session.with_timeout(self.timeout);
        let mut params = runtime::CallFunctionOnParams::new(format!(
            "function () {{ {GUARD} return {expression}; }}"
        ));
        params.execution_context_id = Some(context.context_id);
        let result = injected::call(&session, context.context_id, params)
            .await
            .map_err(|e| from_cdp(&e, self.session()))?;
        let Some(object_id) = result.result.object_id else {
            return Ok(None);
        };
        let node = session
            .call(dom::DescribeNodeParams {
                object_id: Some(object_id.clone()),
                ..dom::DescribeNodeParams::default()
            })
            .await;
        Ok(Some(Target {
            key: NodeKey {
                frame,
                backend: match node {
                    Ok(node) => node.node.backend_node_id,
                    Err(e) => {
                        crate::locator::release(&context, object_id);
                        return Err(from_cdp(&e, self.session()));
                    }
                },
            },
            context,
            object_id,
            note: None,
        }))
    }

    /// `press <keys> [--in loc]`.
    pub(super) async fn press_keys(&self) -> Result<Output, ErrorInfo> {
        let Action::Press { keys, locator } = &self.req.action else {
            unreachable!("not press")
        };
        let checks = Checks {
            enabled: true,
            no_hit_test: true,
            ..Checks::default()
        };
        let (element, ready, _) = self.keys_target(locator.as_deref(), checks).await?;
        let note = ready.as_ref().and_then(|r| r.ready.target.note.clone());
        if let Some(ready) = ready {
            ready.release();
        }
        let pressed = self.press_chords(keys).await?;
        let mut text = format!("pressed {}", pressed.join(" "));
        if let Some(element) = &element {
            let _ = write!(text, " in {}", describe(element));
        }
        let quiet = pressed.join(" ");
        let data = KeysData {
            tab: self.tab(),
            action: "press".to_owned(),
            keys: pressed,
            chars: None,
            element,
        };
        let mut output = Output::typed(&data, text).quiet(quiet);
        if let Some(note) = note {
            output = output.warning(note);
        }
        Ok(output)
    }

    /// `type <text> [--in loc] [--delay ms]`.
    pub(super) async fn type_text(&self) -> Result<Output, ErrorInfo> {
        let Action::Type {
            text,
            locator,
            delay_ms,
            force,
        } = &self.req.action
        else {
            unreachable!("not type")
        };
        let count = text.chars().count();
        let delay = Duration::from_millis(*delay_ms);
        let needs = delay.saturating_mul(u32::try_from(count).unwrap_or(u32::MAX));
        if Instant::now() + needs >= self.deadline {
            return Err(self.too_slow(count, needs));
        }
        let checks = Checks {
            enabled: true,
            editable: true,
            force: *force,
            no_hit_test: true,
            ..Checks::default()
        };
        let (element, ready, moved) = self.keys_target(locator.as_deref(), checks).await?;
        let note = ready.as_ref().and_then(|r| r.ready.target.note.clone());
        if let Some(ready) = ready {
            // Typing into a field it just focused goes after its text, as
            // after a click there.
            if moved {
                let _ = self
                    .call(
                        &ready.ready.target,
                        "function () { globalThis.__ccdp.caretToEnd(this); }",
                        Vec::new(),
                    )
                    .await;
            }
            ready.release();
        }
        let mut pending = String::new();
        let mut first = true;
        for c in text.chars() {
            let key = keys::typed(c);
            if key.is_some() || !delay.is_zero() {
                if !pending.is_empty() {
                    self.insert_text(&std::mem::take(&mut pending)).await?;
                }
                if !first && !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
            }
            first = false;
            match key {
                Some(key) => self.tap_key(key).await?,
                None if delay.is_zero() => pending.push(c),
                None => self.insert_text(c.encode_utf8(&mut [0; 4])).await?,
            }
        }
        if !pending.is_empty() {
            self.insert_text(&pending).await?;
        }
        let what = if count == 1 {
            "character"
        } else {
            "characters"
        };
        let mut out = format!("typed {count} {what}");
        if let Some(element) = &element {
            let _ = write!(out, " into {}", describe(element));
        }
        let quiet = element
            .as_ref()
            .and_then(|e| e.reference.clone())
            .unwrap_or_else(|| "typed".to_owned());
        let data = KeysData {
            tab: self.tab(),
            action: "type".to_owned(),
            keys: Vec::new(),
            chars: Some(count),
            element,
        };
        let mut output = Output::typed(&data, out).quiet(quiet);
        if let Some(note) = note {
            output = output.warning(note);
        }
        Ok(output)
    }

    /// `type --delay` would outlast the action's timeout.
    fn too_slow(&self, count: usize, needs: Duration) -> ErrorInfo {
        let mut retry = match &self.req.action {
            Action::Type {
                locator: Some(l), ..
            } => self.retry(l),
            Action::Type { text, delay_ms, .. } => Cmd::new("type")
                .arg(text)
                .flag(
                    "delay",
                    protocol::format::duration(Duration::from_millis(*delay_ms)),
                )
                .scoped(self.session()),
            _ => Cmd::new("type"),
        };
        let wanted = needs + Duration::from_secs(5);
        retry = retry.flag("timeout", format!("{}s", wanted.as_secs()));
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!(
                "typing {count} characters with --delay takes {}, longer than the timeout ({})",
                protocol::format::duration(needs),
                protocol::format::duration(self.timeout)
            ),
        )
        .hint(Hint::run(retry))
    }

    /// `keyboard down <key>` and `keyboard up <key>|--all`.
    pub(super) async fn keyboard(&self, op: &KeyboardOp) -> Result<Output, ErrorInfo> {
        let mac = self.mac().await?;
        let held: Vec<Key> = self
            .running
            .tabs
            .keys(self.page.id, |keys| keys.clone())
            .unwrap_or_default();
        let modifiers = |keys: &[Key]| keys.iter().filter_map(Key::modifier).collect::<Vec<_>>();
        let bad = |message: String| ErrorInfo::new(ErrorKind::BadArgs, message);
        let mut warnings = Vec::new();
        let mut released = Vec::new();
        let mut now = held.clone();
        match op {
            KeyboardOp::Down { key: name } => {
                let key = keys::resolve(name, mac, &modifiers(&held)).map_err(bad)?;
                if held.iter().any(|h| h.code == key.code) {
                    warnings.push(format!("{} is held already", key.key));
                } else {
                    let event = keys::down_event(key, mac, &modifiers(&held));
                    self.key_event(&event).await?;
                    now.push(key);
                    self.set_keys(&now);
                }
            }
            KeyboardOp::Up { key: Some(name) } => {
                let code = keys::resolve(name, mac, &[]).map_err(bad)?.code;
                match held.iter().position(|h| h.code == code) {
                    None => warnings.push(format!("{name} is not held")),
                    Some(at) => {
                        let key = now.remove(at);
                        self.release_key(key, &now).await?;
                        released.push(key.key.to_owned());
                    }
                }
            }
            KeyboardOp::Up { key: None } => {
                if held.is_empty() {
                    warnings.push("no key is held".to_owned());
                }
                while let Some(key) = now.pop() {
                    self.release_key(key, &now).await?;
                    released.push(key.key.to_owned());
                }
            }
        }
        let names: Vec<String> = now.iter().map(|k| k.key.to_owned()).collect();
        let holding = if names.is_empty() {
            "holding nothing".to_owned()
        } else {
            format!("holding {}", names.join(", "))
        };
        let text = if released.is_empty() {
            holding
        } else {
            format!("released {} ({holding})", released.join(", "))
        };
        let data = KeyboardData {
            tab: self.tab(),
            action: op.command().to_owned(),
            key: match op {
                KeyboardOp::Down { key } | KeyboardOp::Up { key: Some(key) } => Some(key.clone()),
                KeyboardOp::Up { key: None } => None,
            },
            released,
            held: names.clone(),
        };
        Ok(warnings.into_iter().fold(
            Output::typed(&data, text).quiet(names.join(",")),
            Output::warning,
        ))
    }

    /// Sends `key`'s up with `rest` still held, and keeps `rest`.
    async fn release_key(&self, key: Key, rest: &[Key]) -> Result<(), ErrorInfo> {
        let modifiers: Vec<Mod> = rest.iter().filter_map(Key::modifier).collect();
        let event = KeyEvent {
            down: false,
            key,
            modifiers: keys::bits(&modifiers),
            commands: Vec::new(),
        };
        self.key_event(&event).await?;
        self.set_keys(rest);
        Ok(())
    }

    fn set_keys(&self, keys: &[Key]) {
        self.running
            .tabs
            .keys(self.page.id, |held| *held = keys.to_vec());
    }
}

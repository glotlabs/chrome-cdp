//! Forms (plan 16, milestone 16.4): `forms` and `fill-form`.
//!
//! `forms` lists the page's forms as units (every `<form>`, `role=form`
//! and `role=search`, in every frame, and the fields outside any), each
//! field with its name, kind, state, value and validation message; the
//! page code is `injected/src/forms.ts`.
//!
//! `fill-form` fills one form from a JSON object: each key is matched to
//! a field (by accessible name, then `name`, `id`, placeholder, then a
//! unique part of the name), and the value picks the action (`fill`,
//! `select`, `check`, `upload`, a click on an option). Fields are filled
//! in the page's order, each through the single command's code
//! ([`Actor::run`]); after each the form is listed again, so a field that
//! appears after an earlier choice is matched too. One action report
//! covers the whole form.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use cdp::domains::runtime;
use protocol::schema::{
    Acted, FillFormData, FilledField, FormField, FormRadio, FormUnit, FormsData, UnmatchedKey,
    WhyElement,
};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::Instant;

use super::{Actor, Candidate, element_text, locate_options, refer_all};
use crate::context::Scoped as _;
use crate::errors::from_cdp;
use crate::frames::World;
use crate::injected::{self, GUARD};
use crate::locator::{LocateOptions, read_listed};
use crate::request::{Action, ActionRequest, Button, FillFormRequest, FormsRequest, millis};
use crate::snapshot::{NodeKey, Target};
use crate::tabs::FrameContext;
use crate::tabs::{Page, tab_name};
use crate::{Engine, Running};

/// The object group of the listing calls.
const GROUP: &str = "chrome-cdp-forms";

/// The most elements one listing names.
const NAMED: usize = 4000;

/// How long an iframe's context is waited for.
const FRAME_WAIT: Duration = Duration::from_secs(2);

/// How long `fill-form` waits for a field to appear after it filled one,
/// while keys are left (a region select after a country).
const APPEAR_WAIT: Duration = Duration::from_secs(2);

/// How long it waits for a combobox's listbox to show the option.
const OPTION_WAIT: Duration = Duration::from_millis(1500);

/// How often it looks again.
const POLL: Duration = Duration::from_millis(100);

/// What `formsList` answers.
#[derive(Debug, Default, Deserialize)]
struct Listed {
    #[serde(default)]
    units: Vec<RawUnit>,
    #[serde(default)]
    names: Vec<Candidate>,
}

#[derive(Debug, Default, Deserialize)]
struct RawUnit {
    el: Option<usize>,
    #[serde(default)]
    outside: bool,
    #[serde(default)]
    scope: bool,
    method: Option<String>,
    action: Option<String>,
    #[serde(default)]
    fields: Vec<RawField>,
}

#[derive(Debug, Default, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
struct RawField {
    el: usize,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    custom: bool,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    disabled: bool,
    #[serde(default)]
    readonly: bool,
    value: Option<String>,
    #[serde(default)]
    secret: bool,
    #[serde(default)]
    empty: bool,
    checked: Option<Value>,
    #[serde(default)]
    files: Vec<String>,
    accept: Option<String>,
    #[serde(default)]
    multiple: bool,
    #[serde(default)]
    options: Vec<String>,
    #[serde(default)]
    more: u64,
    #[serde(default)]
    selected: Vec<String>,
    #[serde(default)]
    invalid: bool,
    message: Option<String>,
    html: Option<String>,
    id: Option<String>,
    placeholder: Option<String>,
    #[serde(default)]
    radios: Vec<RawRadio>,
    #[serde(default)]
    button: bool,
    #[serde(default)]
    submit: bool,
}

#[derive(Debug, Default, Deserialize)]
struct RawRadio {
    el: usize,
    value: Option<String>,
    #[serde(default)]
    checked: bool,
    #[serde(default)]
    disabled: bool,
}

/// What `pickOption` answers.
#[derive(Debug, Default, Deserialize)]
struct Picked {
    #[serde(default)]
    found: bool,
    #[serde(default)]
    count: usize,
    #[serde(default)]
    options: Vec<String>,
    #[serde(default)]
    selected: bool,
    name: Option<Candidate>,
}

/// Checks `fill-form`'s arguments without a browser.
pub fn check_fill_form(req: &FillFormRequest) -> Result<(), ErrorInfo> {
    let options = LocateOptions {
        strict_refs: req.strict_refs,
        testid_attribute: req.testid_attribute.clone(),
    };
    if let Some(locator) = &req.locator {
        options.parse(locator)?;
    }
    if req.values.is_empty() {
        return Err(
            ErrorInfo::new(ErrorKind::BadArgs, "no values to fill: the object is empty").hint(
                Hint::run(Cmd::new("fill-form").flag("values", r#"{"Email": "ada@example.com"}"#)),
            ),
        );
    }
    Ok(())
}

impl Engine {
    /// `forms [LOCATOR]`: the forms of the active tab (or `--tab`), or
    /// the one `LOCATOR` names or is inside.
    pub async fn forms(&self, req: FormsRequest) -> Result<Output, ErrorInfo> {
        let options = LocateOptions {
            strict_refs: req.strict_refs,
            testid_attribute: req.testid_attribute.clone(),
        };
        if let Some(locator) = &req.locator {
            options.parse(locator)?;
        }
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let mark = running.tabs.mark();
        let forms = {
            let _tab = self.tabs.lock(&page.target_id).await;
            let work = self.list_forms(&running, &page, req.locator.as_deref(), &options, timeout);
            Box::pin(self.unless_blocked(&running, &page, mark, work)).await?
        };
        let data = FormsData {
            tab: tab_name(page.id),
            forms,
        };
        let text = forms_text(&data.forms);
        let empty: u64 = data.forms.iter().map(|f| f.required_empty).sum();
        Ok(Output::typed(&data, text)
            .quiet(empty.to_string())
            .limit(req.max_chars, "a LOCATOR or --max-chars 0 for all of it"))
    }

    /// The forms `locator` names (see `formsList`), or every form of
    /// every frame, the fields outside any last.
    async fn list_forms(
        &self,
        running: &Running,
        page: &Page,
        locator: Option<&str>,
        options: &LocateOptions,
        timeout: Duration,
    ) -> Result<Vec<FormUnit>, ErrorInfo> {
        if let Some(locator) = locator {
            let parsed = options.parse(locator)?;
            let retry = |r: &str| Cmd::new("forms").arg(r).scoped(&self.session);
            let target = self
                .locate_one(running, page, &parsed, options, Some(&retry), timeout)
                .await?;
            let frame = frame_url(running, page, &target.key.frame);
            let listed = self
                .forms_in(
                    running,
                    page,
                    &target.context,
                    &target.key.frame,
                    Some(&target.object_id),
                    frame,
                    timeout,
                )
                .await;
            target.release();
            return listed;
        }
        let frames = running.tabs.frames(page.id);
        let mut forms = Vec::new();
        let mut outside: Option<FormUnit> = None;
        for (i, frame) in frames.iter().enumerate() {
            let wait = if i == 0 { timeout } else { FRAME_WAIT };
            let context = match running
                .tabs
                .context(page.id, &frame.id, World::Isolated, wait)
                .await
            {
                Ok(context) => context,
                // An iframe still loading (or gone) has no forms yet.
                Err(_) if i > 0 => continue,
                Err(_) => {
                    let (_, context) = self
                        .frame_and_context(running, page, None, World::Isolated, timeout)
                        .await?;
                    context
                }
            };
            let url = (i > 0).then(|| frame.url.clone());
            let listed = self
                .forms_in(running, page, &context, &frame.id, None, url, timeout)
                .await;
            let units = match listed {
                Ok(units) => units,
                Err(_) if i > 0 => continue,
                Err(e) => return Err(e),
            };
            for unit in units {
                if unit.kind != "outside" {
                    forms.push(unit);
                    continue;
                }
                match &mut outside {
                    Some(all) => {
                        all.count += unit.count;
                        all.required_empty += unit.required_empty;
                        all.fields.extend(unit.fields);
                    }
                    None => outside = Some(unit),
                }
            }
        }
        forms.extend(outside);
        Ok(forms)
    }

    /// Calls `formsList` in one frame (on `root`, if given) and names
    /// what it lists, with refs.
    #[allow(clippy::too_many_arguments)]
    async fn forms_in(
        &self,
        running: &Running,
        page: &Page,
        context: &FrameContext,
        frame: &str,
        root: Option<&str>,
        url: Option<String>,
        timeout: Duration,
    ) -> Result<Vec<FormUnit>, ErrorInfo> {
        let generation = running
            .tabs
            .snapshot_memory(page.id, |m| m.refs.generation());
        let mut params = runtime::CallFunctionOnParams::new(format!(
            "function (root) {{ {GUARD} return globalThis.__ccdp.formsList(root || null); }}"
        ));
        params.arguments = Some(vec![runtime::CallArgument {
            value: root.is_none().then_some(Value::Null),
            unserializable_value: None,
            object_id: root.map(str::to_owned),
        }]);
        params.object_group = Some(GROUP.to_owned());
        let (json, keys) = self.listed(context, frame, params, timeout).await?;
        let listed: Listed = serde_json::from_str(&json).unwrap_or_default();
        let names = refer_all(running, page, &listed.names, &keys, generation);
        Ok(listed
            .units
            .into_iter()
            .map(|u| unit(u, &names, url.clone()))
            .collect())
    }

    /// Calls `params` (a function that returns `{json, els}`) in
    /// `context`: the JSON text and the keys of the elements it names.
    async fn listed(
        &self,
        context: &FrameContext,
        frame: &str,
        params: runtime::CallFunctionOnParams,
        timeout: Duration,
    ) -> Result<(String, Vec<Option<NodeKey>>), ErrorInfo> {
        let session = context.session.with_timeout(timeout);
        let result = async {
            let result = injected::call(&session, context.context_id, params)
                .await
                .map_err(|e| from_cdp(&e, &self.session))?;
            if let Some(exception) = &result.exception_details {
                return Err(ErrorInfo::new(
                    ErrorKind::EvalError,
                    format!("listing the forms failed: {}", exception.text),
                ));
            }
            let Some(object_id) = result.result.object_id else {
                return Ok((String::new(), Vec::new()));
            };
            read_listed(&session, &object_id, frame, NAMED)
                .await
                .map_err(|e| from_cdp(&e, &self.session))
        }
        .await;
        let _ = session
            .call(runtime::ReleaseObjectGroupParams::new(GROUP.to_owned()))
            .await;
        result
    }

    /// `fill-form [LOCATOR] --values JSON`: fills the form's fields,
    /// then (`--submit`) submits it, and reports what that caused.
    pub async fn fill_form(&self, req: FillFormRequest) -> Result<Output, ErrorInfo> {
        check_fill_form(&req)?;
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let mark = running.tabs.mark();
        let _tab = self.tabs.lock(&page.target_id).await;
        // The request each field's action is made from; the whole form's
        // report runs as its.
        let template = ActionRequest {
            action: Action::Focus {
                locator: req.locator.clone().unwrap_or_default(),
            },
            tab: req.tab.clone(),
            strict_refs: req.strict_refs,
            testid_attribute: req.testid_attribute.clone(),
            timeout_ms: req.timeout_ms,
            no_wait_after: req.no_wait_after,
            snap: req.snap,
            max_chars: req.max_chars,
        };
        let actor = Actor {
            engine: self,
            running: &running,
            page: &page,
            req: &template,
            options: locate_options(&template),
            timeout,
            deadline: Instant::now() + timeout,
        };
        let filled = async {
            let before = actor.before().await;
            let output = Filler {
                actor: &actor,
                req: &req,
            }
            .run()
            .await?;
            actor.report(output, before, mark).await
        };
        Box::pin(self.unless_blocked(&running, &page, mark, filled)).await
    }
}

/// A form's fill in progress.
struct Filler<'a, 'b> {
    /// The whole form's actor: its report, and what each field's action
    /// is made from.
    actor: &'b Actor<'a>,
    req: &'b FillFormRequest,
}

/// What a key's value does to its field.
#[derive(Debug, Clone, PartialEq)]
enum Plan {
    Fill(String),
    Select(Vec<String>),
    Check(bool),
    /// Checks the group's radio at this index.
    Radio(usize),
    Upload(Vec<String>),
    /// Fills a combobox, then clicks the option with that name.
    Combo(String),
    /// Clicks the listbox's options with these names.
    Pick(Vec<String>),
}

/// How a key matched the fields.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Matched {
    One(usize),
    Several(Vec<usize>),
    None,
}

impl Filler<'_, '_> {
    fn engine(&self) -> &Engine {
        self.actor.engine
    }

    /// The form to fill, as it is now: the one `LOCATOR` names, else the
    /// page's only form (else, with none, the fields outside a form).
    async fn form(&self) -> Result<FormUnit, ErrorInfo> {
        let a = self.actor;
        let mut units = self
            .engine()
            .list_forms(
                a.running,
                a.page,
                self.req.locator.as_deref(),
                &a.options,
                a.timeout,
            )
            .await?;
        if self.req.locator.is_some() && units.len() > 1 {
            return Err(self.ambiguous(&units, "forms are in the element"));
        }
        let forms: Vec<usize> = (0..units.len())
            .filter(|&i| units[i].kind != "outside")
            .collect();
        match forms.len() {
            0 => units.pop().ok_or_else(|| {
                ErrorInfo::new(ErrorKind::NotFound, "the page has no form and no fields").hint(
                    Hint::run(Cmd::new("snapshot").switch("i").scoped(a.session())),
                )
            }),
            1 => Ok(units.swap_remove(forms[0])),
            _ => Err(self.ambiguous(&units, "the page has several forms")),
        }
    }

    fn ambiguous(&self, units: &[FormUnit], why: &str) -> ErrorInfo {
        let forms: Vec<(&FormUnit, &WhyElement)> = units
            .iter()
            .filter_map(|u| u.element.as_ref().map(|e| (u, e)))
            .collect();
        let mut info = ErrorInfo::new(
            ErrorKind::Ambiguous,
            format!("{why} ({}): name the one to fill", forms.len()),
        );
        let mut candidates = Vec::new();
        for (unit, element) in &forms {
            let label = element.reference.clone().unwrap_or_else(|| "?".to_owned());
            // The label is the ref.
            let mut shown = (*unit).clone();
            if let Some(element) = &mut shown.element {
                element.reference = None;
            }
            info = info.note(label, unit_header(&shown));
            candidates.push(json!(element));
        }
        let first = forms
            .first()
            .and_then(|(_, e)| e.reference.clone())
            .unwrap_or_else(|| "<form>".to_owned());
        let values = serde_json::to_string(
            &self
                .req
                .values
                .iter()
                .cloned()
                .collect::<serde_json::Map<_, _>>(),
        )
        .unwrap_or_default();
        info.details(json!({ "candidates": candidates }))
            .hint(Hint::run(
                Cmd::new("fill-form")
                    .arg(first)
                    .flag("values", values)
                    .scoped(self.actor.session()),
            ))
    }

    async fn run(&self) -> Result<Output, ErrorInfo> {
        let mut pending: Vec<(String, Value)> = self.req.values.clone();
        let mut done: HashSet<String> = HashSet::new();
        let mut filled: Vec<FilledField> = Vec::new();
        let mut warnings: Vec<String> = Vec::new();
        let mut lines: Vec<String> = Vec::new();
        let mut last_text: Option<String> = None;
        let mut last: Option<String> = None;
        let mut form = self.form().await?;
        let mut first = true;
        // When a field was last filled: fields may appear for a while.
        let mut since: Option<Instant> = None;
        loop {
            let fields = open_fields(&form, &done);
            let matched: Vec<Matched> = pending
                .iter()
                .map(|(key, _)| {
                    match_key(
                        key,
                        &fields.iter().map(|&i| &form.fields[i]).collect::<Vec<_>>(),
                    )
                })
                .map(|m| match m {
                    Matched::One(i) => Matched::One(fields[i]),
                    Matched::Several(list) => {
                        Matched::Several(list.into_iter().map(|i| fields[i]).collect())
                    }
                    Matched::None => Matched::None,
                })
                .collect();
            // Wrong values fail before anything (more) is filled.
            let mut plans = Vec::new();
            for ((key, value), m) in pending.iter().zip(&matched) {
                if let Matched::One(i) = m {
                    let plan = plan(key, value, &form.fields[*i], self.req.cwd.as_deref())
                        .map_err(|message| bad_value(message, &filled, first))?;
                    plans.push((*i, key.clone(), plan));
                }
            }
            first = false;
            let Some((index, key, plan)) = plans.into_iter().min_by_key(|(i, _, _)| *i) else {
                // Nothing to fill now: wait a little for fields that
                // appear after the last one filled.
                let waiting =
                    since.is_some_and(|t| t.elapsed() < APPEAR_WAIT) && !pending.is_empty();
                if !waiting {
                    let unmatched: Vec<UnmatchedKey> = pending
                        .iter()
                        .zip(&matched)
                        .map(|((key, _), m)| UnmatchedKey {
                            key: key.clone(),
                            candidates: match m {
                                Matched::Several(list) => list
                                    .iter()
                                    .map(|&i| form.fields[i].element.clone())
                                    .collect(),
                                _ => Vec::new(),
                            },
                        })
                        .collect();
                    return self
                        .finish(form, filled, unmatched, lines, warnings, last_text.or(last))
                        .await;
                }
                tokio::time::sleep(POLL).await;
                form = self.form().await?;
                continue;
            };
            let field = form.fields[index].clone();
            let (element, action, line, options) =
                self.fill_one(&field, &plan, &mut warnings).await?;
            lines.push(line);
            done.insert(field_id(&field));
            if is_text(&field) {
                last_text = element.reference.clone();
            }
            last = element.reference.clone().or(last);
            filled.push(FilledField {
                key: key.clone(),
                element,
                action,
                options,
            });
            pending.retain(|(k, _)| *k != key);
            if pending.is_empty() {
                return self
                    .finish(
                        form,
                        filled,
                        Vec::new(),
                        lines,
                        warnings,
                        last_text.or(last),
                    )
                    .await;
            }
            since = Some(Instant::now());
            form = self.form().await?;
        }
    }

    /// Submits (`--submit`, when every key matched), and builds the
    /// output, or the `NotFound` error for the keys left.
    async fn finish(
        &self,
        form: FormUnit,
        filled: Vec<FilledField>,
        unmatched: Vec<UnmatchedKey>,
        mut lines: Vec<String>,
        warnings: Vec<String>,
        last: Option<String>,
    ) -> Result<Output, ErrorInfo> {
        let mut data = FillFormData {
            tab: self.actor.tab(),
            form: form.element.clone(),
            filled,
            unmatched,
            submitted: None,
        };
        if !data.unmatched.is_empty() {
            return Err(unmatched_error(&data, &form, &lines, self.actor.session()));
        }
        if self.req.submit {
            let (submitted, line) = self.submit(&form, last).await?;
            lines.push(line);
            data.submitted = Some(submitted);
        }
        let quiet = data.filled.len().to_string();
        let output = Output::typed(
            &Acted {
                data,
                snapshot: None,
            },
            lines.join("\n"),
        )
        .quiet(quiet);
        Ok(warnings.into_iter().fold(output, Output::warning))
    }

    /// Clicks the form's first submit button, or presses Enter in `last`
    /// (the last text field filled, else the last field).
    async fn submit(
        &self,
        form: &FormUnit,
        last: Option<String>,
    ) -> Result<(FilledField, String), ErrorInfo> {
        let button = form.fields.iter().find(|f| f.submit);
        if let Some(button) = button {
            let reference = ref_of(&button.element)?;
            let action = Action::Click {
                locator: reference,
                button: Button::Left,
                count: 1,
                modifiers: Vec::new(),
                position: None,
                force: false,
                dblclick: false,
            };
            self.act(action).await?;
            let line = format!("submitted: clicked {}", element_text(&button.element));
            return Ok((
                FilledField {
                    key: String::new(),
                    element: button.element.clone(),
                    action: "click".to_owned(),
                    options: Vec::new(),
                },
                line,
            ));
        }
        let Some(last) = last else {
            return Err(ErrorInfo::new(
                ErrorKind::NotFound,
                "nothing to submit with: the form has no submit button and no field was filled",
            ));
        };
        let field = form
            .fields
            .iter()
            .find(|f| f.element.reference.as_deref() == Some(last.as_str()))
            .map(|f| f.element.clone());
        self.act(Action::Press {
            keys: "Enter".to_owned(),
            locator: Some(last.clone()),
        })
        .await?;
        let element = field.unwrap_or(WhyElement {
            reference: Some(last),
            role: String::new(),
            name: String::new(),
            tag: String::new(),
            class: String::new(),
        });
        let line = format!("submitted: pressed Enter in {}", element_text(&element));
        Ok((
            FilledField {
                key: String::new(),
                element,
                action: "press".to_owned(),
                options: Vec::new(),
            },
            line,
        ))
    }

    /// Runs one action through the single command's code, without its
    /// report: the form's covers it.
    async fn act(&self, action: Action) -> Result<Output, ErrorInfo> {
        let req = ActionRequest {
            action,
            ..self.actor.req.clone()
        };
        let actor = Actor {
            engine: self.actor.engine,
            running: self.actor.running,
            page: self.actor.page,
            req: &req,
            options: self.actor.options.clone(),
            timeout: self.actor.timeout,
            deadline: Instant::now() + self.actor.timeout,
        };
        Box::pin(actor.run()).await
    }

    /// Fills one field as `plan` says: the element acted on, the action,
    /// the output line and the options clicked.
    async fn fill_one(
        &self,
        field: &FormField,
        plan: &Plan,
        warnings: &mut Vec<String>,
    ) -> Result<(WhyElement, String, String, Vec<WhyElement>), ErrorInfo> {
        let reference = ref_of(&field.element)?;
        let named = element_text(&shown_element(field));
        let (element, action, line, options) = match plan {
            Plan::Fill(text) => {
                self.collect(
                    self.act(Action::Fill {
                        locator: reference,
                        text: text.clone(),
                        force: false,
                    })
                    .await?,
                    warnings,
                );
                (
                    field.element.clone(),
                    "fill",
                    format!("filled {named}"),
                    Vec::new(),
                )
            }
            Plan::Select(values) => {
                self.collect(
                    self.act(Action::Select {
                        locator: reference,
                        values: values.clone(),
                        force: false,
                    })
                    .await?,
                    warnings,
                );
                (
                    field.element.clone(),
                    "select",
                    format!("selected {named}"),
                    Vec::new(),
                )
            }
            Plan::Check(on) => {
                self.collect(
                    self.act(Action::Check {
                        locator: reference,
                        checked: *on,
                        force: false,
                    })
                    .await?,
                    warnings,
                );
                let verb = if *on { "check" } else { "uncheck" };
                (
                    field.element.clone(),
                    verb,
                    format!("{verb}ed {named}"),
                    Vec::new(),
                )
            }
            Plan::Radio(i) => {
                let radio = &field.radios[*i];
                self.collect(
                    self.act(Action::Check {
                        locator: ref_of(&radio.element)?,
                        checked: true,
                        force: false,
                    })
                    .await?,
                    warnings,
                );
                let line = format!("checked {}", element_text(&radio.element));
                (radio.element.clone(), "check", line, Vec::new())
            }
            Plan::Upload(files) => {
                self.collect(
                    self.act(Action::Upload {
                        locator: reference,
                        files: files.clone(),
                        clear: false,
                        force: false,
                        drop: false,
                    })
                    .await?,
                    warnings,
                );
                (
                    field.element.clone(),
                    "upload",
                    format!("uploaded {named}"),
                    Vec::new(),
                )
            }
            Plan::Combo(text) => {
                self.collect(
                    self.act(Action::Fill {
                        locator: reference.clone(),
                        text: text.clone(),
                        force: false,
                    })
                    .await?,
                    warnings,
                );
                let mut line = format!("filled {named}");
                let mut options = Vec::new();
                match self.pick(&reference, text, true).await? {
                    Some((option, _)) => {
                        self.click(&option).await?;
                        let _ = write!(line, ", clicked {}", element_text(&option));
                        options.push(option);
                    }
                    None => warnings.push(format!(
                        "{named}: no option \"{text}\" showed; left the text typed"
                    )),
                }
                (field.element.clone(), "fill", line, options)
            }
            Plan::Pick(names) => {
                let mut options = Vec::new();
                for name in names {
                    // A listbox fails without the option.
                    if let Some((option, selected)) = self.pick(&reference, name, false).await? {
                        if !selected {
                            self.click(&option).await?;
                        }
                        options.push(option);
                    }
                }
                let shown: Vec<String> = options.iter().map(|o| o.name.clone()).collect();
                let line = format!("selected {named}: {}", shown.join(", "));
                (field.element.clone(), "click", line, options)
            }
        };
        Ok((element, action.to_owned(), line, options))
    }

    /// Keeps an action's warnings.
    #[allow(clippy::unused_self)]
    fn collect(&self, output: Output, warnings: &mut Vec<String>) {
        warnings.extend(output.warnings);
    }

    async fn click(&self, option: &WhyElement) -> Result<(), ErrorInfo> {
        self.act(Action::Click {
            locator: ref_of(option)?,
            button: Button::Left,
            count: 1,
            modifiers: Vec::new(),
            position: None,
            force: false,
            dblclick: false,
        })
        .await
        .map(|_| ())
    }

    /// The option named `name` that the field `reference` offers (a
    /// listbox, or a combobox's popup), looking again for a while, and
    /// whether it is selected already. A combobox (`soft`) whose options
    /// do not show it has `None`; a listbox fails with `NotFound`.
    async fn pick(
        &self,
        reference: &str,
        name: &str,
        soft: bool,
    ) -> Result<Option<(WhyElement, bool)>, ErrorInfo> {
        let a = self.actor;
        let deadline = Instant::now() + OPTION_WAIT.min(a.timeout);
        loop {
            let parsed = a.parse(reference)?;
            let target = a
                .engine
                .locate_one(a.running, a.page, &parsed, &a.options, None, a.timeout)
                .await?;
            let answer = self.pick_on(&target, name).await;
            target.release();
            let (picked, option) = answer?;
            if let (true, Some(option)) = (picked.found, option) {
                return Ok(Some((option, picked.selected)));
            }
            if Instant::now() >= deadline {
                if soft {
                    return Ok(None);
                }
                let listed = if picked.options.is_empty() {
                    "it shows no options".to_owned()
                } else {
                    let more = picked.count.saturating_sub(picked.options.len());
                    let mut listed = format!("options: {}", picked.options.join(", "));
                    if more > 0 {
                        let _ = write!(listed, ", … {more} more");
                    }
                    listed
                };
                return Err(ErrorInfo::new(
                    ErrorKind::NotFound,
                    format!("no option \"{name}\" in {reference}; {listed}"),
                ));
            }
            tokio::time::sleep(POLL).await;
        }
    }

    /// Calls `pickOption` on `target`.
    async fn pick_on(
        &self,
        target: &Target,
        name: &str,
    ) -> Result<(Picked, Option<WhyElement>), ErrorInfo> {
        let a = self.actor;
        let generation = a
            .running
            .tabs
            .snapshot_memory(a.page.id, |m| m.refs.generation());
        let mut params = runtime::CallFunctionOnParams::new(format!(
            "function (name) {{ {GUARD} return globalThis.__ccdp.pickOption(this, name); }}"
        ));
        params.object_id = Some(target.object_id.clone());
        params.arguments = Some(vec![runtime::CallArgument {
            value: Some(json!(name)),
            unserializable_value: None,
            object_id: None,
        }]);
        params.object_group = Some(GROUP.to_owned());
        let (json, keys) = a
            .engine
            .listed(&target.context, &target.key.frame, params, a.timeout)
            .await?;
        let picked: Picked = serde_json::from_str(&json).unwrap_or_default();
        let option = picked.name.as_ref().and_then(|c| {
            refer_all(
                a.running,
                a.page,
                std::slice::from_ref(c),
                &keys,
                generation,
            )
            .pop()
        });
        Ok((picked, option))
    }
}

/// The URL of iframe `frame`; `None` for the main frame.
fn frame_url(running: &Running, page: &Page, frame: &str) -> Option<String> {
    let frames = running.tabs.frames(page.id);
    let found = frames.iter().position(|f| f.id == frame)?;
    (found > 0).then(|| frames[found].url.clone())
}

/// A unit `formsList` listed, its elements named by `names`.
fn unit(raw: RawUnit, names: &[WhyElement], frame: Option<String>) -> FormUnit {
    let name = |i: usize| names.get(i).cloned().unwrap_or_else(nameless);
    let element = raw.el.map(name);
    let kind = if raw.outside {
        "outside"
    } else if raw.scope {
        "scope"
    } else if element.as_ref().is_some_and(|e| e.role == "search") {
        "search"
    } else {
        "form"
    };
    let fields: Vec<FormField> = raw.fields.into_iter().map(|f| field(f, &name)).collect();
    let counted = || fields.iter().filter(|f| !is_button(f));
    let count = counted().count() as u64;
    let required_empty = counted().filter(|f| f.required && f.empty).count() as u64;
    FormUnit {
        kind: kind.to_owned(),
        element,
        method: raw.method,
        action: raw.action,
        frame,
        count,
        required_empty,
        fields,
    }
}

fn field(raw: RawField, name: &dyn Fn(usize) -> WhyElement) -> FormField {
    let (checked, mixed) = match &raw.checked {
        Some(Value::Bool(b)) => (Some(*b), false),
        Some(Value::String(s)) if s == "mixed" => (None, true),
        _ => (None, false),
    };
    FormField {
        element: name(raw.el),
        kind: raw.kind,
        custom: raw.custom,
        required: raw.required,
        disabled: raw.disabled,
        readonly: raw.readonly,
        value: raw.value,
        secret: raw.secret,
        empty: raw.empty,
        checked,
        mixed,
        files: raw.files,
        accept: raw.accept,
        multiple: raw.multiple,
        options: raw.options,
        more_options: raw.more,
        selected: raw.selected,
        invalid: raw.invalid,
        message: raw.message,
        html_name: raw.html,
        id: raw.id,
        placeholder: raw.placeholder,
        radios: raw
            .radios
            .into_iter()
            .map(|r| FormRadio {
                element: name(r.el),
                value: r.value,
                checked: r.checked,
                disabled: r.disabled,
            })
            .collect(),
        submit: raw.button && raw.submit,
    }
}

fn nameless() -> WhyElement {
    WhyElement {
        reference: None,
        role: String::new(),
        name: String::new(),
        tag: String::new(),
        class: String::new(),
    }
}

fn is_button(f: &FormField) -> bool {
    !f.custom && matches!(f.kind.as_str(), "submit" | "reset" | "button")
}

/// A field Enter submits the form from.
fn is_text(f: &FormField) -> bool {
    !f.custom
        && matches!(
            f.kind.as_str(),
            "text"
                | "email"
                | "password"
                | "tel"
                | "url"
                | "search"
                | "number"
                | "date"
                | "time"
                | "datetime-local"
                | "month"
                | "week"
        )
}

/// The field as its line names it: a file input is a `file`, not the
/// button it is to ARIA.
fn shown_element(f: &FormField) -> WhyElement {
    let mut element = f.element.clone();
    if f.kind == "file" && !f.custom {
        "file".clone_into(&mut element.role);
    }
    element
}

/// Whether the line shows the field's kind: a native input's type that
/// its role does not say (`email`, `password`, `date`), a button's.
fn shows_kind(f: &FormField) -> bool {
    !f.custom
        && !matches!(
            f.kind.as_str(),
            "text"
                | "checkbox"
                | "radio"
                | "file"
                | "select"
                | "textarea"
                | "contenteditable"
                | "range"
                | "number"
                | "color"
                | "search"
                | "button"
                | "radiogroup"
        )
}

/// `"Norway"`, quoted as the snapshot quotes names.
fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\\\""))
}

/// `form "Checkout" [e10] → POST /api/orders (5 fields, 2 required
/// empty)`.
fn unit_header(u: &FormUnit) -> String {
    let mut text = match &u.element {
        Some(element) if u.kind != "outside" => element_text(element),
        _ => "fields outside a form".to_owned(),
    };
    if let Some(method) = &u.method {
        let _ = write!(text, " → {method}");
        if let Some(action) = &u.action {
            let _ = write!(text, " {action}");
        }
    }
    if let Some(frame) = &u.frame {
        let _ = write!(text, " in frame {frame}");
    }
    match (u.count, u.required_empty) {
        (0, _) => text.push_str(" (no fields)"),
        (n, 0) => {
            let _ = write!(text, " ({n} field{})", plural(n));
        }
        (n, e) => {
            let _ = write!(text, " ({n} field{}, {e} required empty)", plural(n));
        }
    }
    text
}

fn plural(n: u64) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// One field's line, as `forms` prints it.
fn field_text(f: &FormField) -> String {
    let mut text = if f.kind == "radiogroup" {
        let named =
            matches!(f.element.role.as_str(), "radiogroup" | "group") && !f.element.name.is_empty();
        let mut text = "radiogroup".to_owned();
        if named {
            let _ = write!(text, " {}", quoted(&f.element.name));
        }
        let radios: Vec<String> = f
            .radios
            .iter()
            .map(|r| {
                let mut radio = element_text(&r.element);
                if r.checked {
                    radio.push_str(" checked");
                }
                if r.disabled {
                    radio.push_str(" disabled");
                }
                radio
            })
            .collect();
        let _ = write!(text, ": {}", radios.join(", "));
        text
    } else {
        element_text(&shown_element(f))
    };
    if shows_kind(f) {
        let _ = write!(text, " ({})", f.kind);
    }
    for (on, flag) in [
        (f.required, "required"),
        (f.disabled, "disabled"),
        (f.readonly, "readonly"),
    ] {
        if on {
            let _ = write!(text, " {flag}");
        }
    }
    if is_button(f) || f.kind == "radiogroup" {
        return with_problem(text, f);
    }
    if f.checked.is_some() || f.mixed {
        text.push_str(match (f.checked, f.mixed) {
            (_, true) => " mixed",
            (Some(true), _) => " checked",
            _ => " unchecked",
        });
    } else if f.kind == "file" && !f.custom {
        if let Some(accept) = &f.accept {
            let _ = write!(text, " accept={accept}");
        }
        if f.multiple {
            text.push_str(" multiple");
        }
        if !f.files.is_empty() {
            let _ = write!(text, " files: {}", f.files.join(", "));
        }
    } else if f.multiple || f.element.role == "listbox" {
        if f.selected.is_empty() {
            text.push_str(" selected: none");
        } else {
            let _ = write!(text, " selected: {}", f.selected.join(", "));
        }
    } else if f.secret {
        text.push_str(if f.empty { " value=\"\"" } else { " value=***" });
    } else if let Some(value) = &f.value {
        let _ = write!(text, " value={}", quoted(value));
    }
    if !f.options.is_empty() {
        let _ = write!(text, " options: {}", f.options.join(", "));
        if f.more_options > 0 {
            let _ = write!(text, ", … {} more", f.more_options);
        }
    }
    with_problem(text, f)
}

fn with_problem(mut text: String, f: &FormField) -> String {
    if f.invalid {
        match &f.message {
            Some(message) => {
                let _ = write!(text, " invalid: {}", quoted(message));
            }
            None => text.push_str(" invalid"),
        }
    }
    text
}

/// `forms`' text: each form's header, then its fields, indented.
fn forms_text(forms: &[FormUnit]) -> String {
    if forms.is_empty() {
        return "no forms and no fields on the page".to_owned();
    }
    let mut lines = Vec::new();
    for unit in forms {
        lines.push(unit_header(unit));
        for field in &unit.fields {
            lines.push(format!("  {}", field_text(field)));
        }
    }
    lines.join("\n")
}

/// What identifies a field between two listings: its ref.
fn field_id(f: &FormField) -> String {
    f.element
        .reference
        .clone()
        .unwrap_or_else(|| format!("{} {}", f.kind, f.element.name))
}

/// The fields keys may still match: not buttons, not filled.
fn open_fields(form: &FormUnit, done: &HashSet<String>) -> Vec<usize> {
    (0..form.fields.len())
        .filter(|&i| !is_button(&form.fields[i]) && !done.contains(&field_id(&form.fields[i])))
        .collect()
}

/// A name or key as keys match names: without case, spaces at the ends
/// and a trailing `*` or `:`, whitespace collapsed.
fn normal(s: &str) -> String {
    let mut t = s.trim();
    loop {
        let u = t.trim_end_matches(['*', ':']).trim_end();
        if u == t {
            break;
        }
        t = u;
    }
    t.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The name keys match a field by: a group of radios without a
/// `radiogroup` or `<fieldset>` has none (its element is its first
/// radio).
fn match_name(f: &FormField) -> &str {
    if f.kind == "radiogroup" && !matches!(f.element.role.as_str(), "radiogroup" | "group") {
        return "";
    }
    &f.element.name
}

/// The field `key` names, by the first of these that finds any: the
/// accessible name, the `name` attribute, the `id`, the placeholder,
/// then a part of the name.
fn match_key(key: &str, fields: &[&FormField]) -> Matched {
    let k = normal(key);
    if k.is_empty() {
        return Matched::None;
    }
    let raw = key.trim();
    let tests: [&dyn Fn(&FormField) -> bool; 5] = [
        &|f| normal(match_name(f)) == k,
        &|f| {
            f.html_name
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(raw))
        },
        &|f| f.id.as_deref() == Some(raw),
        &|f| f.placeholder.as_deref().is_some_and(|p| normal(p) == k),
        &|f| {
            let name = normal(match_name(f));
            !name.is_empty() && name.contains(&k)
        },
    ];
    for test in tests {
        let hits: Vec<usize> = (0..fields.len()).filter(|&i| test(fields[i])).collect();
        match hits.len() {
            0 => {}
            1 => return Matched::One(hits[0]),
            _ => return Matched::Several(hits),
        }
    }
    Matched::None
}

/// `"Email"`'s value as text, for fields that take text.
fn as_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The action `value` (under `key`) makes on field `f`, or why the value
/// does not fit it.
fn plan(key: &str, value: &Value, f: &FormField, cwd: Option<&str>) -> Result<Plan, String> {
    let what = format!("{} is {}", quoted(key), element_text(&shown_element(f)));
    let role = f.element.role.as_str();
    let text = || as_text(value);
    if f.kind == "radiogroup" {
        let Some(wanted) = text() else {
            return Err(format!(
                "{what}: give the label (or value) of a choice, not {value}"
            ));
        };
        let w = normal(&wanted);
        let found = f
            .radios
            .iter()
            .position(|r| normal(&r.element.name) == w)
            .or_else(|| {
                f.radios
                    .iter()
                    .position(|r| r.value.as_deref() == Some(wanted.as_str()))
            });
        return found.map(Plan::Radio).ok_or_else(|| {
            let choices: Vec<String> = f.radios.iter().map(|r| quoted(&r.element.name)).collect();
            format!(
                "{what}, which has no choice {}; its choices: {}",
                quoted(&wanted),
                choices.join(", ")
            )
        });
    }
    let checkbox = if f.custom {
        matches!(role, "checkbox" | "switch")
    } else {
        f.kind == "checkbox"
    };
    if checkbox {
        return match value {
            Value::Bool(b) => Ok(Plan::Check(*b)),
            _ => Err(format!("{what}: give true or false, not {value}")),
        };
    }
    let texts = |v: &Value| -> Option<Vec<String>> {
        match v {
            Value::Array(items) => items.iter().map(as_text).collect(),
            other => as_text(other).map(|t| vec![t]),
        }
    };
    if f.kind == "file" && !f.custom {
        let files = match value {
            Value::String(s) => vec![s.clone()],
            Value::Array(items) => items
                .iter()
                .map(|v| v.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| format!("{what}: give a path or an array of paths, not {value}"))?,
            _ => {
                return Err(format!(
                    "{what}: give a path or an array of paths, not {value}"
                ));
            }
        };
        if files.is_empty() {
            return Err(format!("{what}: give at least one path"));
        }
        let files = files
            .into_iter()
            .map(|file| match cwd {
                Some(cwd) if !Path::new(&file).is_absolute() => {
                    Path::new(cwd).join(file).to_string_lossy().into_owned()
                }
                _ => file,
            })
            .collect();
        return Ok(Plan::Upload(files));
    }
    if f.kind == "select" && !f.custom {
        let values = texts(value)
            .ok_or_else(|| format!("{what}: give an option's label or value, not {value}"))?;
        if values.len() > 1 && !f.multiple {
            return Err(format!("{what}, which takes one option, not a list"));
        }
        if values.is_empty() {
            return Err(format!("{what}: give at least one option"));
        }
        return Ok(Plan::Select(values));
    }
    if f.custom && role == "listbox" {
        let names = texts(value).ok_or_else(|| {
            format!("{what}: give an option's name, or an array of them, not {value}")
        })?;
        if names.is_empty() {
            return Err(format!("{what}: give at least one option"));
        }
        return Ok(Plan::Pick(names));
    }
    let Some(text) = text() else {
        return Err(format!("{what}: give a string or a number, not {value}"));
    };
    if role == "combobox" {
        return Ok(Plan::Combo(text));
    }
    Ok(Plan::Fill(text))
}

/// A value that does not fit its field: `BadArgs`.
fn bad_value(message: String, filled: &[FilledField], first: bool) -> ErrorInfo {
    let mut info = ErrorInfo::new(ErrorKind::BadArgs, message);
    if !first && !filled.is_empty() {
        let names: Vec<String> = filled.iter().map(|f| element_text(&f.element)).collect();
        info = info.note("filled before", names.join(", "));
    }
    info.hint(Hint::text(
        "text fields take strings or numbers, checkboxes true or false, a select or radio group \
         a label, a file input a path or an array of paths",
    ))
}

/// The ref of an element of the listing.
fn ref_of(element: &WhyElement) -> Result<String, ErrorInfo> {
    element.reference.clone().ok_or_else(|| {
        ErrorInfo::new(
            ErrorKind::NotFound,
            format!(
                "{} went away while the form was filled",
                element_text(element)
            ),
        )
    })
}

/// The `NotFound` error for keys that matched no field, or several,
/// after the others were filled.
fn unmatched_error(
    data: &FillFormData,
    form: &FormUnit,
    lines: &[String],
    session: &str,
) -> ErrorInfo {
    let parts: Vec<String> = data
        .unmatched
        .iter()
        .map(|u| match u.candidates.len() {
            0 => format!("{} matches no field", quoted(&u.key)),
            n => format!("{} matches {n} fields", quoted(&u.key)),
        })
        .collect();
    let mut info = ErrorInfo::new(ErrorKind::NotFound, parts.join("; "));
    let names: Vec<&str> = form
        .fields
        .iter()
        .filter(|f| !is_button(f))
        .map(match_name)
        .filter(|n| !n.is_empty())
        .collect();
    for u in &data.unmatched {
        if u.candidates.is_empty() {
            let similar = protocol::suggest::similar(&normal(&u.key), names.iter().copied(), 3);
            if !similar.is_empty() {
                let quoted: Vec<String> = similar.iter().map(|s| quoted(s)).collect();
                info = info.note(format!("{}: did you mean", u.key), quoted.join(", "));
            }
        } else {
            let listed: Vec<String> = u.candidates.iter().map(element_text).collect();
            info = info.note(format!("{}: matches", u.key), listed.join(", "));
        }
    }
    if !names.is_empty() {
        let quoted: Vec<String> = names.iter().take(12).map(|s| quoted(s)).collect();
        let more = if names.len() > 12 { ", …" } else { "" };
        info = info.note("fields", format!("{}{more}", quoted.join(", ")));
    }
    if !lines.is_empty() {
        info = info.note("filled", lines.join("; "));
    }
    let mut hint = Cmd::new("forms");
    if let Some(reference) = form.element.as_ref().and_then(|e| e.reference.clone()) {
        hint = hint.arg(reference);
    }
    info.details(json!(data)).hint(
        Hint::run(hint.scoped(session))
            .why("lists the form's fields; use a field's name, or its ref with fill"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(role: &str, name: &str, reference: &str) -> WhyElement {
        WhyElement {
            reference: Some(reference.to_owned()),
            role: role.to_owned(),
            name: name.to_owned(),
            tag: "input".to_owned(),
            class: String::new(),
        }
    }

    fn field(kind: &str, role: &str, name: &str, reference: &str) -> FormField {
        FormField {
            element: element(role, name, reference),
            kind: kind.to_owned(),
            custom: false,
            required: false,
            disabled: false,
            readonly: false,
            value: None,
            secret: false,
            empty: false,
            checked: None,
            mixed: false,
            files: Vec::new(),
            accept: None,
            multiple: false,
            options: Vec::new(),
            more_options: 0,
            selected: Vec::new(),
            invalid: false,
            message: None,
            html_name: None,
            id: None,
            placeholder: None,
            radios: Vec::new(),
            submit: false,
        }
    }

    #[test]
    fn keys_ignore_case_ends_and_marks() {
        assert_eq!(normal("  Card number * "), "card number");
        assert_eq!(normal("Full name:"), "full name");
        assert_eq!(normal("E-mail  address :*"), "e-mail address");
    }

    #[test]
    fn keys_match_name_then_attributes_then_a_part() {
        let mut email = field("email", "textbox", "Email", "e1");
        email.html_name = Some("email".into());
        let mut card = field("text", "textbox", "Card number *", "e2");
        card.id = Some("card".into());
        let mut name = field("text", "textbox", "Full name:", "e3");
        name.placeholder = Some("Ada Lovelace".into());
        let mut news = field("email", "textbox", "Your email", "e4");
        news.html_name = Some("subscriber".into());
        let fields = [&email, &card, &name, &news];
        assert_eq!(match_key("email", &fields), Matched::One(0));
        assert_eq!(match_key("CARD NUMBER", &fields), Matched::One(1));
        assert_eq!(match_key("card", &fields), Matched::One(1));
        assert_eq!(match_key("ada lovelace", &fields), Matched::One(2));
        assert_eq!(match_key("subscriber", &fields), Matched::One(3));
        assert_eq!(match_key("name", &fields), Matched::One(2));
        assert_eq!(match_key("mail", &fields), Matched::Several(vec![0, 3]));
        assert_eq!(match_key("phone", &fields), Matched::None);
        assert_eq!(match_key(" : ", &fields), Matched::None);
    }

    #[test]
    fn values_pick_the_action() {
        let text = field("email", "textbox", "Email", "e1");
        assert_eq!(
            plan("Email", &json!("a@b"), &text, None),
            Ok(Plan::Fill("a@b".into()))
        );
        assert_eq!(
            plan("Qty", &json!(3), &text, None),
            Ok(Plan::Fill("3".into()))
        );
        let err = plan("Email", &json!(true), &text, None).unwrap_err();
        assert_eq!(
            err,
            "\"Email\" is textbox \"Email\" [e1]: give a string or a number, not true"
        );
        let check = field("checkbox", "checkbox", "Save card", "e2");
        assert_eq!(
            plan("Save", &json!(false), &check, None),
            Ok(Plan::Check(false))
        );
        assert!(plan("Save", &json!("yes"), &check, None).is_err());
        let mut select = field("select", "combobox", "Country", "e3");
        assert_eq!(
            plan("Country", &json!("Norway"), &select, None),
            Ok(Plan::Select(vec!["Norway".into()]))
        );
        assert!(plan("Country", &json!(["a", "b"]), &select, None).is_err());
        select.multiple = true;
        assert_eq!(
            plan("Country", &json!(["a", "b"]), &select, None),
            Ok(Plan::Select(vec!["a".into(), "b".into()]))
        );
        let file = field("file", "button", "Receipt", "e4");
        assert_eq!(
            plan("Receipt", &json!("r.pdf"), &file, Some("/work")),
            Ok(Plan::Upload(vec!["/work/r.pdf".into()]))
        );
        assert_eq!(
            plan("Receipt", &json!(["/a.pdf"]), &file, Some("/work")),
            Ok(Plan::Upload(vec!["/a.pdf".into()]))
        );
        let combo = field("text", "combobox", "City", "e5");
        assert_eq!(
            plan("City", &json!("Bergen"), &combo, None),
            Ok(Plan::Combo("Bergen".into()))
        );
        let mut list = field("listbox", "listbox", "Days", "e6");
        list.custom = true;
        assert_eq!(
            plan("Days", &json!(["Mon", "Fri"]), &list, None),
            Ok(Plan::Pick(vec!["Mon".into(), "Fri".into()]))
        );
        let mut switch = field("switch", "switch", "News", "e7");
        switch.custom = true;
        assert_eq!(
            plan("News", &json!(true), &switch, None),
            Ok(Plan::Check(true))
        );
    }

    #[test]
    fn radios_are_picked_by_label_or_value() {
        let mut group = field("radiogroup", "group", "Shipping", "e1");
        for (i, (name, value)) in [("Standard", "standard"), ("Express", "express")]
            .iter()
            .enumerate()
        {
            group.radios.push(FormRadio {
                element: element("radio", name, &format!("e{}", i + 2)),
                value: Some((*value).to_owned()),
                checked: i == 0,
                disabled: false,
            });
        }
        assert_eq!(
            plan("Shipping", &json!("express"), &group, None),
            Ok(Plan::Radio(1))
        );
        assert_eq!(
            plan("Shipping", &json!("Standard"), &group, None),
            Ok(Plan::Radio(0))
        );
        let err = plan("Shipping", &json!("Drone"), &group, None).unwrap_err();
        assert!(
            err.contains("no choice \"Drone\"; its choices: \"Standard\", \"Express\""),
            "{err}"
        );
        assert_eq!(
            field_text(&group),
            "radiogroup \"Shipping\": radio \"Standard\" [e2] checked, radio \"Express\" [e3]"
        );
    }

    #[test]
    fn lines_show_kind_state_and_problems() {
        let mut email = field("email", "textbox", "Email", "e11");
        email.required = true;
        email.value = Some(String::new());
        email.empty = true;
        assert_eq!(
            field_text(&email),
            "textbox \"Email\" [e11] (email) required value=\"\""
        );
        let mut card = field("text", "textbox", "Card number", "e12");
        card.value = Some("12".into());
        card.invalid = true;
        card.message = Some("Please enter a valid card number.".into());
        assert_eq!(
            field_text(&card),
            "textbox \"Card number\" [e12] value=\"12\" invalid: \"Please enter a valid card number.\""
        );
        let mut country = field("select", "combobox", "Country", "e13");
        country.value = Some("Norway".into());
        country.options = vec!["Norway".into(), "Sweden".into()];
        country.more_options = 3;
        assert_eq!(
            field_text(&country),
            "combobox \"Country\" [e13] value=\"Norway\" options: Norway, Sweden, … 3 more"
        );
        let mut save = field("checkbox", "checkbox", "Save card", "e14");
        save.checked = Some(false);
        assert_eq!(field_text(&save), "checkbox \"Save card\" [e14] unchecked");
        let mut file = field("file", "button", "Receipt", "e17");
        file.accept = Some(".pdf".into());
        assert_eq!(field_text(&file), "file \"Receipt\" [e17] accept=.pdf");
        let mut pay = field("submit", "button", "Pay", "e18");
        pay.disabled = true;
        pay.submit = true;
        assert_eq!(field_text(&pay), "button \"Pay\" [e18] (submit) disabled");
        let mut password = field("password", "textbox", "Password", "e19");
        password.secret = true;
        assert_eq!(
            field_text(&password),
            "textbox \"Password\" [e19] (password) value=***"
        );
    }

    #[test]
    fn headers_count_fields() {
        let mut email = field("email", "textbox", "Email", "e11");
        email.required = true;
        email.empty = true;
        let unit = FormUnit {
            kind: "form".into(),
            element: Some(element("form", "Checkout", "e10")),
            method: Some("POST".into()),
            action: Some("/api/orders".into()),
            frame: None,
            count: 1,
            required_empty: 1,
            fields: vec![email],
        };
        assert_eq!(
            unit_header(&unit),
            "form \"Checkout\" [e10] → POST /api/orders (1 field, 1 required empty)"
        );
    }
}

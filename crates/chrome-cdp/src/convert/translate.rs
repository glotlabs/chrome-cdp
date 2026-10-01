//! Playwright calls → `.cdp` lines, following
//! `docs/migrating-from-playwright.md`. Each statement becomes command
//! lines, or one TODO line that quotes it and says why.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use super::lex::TemplatePart;
use super::parse::{Body, Expr, Func, Param, Pattern, Prop, Stmt, StmtKind};
use super::{Line, Part, Word};
use crate::history::quoted_name;

/// Why a statement was not converted.
#[derive(Debug)]
pub(super) struct Why(String);

type Out = Result<Vec<Line>, Why>;

fn why<T>(reason: impl Into<String>) -> Result<T, Why> {
    Err(Why(reason.into()))
}

/// What names mean inside a test (or the file).
#[derive(Debug, Clone, Default)]
struct Env {
    pages: HashSet<String>,
    contexts: HashSet<String>,
    browsers: HashSet<String>,
    /// Locators in variables: `const email = page.getByLabel('Email')`.
    locators: HashMap<String, String>,
    /// Strings in variables: `const email = 'ada@example.com'`.
    words: HashMap<String, Word>,
    /// Waits started but not awaited yet: `const p = page.waitForResponse(…)`.
    pending: HashMap<String, Vec<Line>>,
    /// Constant lists and objects: `const ITEMS = ['a', 'b']`.
    values: HashMap<String, Expr>,
    /// Helper functions, inlined where they are called.
    helpers: HashMap<String, Func>,
    /// Names convert could not give a value, and why: their uses are TODO
    /// lines that say so.
    unknown: HashMap<String, String>,
    /// How many helper calls deep the lines are.
    depth: usize,
    /// A page was opened (script mode): the next one is `tab new`.
    page_open: bool,
    /// The test's title as a file name, for unnamed screenshots.
    slug: String,
    screenshots: usize,
    /// In a test's own body (not a hook): `test.skip()` and
    /// `test.setTimeout(…)` there set [`Env::settings`].
    in_test: bool,
    settings: Settings,
}

impl Env {
    /// Drops what `name` meant, before it gets a new meaning.
    fn forget(&mut self, name: &str) {
        self.pages.remove(name);
        self.contexts.remove(name);
        self.browsers.remove(name);
        self.locators.remove(name);
        self.words.remove(name);
        self.pending.remove(name);
        self.values.remove(name);
        self.helpers.remove(name);
        self.unknown.remove(name);
    }
}

/// A scenario's settings (docs/e2e/scenarios.md), for the file or a test.
#[derive(Debug, Clone, Default, PartialEq)]
struct Settings {
    /// `@skip [reason]`.
    skip: Option<String>,
    only: bool,
    /// `@tags`, without Playwright's leading `@`.
    tags: Vec<String>,
    timeout: Option<String>,
    retries: Option<String>,
    viewport: Option<String>,
    device: Option<String>,
}

impl Settings {
    /// What a runner call's last word says: `test.only`, `describe.skip`,
    /// `test.fixme`, ….
    fn of_call(path: &str) -> Self {
        let last = path.rsplit('.').next();
        Self {
            only: last == Some("only"),
            skip: match last {
                Some("skip") => Some(String::new()),
                Some("fixme") => Some("fixme".into()),
                _ => None,
            },
            ..Self::default()
        }
    }

    /// These settings with `inner`'s on top, as the runner puts a test's
    /// on its file's.
    fn with(&self, inner: &Self) -> Self {
        let (viewport, device) = if inner.viewport.is_some() || inner.device.is_some() {
            (inner.viewport.clone(), inner.device.clone())
        } else {
            (self.viewport.clone(), self.device.clone())
        };
        let mut tags = self.tags.clone();
        for t in &inner.tags {
            if !tags.contains(t) {
                tags.push(t.clone());
            }
        }
        Self {
            skip: inner.skip.clone().or_else(|| self.skip.clone()),
            only: self.only || inner.only,
            tags,
            timeout: inner.timeout.clone().or_else(|| self.timeout.clone()),
            retries: inner.retries.clone().or_else(|| self.retries.clone()),
            viewport,
            device,
        }
    }

    /// The directives, in `chrome-cdp fmt`'s order.
    fn directives(&self) -> Vec<String> {
        let mut out = Vec::new();
        match self.skip.as_deref() {
            Some("") => out.push("@skip".to_owned()),
            Some(reason) => out.push(format!("@skip {}", quoted_name(reason))),
            None => {}
        }
        if self.only {
            out.push("@only".into());
        }
        if !self.tags.is_empty() {
            let tags: Vec<String> = self
                .tags
                .iter()
                .map(|t| Word::lit(t.as_str()).render())
                .collect();
            out.push(format!("@tags {}", tags.join(" ")));
        }
        if let Some(t) = &self.timeout {
            out.push(format!("@timeout {t}"));
        }
        if let Some(r) = &self.retries {
            out.push(format!("@retries {r}"));
        }
        if let Some(v) = &self.viewport {
            out.push(format!("@viewport {v}"));
        }
        if let Some(d) = &self.device {
            out.push(format!("@device {}", quoted_name(d)));
        }
        out
    }
}

/// A `describe` block, or the file.
#[derive(Default)]
struct Scope {
    title: Option<String>,
    uses: Vec<Stmt>,
    before_each: Vec<Func>,
    after_each: Vec<Func>,
    /// From `describe.skip`/`only` and `describe.configure`.
    settings: Settings,
    /// The names its hooks see, once the block is read.
    env: Env,
}

/// A `test(…)`, read; it is converted once every hook is known.
struct TestCall {
    title: String,
    func: Func,
    env: Env,
    /// Its scopes, outermost (the file) first.
    chain: Vec<usize>,
    settings: Settings,
}

pub(super) struct Converter<'s> {
    src: &'s str,
    pub(super) lines: Vec<Line>,
    pub(super) tests: usize,
    scopes: Vec<Scope>,
    /// The open scopes, the file first.
    open: Vec<usize>,
    calls: Vec<TestCall>,
    before_all: Vec<(Func, Env)>,
    after_all: Vec<(Func, Env)>,
    /// Runner calls that cannot be converted, e.g. a conditional
    /// `test.skip(…)` outside a test: TODO lines at the top.
    runner_todos: Vec<Line>,
    /// Statements outside any test: converted as a script when the file
    /// has no tests.
    loose: Vec<Stmt>,
    /// Names the file assigns to or changes in place: not constants.
    mutated: HashSet<String>,
}

impl<'s> Converter<'s> {
    pub(super) fn new(src: &'s str) -> Self {
        Self {
            src,
            lines: Vec::new(),
            tests: 0,
            scopes: vec![Scope::default()],
            open: vec![0],
            calls: Vec::new(),
            before_all: Vec::new(),
            after_all: Vec::new(),
            runner_todos: Vec::new(),
            loose: Vec::new(),
            mutated: super::parse::mutated(src),
        }
    }

    /// Converts the file's statements.
    pub(super) fn file(&mut self, stmts: &[Stmt]) {
        let mut env = Env::default();
        self.walk(stmts, &mut env);
        self.close_scope(env.clone());
        if self.calls.is_empty() {
            let mut env = env;
            env.pages.insert("page".into());
            let loose = std::mem::take(&mut self.loose);
            let lines = self.body(&mut env, &loose);
            self.lines.extend(lines);
        } else {
            self.scenario();
        }
    }

    /// The runner's structure: describe blocks, hooks, `test.use` and tests.
    fn walk(&mut self, stmts: &[Stmt], env: &mut Env) {
        // Tests run once the whole file is read, so they see the helpers
        // and constants declared after them.
        for s in stmts {
            match &s.kind {
                StmtKind::Function { name, func } => {
                    env.forget(name);
                    env.helpers.insert(name.clone(), func.clone());
                }
                StmtKind::Let {
                    pattern: Pattern::Name(name),
                    init,
                } if !is_require(init) => {
                    self.constant(env, name, init);
                }
                _ => {}
            }
        }
        for s in stmts {
            match &s.kind {
                StmtKind::Let { init, .. } if is_require(init) => {}
                StmtKind::Let {
                    pattern: Pattern::Name(name),
                    init,
                } => {
                    if !self.constant(env, name, init) {
                        self.loose.push(s.clone());
                    }
                }
                StmtKind::Expr(e) => {
                    if !self.runner(s, e, env) {
                        self.loose.push(s.clone());
                    }
                }
                StmtKind::Function { .. } | StmtKind::Ignored => {}
                _ => self.loose.push(s.clone()),
            }
        }
    }

    /// Keeps a declaration's value when it is known now and the file does
    /// not change it: a text, a list or object of them, or a helper.
    fn constant(&self, env: &mut Env, name: &str, init: &Expr) -> bool {
        if self.mutated.contains(name) {
            return false;
        }
        if let Some(value) = literal(env, init).filter(|v| !matches!(v, Expr::Str(_))) {
            env.forget(name);
            env.values.insert(name.to_owned(), value);
        } else if let Ok(word) = word(env, init) {
            env.forget(name);
            env.words.insert(name.to_owned(), word);
        } else if let Expr::Func(f) = init {
            env.forget(name);
            env.helpers.insert(name.to_owned(), f.clone());
        } else {
            return false;
        }
        true
    }

    /// Handles a call to the test runner; false when `e` is not one.
    fn runner(&mut self, s: &Stmt, e: &Expr, env: &Env) -> bool {
        let Expr::Call(callee, args) = e.awaited() else {
            return false;
        };
        // `(async () => { … })()`: a script.
        if let Expr::Func(f) = &**callee {
            if let Body::Block(body) = &f.body {
                self.loose.extend(body.iter().cloned());
            }
            return true;
        }
        let Some(path) = callee.path() else {
            return false;
        };
        let title = args.first().and_then(|a| plain(env, a).ok());
        let func = args.iter().rev().find_map(|a| match a {
            Expr::Func(f) => Some(f.clone()),
            _ => None,
        });
        match path.as_str() {
            "test.describe"
            | "test.describe.serial"
            | "test.describe.parallel"
            | "test.describe.only"
            | "test.describe.skip"
            | "test.describe.fixme"
            | "describe"
            | "describe.only"
            | "describe.skip" => {
                let Some(f) = func else { return false };
                let mut settings = Settings::of_call(&path);
                if let Some(Expr::Object(props)) = args.get(1) {
                    for w in details(env, props, &mut settings) {
                        let todo = self.todo(s, &w.0);
                        self.runner_todos.push(todo);
                    }
                }
                self.scopes.push(Scope {
                    title,
                    settings,
                    ..Scope::default()
                });
                self.open.push(self.scopes.len() - 1);
                let mut inner = env.clone();
                if let Body::Block(body) = &f.body {
                    self.walk(body, &mut inner);
                }
                self.close_scope(inner);
                self.open.pop();
            }
            "test.describe.configure" => {
                let Some(Expr::Object(props)) = args.first() else {
                    return true;
                };
                for p in props {
                    match p {
                        Prop::Pair(k, v) if k == "retries" => {
                            if let Ok(n) = number(v) {
                                self.scope().settings.retries = Some(n);
                            }
                        }
                        Prop::Pair(k, v) if k == "timeout" => {
                            if let Ok(t) = duration(v)
                                && t != "0ms"
                            {
                                self.scope().settings.timeout = Some(t);
                            }
                        }
                        _ => {}
                    }
                }
            }
            "test.slow" => {}
            "test.setTimeout" => {
                if let Some(Ok(t)) = args.first().map(duration)
                    && t != "0ms"
                {
                    self.scope().settings.timeout = Some(t);
                }
            }
            "test.use" => self.scope().uses.push(s.clone()),
            "test.beforeEach" | "beforeEach" => {
                if let Some(f) = func {
                    self.scope().before_each.push(f);
                }
            }
            "test.afterEach" | "afterEach" => {
                if let Some(f) = func {
                    self.scope().after_each.push(f);
                }
            }
            "test.beforeAll" | "beforeAll" => {
                if let Some(f) = func {
                    self.before_all.push((f, env.clone()));
                }
            }
            "test.afterAll" | "afterAll" => {
                if let Some(f) = func {
                    self.after_all.push((f, env.clone()));
                }
            }
            "test" | "test.only" | "it" | "it.only" | "test.skip" | "test.fixme" | "it.skip"
                if !matches!(args.first(), Some(Expr::Func(_))) =>
            {
                let Some(func) = func else { return false };
                let mut settings = Settings::of_call(&path);
                if let Some(Expr::Object(props)) = args.get(1) {
                    for w in details(env, props, &mut settings) {
                        let todo = self.todo(s, &w.0);
                        self.runner_todos.push(todo);
                    }
                }
                self.calls.push(TestCall {
                    title: self.full_title(title.as_deref().unwrap_or("test")),
                    func,
                    env: env.clone(),
                    chain: self.open.clone(),
                    settings,
                });
            }
            "test.skip" | "test.fixme" if args.is_empty() => {
                self.scope().settings.skip = Some(if path == "test.fixme" {
                    "fixme".into()
                } else {
                    String::new()
                });
            }
            "test.skip" | "test.fixme" | "test.fail" => match condition(env, args) {
                // A condition that never holds in Chrome, such as
                // `browserName === 'webkit'`.
                Some(false) => {}
                Some(true) if path != "test.fail" => {
                    self.scope().settings.skip = Some(skip_reason(env, &path, args));
                }
                _ => {
                    let todo = self.todo(
                        s,
                        "a runner call with a condition: write @skip on the tests it skips",
                    );
                    self.runner_todos.push(todo);
                }
            },
            _ => return false,
        }
        true
    }

    fn scope(&mut self) -> &mut Scope {
        let open = *self.open.last().unwrap_or(&0);
        &mut self.scopes[open]
    }

    /// Ends the innermost open scope: `env` is what its hooks see, and its
    /// `afterAll` hooks run after its tests.
    fn close_scope(&mut self, env: Env) {
        self.scope().env = env;
    }

    fn full_title(&self, title: &str) -> String {
        self.open
            .iter()
            .filter_map(|&i| self.scopes[i].title.as_deref())
            .chain([title])
            .collect::<Vec<_>>()
            .join(" > ")
    }

    /// The names a hook or test function's fixtures bring.
    fn hook_env(env: &Env, f: &Func) -> Env {
        let mut env = env.clone();
        env.in_test = false;
        for p in &f.params {
            match p.as_str() {
                "page" => {
                    env.forget(p);
                    env.pages.insert(p.clone());
                }
                "context" => {
                    env.forget(p);
                    env.contexts.insert(p.clone());
                }
                // The tests run in Chrome.
                "browserName" => {
                    env.forget(p);
                    env.words.insert(p.clone(), Word::lit("chromium"));
                }
                _ => {}
            }
        }
        env
    }

    /// The scenario of the tests read: settings, hooks and `@test`s. The
    /// scopes every test is in make the file's settings and its `@before`
    /// and `@after`; the steps of inner scopes' hooks are copied into
    /// their tests.
    fn scenario(&mut self) {
        let calls = std::mem::take(&mut self.calls);
        self.tests = calls.len();
        let first = &calls[0].chain;
        let common = calls
            .iter()
            .map(|c| {
                c.chain
                    .iter()
                    .zip(first)
                    .take_while(|(a, b)| a == b)
                    .count()
            })
            .min()
            .unwrap_or(1);
        let shared = first[..common].to_vec();

        // The file's settings.
        let mut settings = Settings::default();
        let mut before = Vec::new();
        let mut locators = HashMap::new();
        for &i in &shared {
            let env = self.scopes[i].env.clone();
            let mut uses = self.scopes[i].settings.clone();
            for s in &self.scopes[i].uses {
                before.extend(self.test_use(&env, s, &mut uses));
            }
            settings = settings.with(&uses);
        }
        // An unnamed toHaveScreenshot() is named after its test, so a hook
        // with one stays in each test.
        let mut named = false;
        for &i in &shared {
            for hook in &self.scopes[i].before_each {
                let mut env = Self::hook_env(&self.scopes[i].env, hook);
                before.extend(self.func_body(&mut env, hook));
                named |= env.screenshots > 0;
                locators.extend(env.locators);
            }
        }
        let mut after = Vec::new();
        for &i in shared.iter().rev() {
            for hook in &self.scopes[i].after_each {
                let mut env = Self::hook_env(&self.scopes[i].env, hook);
                env.locators.extend(locators.clone());
                after.extend(self.func_body(&mut env, hook));
                named |= env.screenshots > 0;
            }
        }
        let hooks_from = if named {
            before.clear();
            after.clear();
            locators.clear();
            0
        } else {
            common
        };
        let hook = |lines: &mut Vec<Line>, lines_of: Vec<Line>, name: &str, pw: &str| {
            if lines_of.iter().any(|l| matches!(l, Line::Cmd(_))) {
                lines.push(Line::Directive {
                    text: format!("@{name}"),
                    block: true,
                });
                lines.extend(lines_of);
                lines.push(Line::Blank);
            } else if !lines_of.is_empty() {
                // A block of comments only would not load.
                lines.push(Line::Comment(format!("{pw}: no step to run")));
                lines.extend(lines_of);
                lines.push(Line::Blank);
            }
        };

        let mut out = std::mem::take(&mut self.runner_todos);
        if !out.is_empty() {
            out.push(Line::Blank);
        }
        let directives = settings.directives();
        if !directives.is_empty() {
            for d in directives {
                out.push(Line::Directive {
                    text: d,
                    block: false,
                });
            }
            out.push(Line::Blank);
        }
        let mut before_all = Vec::new();
        for (f, env) in &self.before_all {
            before_all.extend(self.func_body(&mut Self::hook_env(env, f), f));
        }
        hook(&mut out, before_all, "before-all", "beforeAll");
        hook(&mut out, before, "before", "beforeEach");
        hook(&mut out, after, "after", "afterEach");
        let mut after_all = Vec::new();
        for (f, env) in self.after_all.iter().rev() {
            after_all.extend(self.func_body(&mut Self::hook_env(env, f), f));
        }
        hook(&mut out, after_all, "after-all", "afterAll");

        for call in &calls {
            out.extend(self.test(call, &settings, common, hooks_from, &locators));
        }
        self.lines.extend(out);
    }

    /// A `@test` block: the settings of its scopes after the first
    /// `common` (the file's), the hooks and `test.use` commands of those
    /// after the first `hooks_from`, then its steps.
    fn test(
        &self,
        call: &TestCall,
        file: &Settings,
        common: usize,
        hooks_from: usize,
        locators: &HashMap<String, String>,
    ) -> Vec<Line> {
        let inner = &call.chain[hooks_from..];
        let mut env = Self::hook_env(&call.env, &call.func);
        env.locators.extend(locators.clone());
        env.slug = slug(call.title.rsplit(" > ").next().unwrap_or(&call.title));
        env.screenshots = 0;
        let mut settings = Settings::default();
        let mut body = Vec::new();
        for (depth, &i) in inner.iter().enumerate() {
            let mut uses = self.scopes[i].settings.clone();
            for s in &self.scopes[i].uses {
                body.extend(self.test_use(&env, s, &mut uses));
            }
            if hooks_from + depth >= common {
                settings = settings.with(&uses);
            }
        }
        for &i in inner {
            for hook in &self.scopes[i].before_each {
                let mut hook_env = Self::hook_env(&env, hook);
                body.extend(self.func_body(&mut hook_env, hook));
                env.locators.extend(hook_env.locators);
            }
        }
        env.in_test = true;
        body.extend(self.func_body(&mut env, &call.func));
        for &i in inner.iter().rev() {
            for hook in &self.scopes[i].after_each {
                let mut hook_env = Self::hook_env(&env, hook);
                body.extend(self.func_body(&mut hook_env, hook));
            }
        }
        if file.device.is_some()
            && settings.device.is_none()
            && let Some(size) = settings.viewport.take()
        {
            // @viewport would replace the file's @device; Playwright keeps
            // the device and changes only the size.
            body.insert(0, cmd(["viewport"]).arg(size).line());
        }
        let mut settings = settings.with(&call.settings).with(&env.settings);
        // The file's tags apply already.
        settings.tags.retain(|t| !file.tags.contains(t));
        if settings.skip.is_none() && !body.iter().any(|l| matches!(l, Line::Cmd(_))) {
            // A test without steps does not load unless it is skipped.
            settings.skip = Some(if body.iter().any(|l| matches!(l, Line::Todo { .. })) {
                "convert: rewrite its TODO lines".into()
            } else {
                "convert: no steps".into()
            });
        }
        let mut head = format!("@test {}", quoted_name(&call.title));
        for d in settings.directives() {
            head.push(' ');
            head.push_str(&d);
        }
        let mut lines = vec![Line::Directive {
            text: head,
            block: true,
        }];
        lines.extend(body);
        lines.push(Line::Blank);
        lines
    }

    /// `test.use({ … })`: `@viewport` and `@device` in `settings`, and
    /// emulation commands.
    fn test_use(&self, env: &Env, s: &Stmt, settings: &mut Settings) -> Vec<Line> {
        let StmtKind::Expr(Expr::Call(_, args)) = &s.kind else {
            return vec![self.todo(s, "not understood")];
        };
        match args.first() {
            Some(Expr::Object(props)) => self.options(env, s, props, Some(settings)),
            _ => vec![self.todo(s, "test.use takes an object")],
        }
    }

    /// Context options (`test.use`, `browser.newContext`) as commands.
    /// With `settings` (`test.use`), a device and a plain viewport are
    /// the `@device` and `@viewport` settings instead.
    fn options(
        &self,
        env: &Env,
        s: &Stmt,
        props: &[Prop],
        mut settings: Option<&mut Settings>,
    ) -> Vec<Line> {
        let mut lines = Vec::new();
        let mut get = HashMap::new();
        let spread = props.iter().any(|p| matches!(p, Prop::Spread(_)));
        for p in props {
            match p {
                Prop::Pair(k, v) => {
                    get.insert(k.as_str(), v);
                }
                Prop::Spread(e) => match (device(e), settings.as_deref_mut()) {
                    (Some(name), Some(settings)) => {
                        settings.device = Some(name);
                        settings.viewport = None;
                    }
                    (Some(name), None) => lines.push(cmd(["emulate"]).arg(name).line()),
                    (None, _) => lines.push(self.todo(s, "a spread of options")),
                },
                Prop::Other => lines.push(self.todo(s, "a computed option")),
            }
        }
        let mut keys: Vec<&str> = get.keys().copied().collect();
        keys.sort_by_key(|k| {
            props
                .iter()
                .position(|p| matches!(p, Prop::Pair(n, _) if n == k))
        });
        for key in keys {
            let value = get[key];
            if key == "viewport"
                && !spread
                && !["deviceScaleFactor", "isMobile", "hasTouch"]
                    .iter()
                    .any(|k| get.contains_key(k))
                && let Some(settings) = settings.as_deref_mut()
                && let Ok(size) = size(value)
            {
                settings.viewport = Some(size);
                settings.device = None;
                continue;
            }
            let line = match key {
                "viewport" => viewport(value, &get),
                "deviceScaleFactor" | "isMobile" | "hasTouch" if get.contains_key("viewport") => {
                    continue;
                }
                "locale" => word(env, value).map(|w| cmd(["locale"]).word(w).line()),
                "timezoneId" => word(env, value).map(|w| cmd(["tz"]).word(w).line()),
                "userAgent" => word(env, value).map(|w| cmd(["ua"]).word(w).line()),
                "colorScheme" | "reducedMotion" | "forcedColors" => plain(env, value).map(|v| {
                    let flag = match key {
                        "colorScheme" => "--color-scheme",
                        "reducedMotion" => "--reduced-motion",
                        _ => "--forced-colors",
                    };
                    cmd(["media", flag]).arg(v).line()
                }),
                "geolocation" => geolocation(value),
                "permissions" => strings(env, value).map(|names| {
                    let mut c = cmd(["permissions", "grant"]);
                    for n in names {
                        c = c.arg(n);
                    }
                    c.line()
                }),
                "offline" => match value {
                    Expr::Name(b) if b == "true" => Ok(cmd(["offline", "on"]).line()),
                    Expr::Name(b) if b == "false" => continue,
                    _ => why("offline takes true or false"),
                },
                "storageState" => word(env, value).map(|w| cmd(["state", "load"]).word(w).line()),
                "extraHTTPHeaders" => headers(env, value, "--set-header")
                    .map(|flags| cmd(["route", "**"]).words(flags).line()),
                "javaScriptEnabled" => match value {
                    Expr::Name(b) if b == "false" => {
                        Ok(cmd(["cdp", "Emulation.setScriptExecutionDisabled"])
                            .arg(r#"{"value":true}"#)
                            .line())
                    }
                    _ => continue,
                },
                "baseURL" => {
                    why("set base_url in .chrome-cdp.toml; goto /path resolves against it")
                }
                "headless" => why("the browser starts headless; open --headed for a window"),
                "trace" | "video" | "screenshot" => continue,
                "ignoreHTTPSErrors" | "httpCredentials" | "bypassCSP" | "acceptDownloads" => why(
                    format!("{key}: an open option, see docs/migrating-from-playwright.md"),
                ),
                other => why(format!("the {other} option")),
            };
            lines.push(line.unwrap_or_else(|w| self.todo(s, &w.0)));
        }
        lines
    }

    fn func_body(&self, env: &mut Env, f: &Func) -> Vec<Line> {
        match &f.body {
            Body::Block(stmts) => self.body(env, stmts),
            Body::Expr(e) => {
                let s = Stmt {
                    kind: StmtKind::Expr((**e).clone()),
                    start: f.start,
                    end: f.end,
                };
                self.body(env, std::slice::from_ref(&s))
            }
        }
    }

    fn body(&self, env: &mut Env, stmts: &[Stmt]) -> Vec<Line> {
        for s in stmts {
            if let StmtKind::Function { name, func } = &s.kind {
                env.forget(name);
                env.helpers.insert(name.clone(), func.clone());
            }
        }
        let mut out = Vec::new();
        for s in stmts {
            match self.stmt(env, s) {
                Ok(lines) => out.extend(lines),
                Err(w) => out.push(self.todo(s, &w.0)),
            }
            if matches!(s.kind, StmtKind::Return(_)) {
                // What follows does not run.
                break;
            }
        }
        out
    }

    fn stmt(&self, env: &mut Env, s: &Stmt) -> Out {
        match &s.kind {
            StmtKind::Ignored | StmtKind::Function { .. } | StmtKind::Return(None) => Ok(vec![]),
            StmtKind::ForOf { name, iter, body } => self.for_of(env, name, iter, body),
            StmtKind::Other => {
                let code = self.code(s);
                let first = code
                    .split(|c: char| !c.is_alphanumeric())
                    .next()
                    .unwrap_or("");
                match first {
                    "if" | "for" | "while" | "do" | "switch" => why(
                        "conditions and loops: split into scripts, or drive chrome-cdp from a shell",
                    ),
                    "function" | "class" => why("a helper: inline its steps"),
                    _ => why("not understood"),
                }
            }
            StmtKind::Let { pattern, init } => {
                let result = self.declare(env, s, pattern, init);
                if let Err(w) = &result {
                    let names = match pattern {
                        Pattern::Name(n) => vec![n.clone()],
                        Pattern::Array(names) => names.iter().flatten().cloned().collect(),
                        Pattern::Object(names) => names.clone(),
                    };
                    let line = self.line(s);
                    for n in names {
                        env.forget(&n);
                        env.unknown.insert(
                            n.clone(),
                            format!(
                                "{n} is set on line {line}, which convert could not translate ({})",
                                w.0
                            ),
                        );
                    }
                }
                result
            }
            StmtKind::Expr(e) | StmtKind::Return(Some(e)) => self.expr_stmt(env, s, e),
        }
    }

    /// `for (const x of LIST)` over a list known now: its body once per
    /// item. A body with a step convert cannot translate is one TODO line.
    fn for_of(&self, env: &mut Env, name: &str, iter: &Expr, body: &[Stmt]) -> Out {
        const MAX: usize = 50;
        let Some(Expr::Array(items)) = literal(env, iter) else {
            return why(
                "conditions and loops: split into scripts, or drive chrome-cdp from a shell",
            );
        };
        if items.len() > MAX {
            return why(format!(
                "a loop over {} items: drive chrome-cdp from a shell",
                items.len()
            ));
        }
        let mut out = Vec::new();
        for item in &items {
            let mut inner = env.clone();
            bind(env, &mut inner, name, item);
            let lines = self.body(&mut inner, body);
            if let Some(Line::Todo { reason, line, .. }) =
                lines.iter().find(|l| matches!(l, Line::Todo { .. }))
            {
                return why(format!("in the loop, line {line}: {reason}"));
            }
            env.screenshots = inner.screenshots;
            out.extend(lines);
        }
        Ok(out)
    }

    /// A call of a helper of the file: its body, with the parameters bound
    /// to the arguments. What the body cannot translate stays TODO lines
    /// that name the call.
    fn inline(&self, env: &mut Env, s: &Stmt, name: &str, f: &Func, args: &[Expr]) -> Out {
        const MAX_DEPTH: usize = 8;
        if env.depth >= MAX_DEPTH {
            return why(format!(
                "{name}() calls helpers {MAX_DEPTH} deep: inline its steps"
            ));
        }
        let mut inner = env.clone();
        bind_params(env, &mut inner, name, f, args)?;
        inner.depth += 1;
        inner.in_test = false;
        let mut lines = self.func_body(&mut inner, f);
        env.screenshots = inner.screenshots;
        let call = self.line(s);
        for l in &mut lines {
            if let Line::Todo { reason, .. } = l
                && !reason.starts_with("in ")
            {
                *reason = format!("in {name}(), called on line {call}: {reason}");
            }
        }
        Ok(lines)
    }

    fn declare(&self, env: &mut Env, s: &Stmt, pattern: &Pattern, init: &Expr) -> Out {
        let awaited = matches!(init, Expr::Await(_));
        let value = init.awaited();
        if let Pattern::Name(name) = pattern {
            let constant = !self.mutated.contains(name);
            if constant && let Some(v) = literal(env, value).filter(|v| !matches!(v, Expr::Str(_)))
            {
                env.forget(name);
                env.values.insert(name.clone(), v);
                return Ok(vec![]);
            }
            if let Ok(w) = word(env, value)
                && constant
            {
                env.forget(name);
                env.words.insert(name.clone(), w);
                return Ok(vec![]);
            }
            let loc = Self::locator(env, value);
            if !awaited && let Ok(loc) = &loc {
                env.forget(name);
                env.locators.insert(name.clone(), loc.clone());
                return Ok(vec![]);
            }
            if let Expr::Func(f) = value
                && constant
            {
                env.forget(name);
                env.helpers.insert(name.clone(), f.clone());
                return Ok(vec![]);
            }
            if !awaited && Self::is_context(env, value) {
                env.forget(name);
                env.contexts.insert(name.clone());
                return Ok(vec![]);
            }
            if let Expr::Name(pending) = value
                && let Some(lines) = env.pending.remove(pending)
            {
                return Ok(lines);
            }
            if !awaited && is_wait(value) {
                let lines = Self::call(env, value)?;
                env.pending.insert(name.clone(), lines);
                return Ok(vec![]);
            }
            if let Expr::Call(callee, args) = value
                && let Expr::Member(recv, method) = &**callee
            {
                match (method.as_str(), recv.path().as_deref()) {
                    ("launch", Some("chromium")) => {
                        env.browsers.insert(name.clone());
                        return Ok(match args.first() {
                            Some(Expr::Object(props))
                                if props.iter().any(|p| {
                                    matches!(p, Prop::Pair(k, Expr::Name(v)) if k == "headless" && v == "false")
                                }) =>
                            {
                                vec![cmd(["open", "--headed"]).line()]
                            }
                            _ => vec![],
                        });
                    }
                    ("newContext", Some(b)) if env.browsers.contains(b) => {
                        env.contexts.insert(name.clone());
                        return Ok(match args.first() {
                            Some(Expr::Object(props)) => self.options(env, s, props, None),
                            _ => vec![],
                        });
                    }
                    ("newPage", Some(owner))
                        if env.browsers.contains(owner) || env.contexts.contains(owner) =>
                    {
                        env.pages.insert(name.clone());
                        let first = !env.page_open;
                        env.page_open = true;
                        return Ok(if first {
                            vec![]
                        } else {
                            vec![cmd(["tab", "new"]).line()]
                        });
                    }
                    _ => {}
                }
            }
        }
        if let Pattern::Array(names) = pattern
            && let Some(result) = Self::promise_all(env, value)
        {
            let _ = names;
            return result;
        }
        if awaited {
            Self::call(env, value)?;
            return why(format!(
                "a value kept for later: read it with {}, or check it with expect",
                reader_hint(value)
            ));
        }
        if let Pattern::Name(_) = pattern
            && looks_like_locator(env, value)
            && let Err(w) = Self::locator(env, value)
        {
            return Err(w);
        }
        if let Pattern::Name(name) = pattern
            && self.mutated.contains(name)
        {
            return why(format!("{name} changes later: the script needs one value"));
        }
        if matches!(value, Expr::Member(..) | Expr::Index(..) | Expr::Name(_)) {
            word(env, value)?;
        }
        why("a value the script cannot keep")
    }

    fn expr_stmt(&self, env: &mut Env, s: &Stmt, e: &Expr) -> Out {
        let value = e.awaited();
        if matches!(value, Expr::Other) && is_assignment(&self.code(s)) {
            return why("an assignment: the script has no variables to change");
        }
        if let Expr::Name(name) = value {
            if let Some(lines) = env.pending.remove(name) {
                return Ok(lines);
            }
            return why("not understood");
        }
        if let Some(result) = Self::promise_all(env, value) {
            return result;
        }
        if let Expr::Call(callee, args) = value
            && let Some(path) = callee.path()
        {
            match path.as_str() {
                "test.step" => {
                    let title = args.first().map(|a| plain(env, a)).transpose()?;
                    let Some(Expr::Func(f)) = args.get(1) else {
                        return why("test.step without a function");
                    };
                    let mut lines = vec![Line::Comment(format!(
                        "step: {}",
                        title.unwrap_or_default()
                    ))];
                    lines.extend(self.func_body(env, f));
                    return Ok(lines);
                }
                "test.setTimeout" if env.in_test => {
                    if let Some(Ok(t)) = args.first().map(duration)
                        && t != "0ms"
                    {
                        env.settings.timeout = Some(t);
                    }
                    return Ok(vec![]);
                }
                "test.skip" | "test.fixme" if env.in_test && args.is_empty() => {
                    env.settings.skip = Some(if path == "test.fixme" {
                        "fixme".into()
                    } else {
                        String::new()
                    });
                    return Ok(vec![]);
                }
                "test.slow" | "test.setTimeout" | "console.log" => return Ok(vec![]),
                "test.skip" | "test.fixme" => {
                    return match condition(env, args) {
                        Some(false) => Ok(vec![]),
                        Some(true) if env.in_test => {
                            env.settings.skip = Some(skip_reason(env, &path, args));
                            Ok(vec![])
                        }
                        _ => why(
                            "a runner call with a condition: write @skip on the @test line when it holds",
                        ),
                    };
                }
                "test.fail" | "test.info" => {
                    return why("a runner call");
                }
                _ => {}
            }
        }
        if let Expr::Call(callee, args) = value
            && let Expr::Name(name) = &**callee
        {
            let Some(f) = env.helpers.get(name).cloned() else {
                return why(format!(
                    "{name}() is not defined in this file: inline its steps"
                ));
            };
            return self.inline(env, s, name, &f, args);
        }
        if let Some(expect) = Expect::of(value) {
            return Self::expect(env, &expect);
        }
        Self::call(env, value)
    }

    /// `Promise.all([wait, action])`: the action, then the wait.
    fn promise_all(env: &mut Env, e: &Expr) -> Option<Out> {
        let Expr::Call(callee, args) = e else {
            return None;
        };
        if callee.path().as_deref() != Some("Promise.all") {
            return None;
        }
        let Some(Expr::Array(items)) = args.first() else {
            return Some(why("Promise.all without an array"));
        };
        let mut actions = Vec::new();
        let mut waits = Vec::new();
        for item in items {
            let item = item.awaited();
            let lines = match Self::call(env, item) {
                Ok(lines) => lines,
                Err(w) => return Some(Err(w)),
            };
            if is_wait(item) {
                waits.extend(lines);
            } else {
                actions.extend(lines);
            }
        }
        actions.extend(waits);
        Some(Ok(actions))
    }

    /// A call on a page, context, browser, locator, keyboard or mouse.
    fn call(env: &Env, e: &Expr) -> Out {
        let Expr::Call(callee, args) = e.awaited() else {
            return why("not understood");
        };
        let Expr::Member(recv, method) = &**callee else {
            return match &**callee {
                Expr::Name(name) => why(format!("{name}() is not a Playwright call")),
                _ => why("not a Playwright call"),
            };
        };
        let method = method.as_str();
        if Self::is_page(env, recv) {
            return Self::page(env, method, args);
        }
        if let Expr::Member(owner, name) = &**recv
            && Self::is_page(env, owner)
        {
            match name.as_str() {
                "keyboard" => return keyboard(env, method, args),
                "mouse" => return mouse(method, args),
                "clock" => return clock(env, method, args),
                _ => {}
            }
        }
        if Self::is_context(env, recv) {
            return Self::context(env, method, args);
        }
        if let Some(b) = recv.path()
            && env.browsers.contains(&b)
        {
            return match method {
                "close" => Ok(vec![cmd(["close"]).line()]),
                _ => why(format!("browser.{method}")),
            };
        }
        let loc = Self::locator(env, recv)?;
        Self::act(env, &loc, method, args)
    }

    fn is_page(env: &Env, e: &Expr) -> bool {
        matches!(e, Expr::Name(n) if env.pages.contains(n))
    }

    fn is_context(env: &Env, e: &Expr) -> bool {
        match e {
            Expr::Name(n) => env.contexts.contains(n),
            Expr::Call(callee, args) if args.is_empty() => {
                matches!(&**callee, Expr::Member(p, m) if m == "context" && Self::is_page(env, p))
            }
            _ => false,
        }
    }

    fn page(env: &Env, method: &str, args: &[Expr]) -> Out {
        let arg = |i: usize| args.get(i);
        let one = |c: Cmd| Ok(vec![c.line()]);
        match method {
            "goto" => {
                let url = word(env, need(arg(0), "a URL")?)?;
                let mut c = cmd(["goto"]).word(url);
                for (k, v) in options(arg(1), &["waitUntil", "timeout"])? {
                    c = match k {
                        "waitUntil" => c.flag("--wait", plain(env, v)?),
                        _ => c.flag("--timeout", duration(v)?),
                    };
                }
                one(c)
            }
            "goBack" | "goForward" | "reload" => {
                let name = match method {
                    "goBack" => "back",
                    "goForward" => "forward",
                    _ => "reload",
                };
                let mut c = cmd([name]);
                for (k, v) in options(arg(0), &["waitUntil", "timeout"])? {
                    c = match k {
                        "waitUntil" => c.flag("--wait", plain(env, v)?),
                        _ => c.flag("--timeout", duration(v)?),
                    };
                }
                one(c)
            }
            "waitForURL" => {
                let mut c = cmd(["wait", "--url"]).arg(pattern(env, need(arg(0), "a URL")?)?);
                c = timeout_only(c, arg(1))?;
                one(c)
            }
            "waitForLoadState" => {
                let state = arg(0).map(|a| plain(env, a)).transpose()?;
                let flag = match state.as_deref() {
                    None | Some("load") => "--load",
                    Some("domcontentloaded") => "--domcontentloaded",
                    Some("networkidle") => "--idle",
                    Some(other) => return why(format!("load state {other}")),
                };
                one(timeout_only(cmd(["wait", flag]), arg(1))?)
            }
            "waitForSelector" => {
                let loc = selector(&plain(env, need(arg(0), "a selector")?)?)?;
                let mut c = cmd(["wait"]).arg(loc);
                for (k, v) in options(arg(1), &["state", "timeout", "strict"])? {
                    c = match k {
                        "state" => c.flag("--state", plain(env, v)?),
                        "timeout" => c.flag("--timeout", duration(v)?),
                        _ => c,
                    };
                }
                one(c)
            }
            "waitForTimeout" => one(cmd(["wait"]).arg(duration(need(arg(0), "a time")?)?)),
            "waitForFunction" => {
                let f = need(arg(0), "a function")?;
                let js = match arg(1) {
                    Some(Expr::Name(n)) if n == "undefined" => script_source(env, f, true)?,
                    None => script_source(env, f, true)?,
                    Some(value) => called(env, "waitForFunction", f, None, value)?,
                };
                one(timeout_only(cmd(["wait", "--fn"]).arg(js), arg(2))?)
            }
            "waitForResponse" | "waitForRequest" => {
                let flag = if method == "waitForResponse" {
                    "--response"
                } else {
                    "--request"
                };
                let p = pattern(env, need(arg(0), "a URL")?)?;
                one(timeout_only(cmd(["wait", flag]).arg(p), arg(1))?)
            }
            "waitForEvent" => match plain(env, need(arg(0), "an event")?)?.as_str() {
                "download" => one(cmd(["wait", "--download"])),
                "popup" => {
                    why("a popup opens as a new tab: tabs, then tab t2 (or open --follow-popups)")
                }
                "filechooser" => why("upload <button> <file> clicks it and answers the chooser"),
                other => why(format!("the {other} event")),
            },
            "evaluate" => {
                let f = need(arg(0), "a script")?;
                let js = match arg(1) {
                    Some(value) => called(env, "evaluate", f, None, value)?,
                    None => script_source(env, f, false)?,
                };
                one(cmd(["eval"]).arg(js))
            }
            "screenshot" => {
                let mut c = cmd(["screenshot"]);
                c = screenshot_options(env, c, arg(0))?;
                one(c)
            }
            "pdf" => {
                let mut c = cmd(["pdf"]);
                // The templates print only with `displayHeaderFooter`.
                let mut header_footer = false;
                let mut templates = Vec::new();
                let mut margin = None;
                for (k, v) in options(
                    arg(0),
                    &[
                        "path",
                        "format",
                        "landscape",
                        "printBackground",
                        "scale",
                        "pageRanges",
                        "preferCSSPageSize",
                        "margin",
                        "displayHeaderFooter",
                        "headerTemplate",
                        "footerTemplate",
                    ],
                )? {
                    c = match k {
                        "path" => c.flag("-o", word(env, v)?),
                        "format" => c.flag("--format", word(env, v)?),
                        "landscape" => bool_switch(c, v, "--landscape")?,
                        "scale" => c.flag("--scale", number(v)?),
                        "pageRanges" => c.flag("--pages", plain(env, v)?.replace(' ', "")),
                        "preferCSSPageSize" => bool_switch(c, v, "--css-page-size")?,
                        "margin" => {
                            margin = Some(pdf_margin(v)?);
                            c
                        }
                        "displayHeaderFooter" => {
                            header_footer = boolean(v, "displayHeaderFooter")?;
                            c
                        }
                        "headerTemplate" => {
                            templates.push(("--header", word(env, v)?));
                            c
                        }
                        "footerTemplate" => {
                            templates.push(("--footer", word(env, v)?));
                            c
                        }
                        _ => bool_switch(c, v, "--background")?,
                    };
                }
                // Playwright's margins are 0 unless given; pdf's are 1cm.
                c = c.flag("--margin", margin.unwrap_or_else(|| "0".to_owned()));
                if header_footer {
                    if templates.is_empty() {
                        return why(
                            "displayHeaderFooter without templates: pdf prints Chrome's default \
                             header and footer only through --header and --footer",
                        );
                    }
                    for (flag, template) in templates {
                        c = c.flag(flag, template);
                    }
                }
                one(c)
            }
            "setViewportSize" => one(cmd(["viewport"]).arg(size(need(arg(0), "a size")?)?)),
            "emulateMedia" => {
                let mut c = cmd(["media"]);
                for (k, v) in options(
                    arg(0),
                    &["media", "colorScheme", "reducedMotion", "forcedColors"],
                )? {
                    let flag = match k {
                        "media" => "--type",
                        "colorScheme" => "--color-scheme",
                        "reducedMotion" => "--reduced-motion",
                        _ => "--forced-colors",
                    };
                    c = c.flag(flag, plain(env, v)?);
                }
                one(c)
            }
            "route" => route(env, args),
            "unroute" => why("unroute takes a rule's id: chrome-cdp routes lists them"),
            "unrouteAll" => one(cmd(["unroute", "--all"])),
            "setExtraHTTPHeaders" => {
                let flags = headers(env, need(arg(0), "headers")?, "--set-header")?;
                one(cmd(["route", "**"]).words(flags))
            }
            "addInitScript" => init_script(env, need(arg(0), "a script")?),
            "bringToFront" => why("tab <id> makes a tab active: chrome-cdp tabs lists them"),
            "close" => one(cmd(["tab", "close"])),
            "pause" => why("open --headed, then inspect, to look at the page"),
            "on" | "once" => why(
                "events: dialog accept|dismiss after the step (or open --dialogs), console, errors, network",
            ),
            "content" | "title" | "url" => {
                why(format!("page.{method}(): read it with html or eval"))
            }
            "setContent" => why("goto a data: or file: URL with the HTML"),
            "exposeFunction" | "exposeBinding" => {
                let name = plain(env, need(arg(0), "a name")?)?;
                let mut c = cmd(["expose"]).arg(name);
                // `() => ({ ok: true })`: a fixed answer. Other functions'
                // logic stays behind; their calls are logged.
                if let Some(Expr::Func(Func {
                    body: Body::Expr(value),
                    ..
                })) = arg(1)
                    && let Ok(value) = json(env, value)
                {
                    c = c.flag("--returns", value);
                }
                Ok(vec![c.line()])
            }
            "tap" | "click" | "dblclick" | "hover" | "fill" | "type" | "press" | "check"
            | "uncheck" | "selectOption" | "focus" | "setInputFiles" | "textContent"
            | "innerText" | "getAttribute" | "inputValue" | "isVisible" | "dragAndDrop" => {
                // The deprecated `page.click(selector, …)` form.
                let sel = selector(&plain(env, need(arg(0), "a selector")?)?)?;
                if method == "dragAndDrop" {
                    let to = selector(&plain(env, need(arg(1), "a target")?)?)?;
                    return Ok(vec![cmd(["drag"]).arg(sel).arg(to).line()]);
                }
                Self::act(env, &sel, method, &args[1..])
            }
            other => why(format!("page.{other}")),
        }
    }

    fn context(env: &Env, method: &str, args: &[Expr]) -> Out {
        let arg = |i: usize| args.get(i);
        let one = |c: Cmd| Ok(vec![c.line()]);
        match method {
            "grantPermissions" => {
                let mut c = cmd(["permissions", "grant"]);
                for n in strings(env, need(arg(0), "permissions")?)? {
                    c = c.arg(n);
                }
                for (_, v) in options(arg(1), &["origin"])? {
                    c = c.flag("--origin", word(env, v)?);
                }
                one(c)
            }
            "clearPermissions" => one(cmd(["permissions", "reset"])),
            "setOffline" => match arg(0) {
                Some(Expr::Name(b)) if b == "true" => one(cmd(["offline", "on"])),
                Some(Expr::Name(b)) if b == "false" => one(cmd(["offline", "off"])),
                _ => why("setOffline takes true or false"),
            },
            "setGeolocation" => Ok(vec![geolocation(need(arg(0), "a position")?)?]),
            "clearCookies" => one(cmd(["cookies", "clear"])),
            "storageState" => {
                let mut c = cmd(["state", "save"]);
                let mut path = false;
                for (_, v) in options(arg(0), &["path", "indexedDB"])? {
                    if let Ok(w) = word(env, v) {
                        c = c.word(w);
                        path = true;
                    }
                }
                if !path {
                    return why("storageState without a path: give state save a file");
                }
                one(c)
            }
            "setExtraHTTPHeaders" => {
                let flags = headers(env, need(arg(0), "headers")?, "--set-header")?;
                one(cmd(["route", "**"]).words(flags))
            }
            "addInitScript" => init_script(env, need(arg(0), "a script")?),
            "route" => route(env, args),
            "unrouteAll" => one(cmd(["unroute", "--all"])),
            "newPage" => one(cmd(["tab", "new"])),
            "addCookies" => why("cookies set name=value --url URL, once per cookie"),
            "tracing" => why("trace start / trace stop -o trace.json record a Chrome trace"),
            other => why(format!("context.{other}")),
        }
    }

    /// A method on a locator.
    fn act(env: &Env, loc: &str, method: &str, args: &[Expr]) -> Out {
        let arg = |i: usize| args.get(i);
        let one = |c: Cmd| Ok(vec![c.line()]);
        let on = |name: &str| cmd([name]).arg(loc.to_owned());
        match method {
            "click" | "dblclick" => {
                let mut c = on(method);
                for (k, v) in options(
                    arg(0),
                    &[
                        "button",
                        "modifiers",
                        "position",
                        "force",
                        "clickCount",
                        "noWaitAfter",
                        "timeout",
                        "delay",
                    ],
                )? {
                    c = match k {
                        "button" => c.flag("--button", plain(env, v)?),
                        "modifiers" => c.flag(
                            "--modifiers",
                            strings(env, v)?
                                .into_iter()
                                .map(|m| {
                                    if m == "ControlOrMeta" {
                                        "Mod".into()
                                    } else {
                                        m
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join(","),
                        ),
                        "position" => c.flag("--position", point(v)?),
                        "clickCount" => c.flag("--count", number(v)?),
                        "force" => bool_switch(c, v, "--force")?,
                        "noWaitAfter" => bool_switch(c, v, "--no-wait-after")?,
                        "timeout" => c.flag("--timeout", duration(v)?),
                        _ => c,
                    };
                }
                one(c)
            }
            "hover" | "tap" | "check" | "uncheck" | "focus" | "blur" | "highlight" => {
                let mut c = on(method);
                for (k, v) in options(arg(0), &["force", "timeout", "noWaitAfter"])? {
                    c = match k {
                        "force" if method != "highlight" => bool_switch(c, v, "--force")?,
                        "timeout" => c.flag("--timeout", duration(v)?),
                        _ => c,
                    };
                }
                one(c)
            }
            "setChecked" => match arg(0) {
                Some(Expr::Name(b)) if b == "true" => one(on("check")),
                Some(Expr::Name(b)) if b == "false" => one(on("uncheck")),
                _ => why("setChecked takes true or false"),
            },
            "fill" => {
                let text = word(env, need(arg(0), "a text")?)?;
                one(timeout_force(on("fill").word(text), arg(1))?)
            }
            "clear" => one(timeout_force(on("fill").arg(String::new()), arg(0))?),
            "type" | "pressSequentially" => {
                let text = word(env, need(arg(0), "a text")?)?;
                let mut c = cmd(["type"]).word(text).flag("--in", loc.to_owned());
                for (k, v) in options(arg(1), &["delay", "timeout", "noWaitAfter"])? {
                    c = match k {
                        "delay" => c.flag("--delay", duration(v)?),
                        "timeout" => c.flag("--timeout", duration(v)?),
                        _ => c,
                    };
                }
                one(c)
            }
            "press" => {
                let key = keys(&plain(env, need(arg(0), "a key")?)?);
                one(timeout_only(
                    cmd(["press"]).arg(key).flag("--in", loc.to_owned()),
                    arg(1),
                )?)
            }
            "selectOption" => {
                let mut c = on("select");
                let values = match need(arg(0), "a value")? {
                    Expr::Array(items) => items
                        .iter()
                        .map(|i| option_value(env, i))
                        .collect::<Result<Vec<_>, _>>()?,
                    one => vec![option_value(env, one)?],
                };
                for v in values {
                    c = c.arg(v);
                }
                one(timeout_force(c, arg(1))?)
            }
            "setInputFiles" => {
                let files = match need(arg(0), "files")? {
                    Expr::Array(items) if items.is_empty() => {
                        return one(on("upload").arg("--clear".to_owned()));
                    }
                    Expr::Array(items) => items
                        .iter()
                        .map(|i| word(env, i))
                        .collect::<Result<Vec<_>, _>>()?,
                    one => vec![word(env, one)?],
                };
                one(on("upload").words(files))
            }
            "dragTo" => {
                let target = Self::locator(env, need(arg(0), "a target")?)?;
                one(timeout_force(on("drag").arg(target), arg(1))?)
            }
            "scrollIntoViewIfNeeded" => one(timeout_only(on("scroll"), arg(0))?),
            "screenshot" => one(screenshot_options(env, on("screenshot"), arg(0))?),
            "waitFor" => {
                let mut c = cmd(["wait"]).arg(loc.to_owned());
                for (k, v) in options(arg(0), &["state", "timeout"])? {
                    c = match k {
                        "state" => c.flag("--state", plain(env, v)?),
                        _ => c.flag("--timeout", duration(v)?),
                    };
                }
                one(c)
            }
            "evaluate" => {
                let f = need(arg(0), "a function")?;
                let js = match arg(1) {
                    Some(value) => called(env, "evaluate", f, Some("el"), value)?,
                    None => script_source(env, f, false)?,
                };
                one(cmd(["eval"]).flag("--in", loc.to_owned()).arg(js))
            }
            "dispatchEvent" => {
                let event = plain(env, need(arg(0), "an event")?)?;
                if event.starts_with("drag") || event == "drop" {
                    // The dataTransfer a test builds to drop files cannot
                    // go into the eval.
                    return why(
                        "dispatching drag events: drop files on the element with upload LOC FILE --drop",
                    );
                }
                one(cmd(["eval"]).flag("--in", loc.to_owned()).arg(format!(
                    "el => el.dispatchEvent(new Event({}, {{ bubbles: true }}))",
                    js_string(&event)
                )))
            }
            "textContent" | "innerText" | "allTextContents" | "allInnerTexts" | "inputValue"
            | "getAttribute" | "count" | "isVisible" | "isHidden" | "isEnabled" | "isChecked"
            | "boundingBox" | "innerHTML" => why(format!(
                "{method}() gives a value: read it with {}, or check it with expect",
                reader(method)
            )),
            other => why(format!("locator.{other}")),
        }
    }

    /// A locator expression → a chrome-cdp locator.
    fn locator(env: &Env, e: &Expr) -> Result<String, Why> {
        match e {
            Expr::Name(n) => env.locators.get(n).cloned().ok_or_else(|| {
                Why(env
                    .unknown
                    .get(n)
                    .cloned()
                    .unwrap_or_else(|| format!("{n} is not a locator the script knows")))
            }),
            Expr::Call(callee, args) => {
                if let Expr::Name(name) = &**callee {
                    // A helper that returns a locator.
                    let Some(f) = env.helpers.get(name) else {
                        return why(format!(
                            "{name}() is not defined in this file: inline what it returns"
                        ));
                    };
                    let returned = match &f.body {
                        Body::Expr(e) => Some(&**e),
                        Body::Block(stmts) => match stmts.as_slice() {
                            [
                                Stmt {
                                    kind: StmtKind::Return(Some(e)),
                                    ..
                                },
                            ] => Some(e),
                            _ => None,
                        },
                    };
                    let Some(returned) = returned.filter(|_| env.depth < 8) else {
                        return why(format!("{name}() does more than return a locator"));
                    };
                    let mut inner = env.clone();
                    bind_params(env, &mut inner, name, f, args)?;
                    inner.depth += 1;
                    return Self::locator(&inner, returned.awaited());
                }
                let Expr::Member(recv, method) = &**callee else {
                    return why("not a locator");
                };
                if Self::is_page(env, recv) {
                    return base_locator(env, method, args);
                }
                let before = Self::locator(env, recv)?;
                let step = match method.as_str() {
                    "first" => "nth=0".to_owned(),
                    "last" => "nth=-1".to_owned(),
                    "nth" => format!(
                        "nth={}",
                        known_number(env, need(args.first(), "an index")?)?
                    ),
                    "filter" => return filter(env, &before, args.first()),
                    "contentFrame" => return frame_of(&before),
                    "and" | "or" => return combine(env, &before, method, args.first()),
                    _ => base_locator(env, method, args)?,
                };
                Ok(format!("{before} >> {step}"))
            }
            _ => why("not a locator"),
        }
    }

    fn expect(env: &mut Env, x: &Expect<'_>) -> Out {
        let mut c = cmd(["expect"]);
        let arg = |i: usize| x.args.get(i);
        if Self::is_page(env, x.subject) {
            match x.matcher {
                "toHaveURL" => {
                    c = c.flag("--url", pattern(env, need(arg(0), "a URL")?)?);
                    c = timeout_only(c, arg(1))?;
                }
                "toHaveTitle" => {
                    let title = need(arg(0), "a title")?;
                    c = c.flag("--title", text_value(env, title)?);
                    if !matches!(regex_of(env, title), Expr::Regex { .. }) {
                        c = c.arg("--exact".to_owned());
                    }
                    c = timeout_only(c, arg(1))?;
                }
                "toHaveScreenshot" => {
                    return Self::screenshot_assertion(env, None, x);
                }
                other => return why(format!("expect(page).{other}")),
            }
            if x.not {
                c = c.arg("--not".to_owned());
            }
            return Ok(vec![c.line()]);
        }
        let loc = match Self::locator(env, x.subject) {
            Ok(loc) => loc,
            // A locator convert could not read: say why.
            Err(w)
                if ELEMENT_MATCHERS.contains(&x.matcher) && looks_like_locator(env, x.subject) =>
            {
                return Err(w);
            }
            Err(_) => {
                return why(format!(
                    "{} on a value, not a locator: compare a command's -q output in a shell",
                    x.matcher
                ));
            }
        };
        if x.matcher == "toHaveScreenshot" {
            return Self::screenshot_assertion(env, Some(&loc), x);
        }
        c = c.arg(loc.clone());
        // The index of the options object among the arguments.
        let opts;
        let mut negate = x.not;
        match x.matcher {
            "toBeVisible" | "toBeHidden" | "toBeAttached" | "toBeEnabled" | "toBeDisabled"
            | "toBeEditable" | "toBeChecked" | "toBeFocused" | "toBeInViewport" => {
                let (yes, no, key) = match x.matcher {
                    "toBeVisible" => ("visible", "hidden", "visible"),
                    "toBeHidden" => ("hidden", "visible", ""),
                    "toBeAttached" => ("attached", "detached", "attached"),
                    "toBeEnabled" => ("enabled", "disabled", "enabled"),
                    "toBeDisabled" => ("disabled", "enabled", ""),
                    "toBeEditable" => ("editable", "", "editable"),
                    "toBeChecked" => ("checked", "unchecked", "checked"),
                    "toBeFocused" => ("focused", "", ""),
                    _ => ("in-viewport", "", ""),
                };
                let mut state = yes;
                let mut timeout = None;
                for (k, v) in options(arg(0), &["timeout", key])? {
                    if k == "timeout" {
                        timeout = Some(duration(v)?);
                    } else if matches!(v, Expr::Name(b) if b == "false") {
                        if no.is_empty() {
                            state = yes;
                            negate = !negate;
                        } else {
                            state = no;
                        }
                    }
                }
                c = c.arg(state.to_owned());
                if let Some(t) = timeout {
                    c = c.flag("--timeout", t);
                }
                opts = usize::MAX;
            }
            "toHaveText" | "toContainText" => {
                let value = need(arg(0), "a text")?;
                let known = literal(env, value);
                if let Some(Expr::Array(items)) = known.as_ref().or(Some(value)) {
                    if x.matcher == "toContainText" || negate {
                        return why(
                            "a list of texts: expect LOC count N, then expect 'LOC >> nth=0' text …",
                        );
                    }
                    // Each element's whole text, in order.
                    return each(&loc, items, arg(1), |c, item| {
                        let c = c.arg("text".to_owned()).word(text_word(env, item)?);
                        Ok(if matches!(regex_of(env, item), Expr::Regex { .. }) {
                            c
                        } else {
                            c.arg("--exact".to_owned())
                        })
                    });
                }
                c = c.arg("text".to_owned()).word(text_word(env, value)?);
                if x.matcher == "toHaveText" && !matches!(regex_of(env, value), Expr::Regex { .. })
                {
                    c = c.arg("--exact".to_owned());
                }
                opts = 1;
            }
            "toHaveValue" => {
                c = c
                    .arg("value".to_owned())
                    .word(text_word(env, need(arg(0), "a value")?)?);
                opts = 1;
            }
            "toHaveValues" => {
                let Some(Expr::Array(items)) = arg(0) else {
                    return why("toHaveValues takes a list");
                };
                c = c.arg("values".to_owned());
                for i in items {
                    c = c.word(text_word(env, i)?);
                }
                opts = 1;
            }
            "toHaveAttribute" => {
                c = c
                    .arg("attr".to_owned())
                    .arg(plain(env, need(arg(0), "a name")?)?);
                match arg(1) {
                    Some(Expr::Object(_)) | None => opts = 1,
                    Some(v) => {
                        c = c.word(text_word(env, v)?);
                        opts = 2;
                    }
                }
            }
            "toHaveId" => {
                c = c
                    .arg("attr".to_owned())
                    .arg("id".to_owned())
                    .word(text_word(env, need(arg(0), "an id")?)?);
                opts = 1;
            }
            "toHaveClass" => {
                let value = need(arg(0), "a class")?;
                let known = literal(env, value);
                if let Some(Expr::Array(items)) = known.as_ref().or(Some(value)) {
                    if negate {
                        return why("a negated list of classes: one expect per element");
                    }
                    return each(&loc, items, arg(1), |c, item| whole_class(env, c, item));
                }
                c = whole_class(env, c, value)?;
                opts = 1;
            }
            "toContainClass" => {
                c = c
                    .arg("class".to_owned())
                    .word(word(env, need(arg(0), "a class")?)?);
                opts = 1;
            }
            "toHaveCSS" => {
                c = c
                    .arg("css".to_owned())
                    .arg(plain(env, need(arg(0), "a property")?)?)
                    .word(text_word(env, need(arg(1), "a value")?)?);
                opts = 2;
            }
            "toHaveCount" => {
                c = c
                    .arg("count".to_owned())
                    .arg(known_number(env, need(arg(0), "a count")?)?);
                opts = 1;
            }
            "toBeEmpty"
                if loc
                    .rsplit(" >> ")
                    .next()
                    .is_some_and(|l| l.starts_with("placeholder=")) =>
            {
                // Only fields have a placeholder.
                c = c.arg("value".to_owned()).arg(String::new());
                opts = 0;
            }
            "toBeEmpty" => {
                return why("toBeEmpty: expect LOC text '' --exact, or value '' for a field");
            }
            "toMatchAriaSnapshot" => {
                return why(
                    "an ARIA template: expect-snapshot NAME --scope LOC stores a baseline; edit it into a template (\"*\", /regex/, ...)",
                );
            }
            other => return why(format!("the {other} matcher")),
        }
        if opts != usize::MAX {
            c = timeout_only(c, arg(opts))?;
        }
        if negate {
            c = c.arg("--not".to_owned());
        }
        Ok(vec![c.line()])
    }

    fn screenshot_assertion(env: &mut Env, loc: Option<&str>, x: &Expect<'_>) -> Out {
        if x.not {
            return why("a negated screenshot assertion");
        }
        let (name, opts) = match x.args.first() {
            Some(Expr::Object(_)) | None => {
                env.screenshots += 1;
                let base = if env.slug.is_empty() {
                    "screenshot"
                } else {
                    env.slug.as_str()
                };
                (format!("{base}-{}", env.screenshots), x.args.first())
            }
            Some(Expr::Array(parts)) => (
                parts
                    .iter()
                    .map(|p| plain(env, p))
                    .collect::<Result<Vec<_>, _>>()?
                    .join("/"),
                x.args.get(1),
            ),
            Some(other) => (plain(env, other)?, x.args.get(1)),
        };
        let name = name.strip_suffix(".png").unwrap_or(&name).to_owned();
        let mut c = cmd(["expect-screenshot"]).arg(name);
        if let Some(loc) = loc {
            c = c.arg(loc.to_owned());
        }
        for (k, v) in options(
            opts,
            &[
                "fullPage",
                "mask",
                "threshold",
                "maxDiffPixels",
                "timeout",
                "animations",
                "caret",
            ],
        )? {
            c = match k {
                "fullPage" => bool_switch(c, v, "--full")?,
                "mask" => {
                    let Expr::Array(items) = v else {
                        return why("mask takes a list of locators");
                    };
                    for item in items {
                        c = c.flag("--mask", Self::locator(env, item)?);
                    }
                    c
                }
                "threshold" => c.flag("--threshold", number(v)?),
                "maxDiffPixels" => c.flag("--max-diff-pixels", number(v)?),
                "timeout" => c.flag("--timeout", duration(v)?),
                _ => c,
            };
        }
        Ok(vec![c.line()])
    }

    fn todo(&self, s: &Stmt, reason: &str) -> Line {
        Line::Todo {
            line: self.line(s),
            reason: reason.to_owned(),
            code: self.code(s),
        }
    }

    /// The statement's line in the spec file.
    fn line(&self, s: &Stmt) -> usize {
        self.src[..s.start.min(self.src.len())]
            .matches('\n')
            .count()
            + 1
    }

    /// The statement's source on one line.
    fn code(&self, s: &Stmt) -> String {
        const MAX: usize = 120;
        let text = self.src.get(s.start..s.end).unwrap_or("");
        let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if flat.chars().count() > MAX {
            let cut: String = flat.chars().take(MAX - 1).collect();
            format!("{cut}…")
        } else {
            flat
        }
    }
}

/// The matchers of an element (or the page), not of a value.
const ELEMENT_MATCHERS: &[&str] = &[
    "toBeVisible",
    "toBeHidden",
    "toBeAttached",
    "toBeEnabled",
    "toBeDisabled",
    "toBeEditable",
    "toBeChecked",
    "toBeFocused",
    "toBeInViewport",
    "toBeEmpty",
    "toHaveText",
    "toContainText",
    "toHaveValue",
    "toHaveValues",
    "toHaveAttribute",
    "toHaveId",
    "toHaveClass",
    "toContainClass",
    "toHaveCSS",
    "toHaveCount",
    "toHaveScreenshot",
    "toMatchAriaSnapshot",
    "toHaveURL",
    "toHaveTitle",
];

/// A list matcher as `expect LOC count N`, then one check per element.
fn each(
    loc: &str,
    items: &[Expr],
    opts: Option<&Expr>,
    check: impl Fn(Cmd, &Expr) -> Result<Cmd, Why>,
) -> Out {
    let count = cmd(["expect"])
        .arg(loc.to_owned())
        .arg("count")
        .arg(items.len().to_string());
    let mut lines = vec![timeout_only(count, opts)?.line()];
    for (i, item) in items.iter().enumerate() {
        let c = check(cmd(["expect"]).arg(format!("{loc} >> nth={i}")), item)?;
        lines.push(timeout_only(c, opts)?.line());
    }
    Ok(lines)
}

/// `toHaveClass`'s value: the whole class attribute, which is `''` for an
/// element without one.
fn whole_class(env: &Env, c: Cmd, value: &Expr) -> Result<Cmd, Why> {
    let value = literal(env, value).unwrap_or_else(|| value.clone());
    if matches!(&value, Expr::Str(s) if s.trim().is_empty()) {
        return Ok(c.arg("class".to_owned()).arg("/^$/".to_owned()));
    }
    Ok(c.arg("attr".to_owned())
        .arg("class".to_owned())
        .word(text_word(env, &value)?))
}

/// `e` is a locator chain: from a page, a locator, a name convert could not
/// read, or a helper call.
fn looks_like_locator(env: &Env, e: &Expr) -> bool {
    match e {
        Expr::Name(n) => {
            env.pages.contains(n) || env.locators.contains_key(n) || env.unknown.contains_key(n)
        }
        Expr::Call(callee, _) => match &**callee {
            Expr::Name(_) => true,
            Expr::Member(recv, _) => looks_like_locator(env, recv),
            _ => false,
        },
        _ => false,
    }
}

/// `expect(subject)[.not].matcher(args)`.
struct Expect<'e> {
    subject: &'e Expr,
    not: bool,
    matcher: &'e str,
    args: &'e [Expr],
}

impl<'e> Expect<'e> {
    fn of(e: &'e Expr) -> Option<Self> {
        let Expr::Call(callee, args) = e else {
            return None;
        };
        let Expr::Member(target, matcher) = &**callee else {
            return None;
        };
        let (target, not) = match &**target {
            Expr::Member(inner, n) if n == "not" => (&**inner, true),
            other => (other, false),
        };
        let Expr::Call(f, subject) = target else {
            return None;
        };
        if !matches!(f.path().as_deref(), Some("expect" | "expect.soft")) {
            return None;
        }
        Some(Self {
            subject: subject.first()?,
            not,
            matcher,
            args,
        })
    }
}

/// A command being built.
pub(super) struct Cmd(Vec<Word>);

fn cmd<const N: usize>(words: [&str; N]) -> Cmd {
    Cmd(words.iter().map(|w| Word::lit(*w)).collect())
}

impl Cmd {
    fn arg(mut self, text: impl Into<String>) -> Self {
        self.0.push(Word::lit(text));
        self
    }

    fn word(mut self, word: Word) -> Self {
        self.0.push(word);
        self
    }

    fn words(mut self, words: Vec<Word>) -> Self {
        self.0.extend(words);
        self
    }

    fn flag(self, flag: &str, value: impl Into<Word>) -> Self {
        self.arg(flag).word(value.into())
    }

    fn line(self) -> Line {
        Line::Cmd(self.0)
    }
}

impl From<String> for Word {
    fn from(text: String) -> Self {
        Word::lit(text)
    }
}

fn need<'e>(e: Option<&'e Expr>, what: &str) -> Result<&'e Expr, Why> {
    e.ok_or_else(|| Why(format!("needs {what}")))
}

/// A string value, which may read environment variables:
/// `process.env.PASSWORD` is `${env:PASSWORD}`.
fn word(env: &Env, e: &Expr) -> Result<Word, Why> {
    match e {
        Expr::Str(s) => Ok(Word::lit(s.clone())),
        Expr::Num(n) => Ok(Word::lit(n.clone())),
        Expr::Name(n) if env.words.contains_key(n) => Ok(env.words[n].clone()),
        Expr::Template(parts) => {
            let mut out = Word(Vec::new());
            for p in parts {
                match p {
                    TemplatePart::Text(t) => out.0.push(Part::Lit(t.clone())),
                    TemplatePart::Expr(src) => {
                        let inner = super::parse::parse(src);
                        let [stmt] = inner.as_slice() else {
                            return why("a template with an expression");
                        };
                        let StmtKind::Expr(e) = &stmt.kind else {
                            return why("a template with an expression");
                        };
                        out.0.extend(word(env, e)?.0);
                    }
                }
            }
            Ok(out)
        }
        Expr::Binary("+", a, b) => {
            let mut out = word(env, a)?;
            out.0.extend(word(env, b)?.0);
            Ok(out)
        }
        Expr::Binary("??" | "||", a, _) => word(env, a),
        Expr::Name(n) if env.unknown.contains_key(n) => Err(Why(env.unknown[n].clone())),
        other => match other.path() {
            Some(path) if path.starts_with("process.env.") => Ok(Word(vec![Part::Var(format!(
                "env:{}",
                &path["process.env.".len()..]
            ))])),
            _ => match literal(env, other) {
                Some(Expr::Str(s) | Expr::Num(s)) => Ok(Word::lit(s)),
                _ => why("a value computed at run time"),
            },
        },
    }
}

/// A value known when converting, as a literal: texts, numbers, booleans,
/// and lists and objects of them, read through constants, `[i]`, `.key`,
/// `.length` and `.slice()`.
fn literal(env: &Env, e: &Expr) -> Option<Expr> {
    match e {
        Expr::Str(_) | Expr::Num(_) | Expr::Regex { .. } => Some(e.clone()),
        Expr::Name(n) if matches!(n.as_str(), "true" | "false" | "null") => Some(e.clone()),
        Expr::Name(n) => env.values.get(n).cloned().or_else(|| {
            let w = env.words.get(n)?;
            match w.0.as_slice() {
                [] => Some(Expr::Str(String::new())),
                [Part::Lit(t)] => Some(Expr::Str(t.clone())),
                _ => None,
            }
        }),
        Expr::Unary("-", inner) => match &**inner {
            Expr::Num(_) => Some(e.clone()),
            _ => None,
        },
        Expr::Template(_) | Expr::Binary("+", ..) => plain(env, e).ok().map(Expr::Str),
        Expr::Array(items) => {
            let mut out = Vec::new();
            for item in items {
                match item {
                    Expr::Spread(inner) => match literal(env, inner)? {
                        Expr::Array(more) => out.extend(more),
                        _ => return None,
                    },
                    other => out.push(literal(env, other)?),
                }
            }
            Some(Expr::Array(out))
        }
        Expr::Object(props) => {
            let mut out = Vec::new();
            for p in props {
                match p {
                    Prop::Pair(k, v) => out.push(Prop::Pair(k.clone(), literal(env, v)?)),
                    Prop::Spread(inner) => match literal(env, inner)? {
                        Expr::Object(more) => out.extend(more),
                        _ => return None,
                    },
                    Prop::Other => return None,
                }
            }
            Some(Expr::Object(out))
        }
        Expr::Index(object, key) => match (literal(env, object)?, literal(env, key)?) {
            (Expr::Array(items), Expr::Num(i)) => items.into_iter().nth(i.parse().ok()?),
            (Expr::Object(props), Expr::Str(k) | Expr::Num(k)) => member(&props, &k),
            _ => None,
        },
        Expr::Member(object, key) => match literal(env, object)? {
            Expr::Object(props) => member(&props, key),
            Expr::Array(items) if key == "length" => Some(Expr::Num(items.len().to_string())),
            Expr::Str(s) if key == "length" => Some(Expr::Num(s.chars().count().to_string())),
            _ => None,
        },
        Expr::Call(callee, args) => {
            let Expr::Member(object, method) = &**callee else {
                return None;
            };
            let Expr::Array(items) = literal(env, object)? else {
                return None;
            };
            if method != "slice" {
                return None;
            }
            // Negative indexes count from the end.
            let index = |a: Option<&Expr>, default: usize| -> Option<usize> {
                let Some(a) = a else { return Some(default) };
                let n: i64 = number(&literal(env, a)?).ok()?.parse().ok()?;
                let back = usize::try_from(n.unsigned_abs()).ok()?;
                Some(if n < 0 {
                    items.len().saturating_sub(back)
                } else {
                    back.min(items.len())
                })
            };
            let from = index(args.first(), 0)?;
            let to = index(args.get(1), items.len())?;
            Some(Expr::Array(items.get(from..to.max(from))?.to_vec()))
        }
        _ => None,
    }
}

/// An object literal's value for `key`.
fn member(props: &[Prop], key: &str) -> Option<Expr> {
    props.iter().rev().find_map(|p| match p {
        Prop::Pair(k, v) if k == key => Some(v.clone()),
        _ => None,
    })
}

/// Gives `name` in `inner` what `value` means in `outer`: a page, context,
/// text, constant, locator or helper. A value known only at run time makes
/// the uses of `name` TODO lines.
fn bind(outer: &Env, inner: &mut Env, name: &str, value: &Expr) {
    inner.forget(name);
    let value = value.awaited();
    if Converter::is_page(outer, value) {
        inner.pages.insert(name.to_owned());
    } else if Converter::is_context(outer, value) {
        inner.contexts.insert(name.to_owned());
    } else if let Some(v) = literal(outer, value).filter(|v| !matches!(v, Expr::Str(_))) {
        inner.values.insert(name.to_owned(), v);
    } else if let Ok(w) = word(outer, value) {
        inner.words.insert(name.to_owned(), w);
    } else if let Ok(loc) = Converter::locator(outer, value) {
        inner.locators.insert(name.to_owned(), loc);
    } else if let Expr::Func(f) = value {
        inner.helpers.insert(name.to_owned(), f.clone());
    } else if let Expr::Name(n) = value
        && let Some(f) = outer.helpers.get(n)
    {
        inner.helpers.insert(name.to_owned(), f.clone());
    } else {
        let why = match value {
            Expr::Name(n) if outer.unknown.contains_key(n) => outer.unknown[n].clone(),
            _ => format!("{name} is computed at run time"),
        };
        inner.unknown.insert(name.to_owned(), why);
    }
}

/// Binds a helper's parameters to a call's arguments.
fn bind_params(
    outer: &Env,
    inner: &mut Env,
    name: &str,
    f: &Func,
    args: &[Expr],
) -> Result<(), Why> {
    for (i, p) in f.positional.iter().enumerate() {
        let arg = args
            .get(i)
            .filter(|a| !matches!(a, Expr::Name(n) if n == "undefined"));
        match p {
            Param::Name(n) => {
                if let Some(a) = arg {
                    bind(outer, inner, n, a);
                } else {
                    inner.forget(n);
                    inner
                        .unknown
                        .insert(n.clone(), format!("{name}() is called without {n}"));
                }
            }
            Param::Object(keys) => {
                let props = match arg.map(|a| literal(outer, a).unwrap_or_else(|| a.clone())) {
                    Some(Expr::Object(props)) => props,
                    None => Vec::new(),
                    Some(_) => return why(format!("{name}() takes an object convert cannot read")),
                };
                for (key, local) in keys {
                    if let Some(v) = member(&props, key) {
                        bind(outer, inner, local, &v);
                    } else {
                        inner.forget(local);
                        inner
                            .unknown
                            .insert(local.clone(), format!("{name}() is called without {key}"));
                    }
                }
            }
            Param::Other => return why(format!("{name}() takes a parameter convert cannot bind")),
        }
    }
    Ok(())
}

/// Whether a condition holds in Chrome, when convert can tell: `true`,
/// `browserName === 'webkit'`, `!x`, `a && b`. For a function (`({
/// browserName }) => …`), what it returns.
fn condition(env: &Env, args: &[Expr]) -> Option<bool> {
    let first = args.first()?;
    let Expr::Func(f) = first else {
        return truth(env, first);
    };
    let env = Converter::hook_env(env, f);
    match &f.body {
        Body::Expr(e) => truth(&env, e),
        Body::Block(stmts) => match stmts.as_slice() {
            [
                Stmt {
                    kind: StmtKind::Return(Some(e)),
                    ..
                },
            ] => truth(&env, e),
            _ => None,
        },
    }
}

fn truth(env: &Env, e: &Expr) -> Option<bool> {
    match e {
        Expr::Unary("!", inner) => truth(env, inner).map(|held| !held),
        Expr::Binary("&&", left, right) => match (truth(env, left), truth(env, right)) {
            (Some(false), _) | (_, Some(false)) => Some(false),
            (Some(true), Some(true)) => Some(true),
            _ => None,
        },
        Expr::Binary("||", left, right) => match (truth(env, left), truth(env, right)) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (Some(false), Some(false)) => Some(false),
            _ => None,
        },
        Expr::Binary(op @ ("===" | "==" | "!==" | "!="), left, right) => {
            let same = match (literal(env, left)?, literal(env, right)?) {
                (Expr::Num(one), Expr::Num(other)) => one
                    .parse::<f64>()
                    .ok()?
                    .total_cmp(&other.parse::<f64>().ok()?)
                    .is_eq(),
                (Expr::Str(one), Expr::Str(other)) | (Expr::Name(one), Expr::Name(other)) => {
                    one == other
                }
                _ => return None,
            };
            Some(same == op.starts_with('='))
        }
        other => match literal(env, other)? {
            Expr::Name(word) if word == "true" => Some(true),
            Expr::Name(word) if word == "false" || word == "null" => Some(false),
            Expr::Str(text) => Some(!text.is_empty()),
            _ => None,
        },
    }
}

/// The `@skip` reason of `test.skip(condition, 'why')` or `test.fixme`.
fn skip_reason(env: &Env, path: &str, args: &[Expr]) -> String {
    match args.get(1).and_then(|a| plain(env, a).ok()) {
        Some(reason) => reason,
        None if path.ends_with("fixme") => "fixme".into(),
        None => String::new(),
    }
}

/// A test's or describe's details object (`{ tag: '@smoke', annotation:
/// { type: 'skip' } }`) into `settings`; what it cannot read, as reasons.
fn details(env: &Env, props: &[Prop], settings: &mut Settings) -> Vec<Why> {
    let mut out = Vec::new();
    for p in props {
        let Prop::Pair(key, value) = p else {
            out.push(Why("a details object computed at run time".into()));
            continue;
        };
        match key.as_str() {
            "tag" => match strings(env, value) {
                Ok(tags) => {
                    for t in tags {
                        let t = t.trim_start_matches('@').to_owned();
                        if !t.is_empty() && !settings.tags.contains(&t) {
                            settings.tags.push(t);
                        }
                    }
                }
                Err(w) => out.push(w),
            },
            "annotation" => {
                let value = literal(env, value).unwrap_or_else(|| value.clone());
                let list = match value {
                    Expr::Array(items) => items,
                    one => vec![one],
                };
                for a in &list {
                    let Expr::Object(fields) = a else {
                        out.push(Why("an annotation computed at run time".into()));
                        continue;
                    };
                    let text = |k: &str| member(fields, k).and_then(|v| plain(env, &v).ok());
                    // Other types (issue, …) are notes for the report.
                    match text("type").as_deref() {
                        Some(kind @ ("skip" | "fixme")) => {
                            settings.skip = Some(text("description").unwrap_or_else(|| {
                                if kind == "fixme" {
                                    "fixme".into()
                                } else {
                                    String::new()
                                }
                            }));
                        }
                        Some(_) => {}
                        None => out.push(Why("an annotation without a type".into())),
                    }
                }
            }
            other => out.push(Why(format!("the {other} detail"))),
        }
    }
    out
}

/// A string known when converting (no variables).
fn plain(env: &Env, e: &Expr) -> Result<String, Why> {
    let w = word(env, e)?;
    let mut out = String::new();
    for p in w.0 {
        match p {
            Part::Lit(t) => out.push_str(&t),
            Part::Var(_) => return why("an environment variable where a fixed value is needed"),
        }
    }
    Ok(out)
}

fn strings(env: &Env, e: &Expr) -> Result<Vec<String>, Why> {
    match e {
        Expr::Array(items) => items.iter().map(|i| plain(env, i)).collect(),
        other => Ok(vec![plain(env, other)?]),
    }
}

/// `e`, or the regex constant it names.
fn regex_of(env: &Env, e: &Expr) -> Expr {
    match literal(env, e) {
        Some(re @ Expr::Regex { .. }) => re,
        _ => e.clone(),
    }
}

/// A text or regex for expect: `/re/flags` stays a regex.
fn text_word(env: &Env, e: &Expr) -> Result<Word, Why> {
    match &regex_of(env, e) {
        Expr::Regex { pattern, flags } => Ok(Word::lit(format!("/{pattern}/{flags}"))),
        other => {
            let w = word(env, other)?;
            // A text that looks like a regex would be read as one.
            if let Some(Part::Lit(first)) = w.0.first()
                && first.starts_with('/')
                && w.0
                    .iter()
                    .any(|p| matches!(p, Part::Lit(t) if t.len() > 1 && t.ends_with('/')))
            {
                return why("a text that reads as a /regex/");
            }
            Ok(w)
        }
    }
}

fn text_value(env: &Env, e: &Expr) -> Result<String, Why> {
    match &regex_of(env, e) {
        Expr::Regex { pattern, flags } => Ok(format!("/{pattern}/{flags}")),
        other => plain(env, other),
    }
}

/// A URL pattern: a glob or part, or a regex.
fn pattern(env: &Env, e: &Expr) -> Result<String, Why> {
    match &regex_of(env, e) {
        Expr::Regex { pattern, flags } => Ok(format!("/{pattern}/{flags}")),
        Expr::Func(_) => why("a predicate function: wait --response takes a URL pattern"),
        other => plain(env, other),
    }
}

/// A number, maybe from a constant: `ITEMS.length`, a helper's `i`.
fn known_number(env: &Env, e: &Expr) -> Result<String, Why> {
    number(&literal(env, e).unwrap_or_else(|| e.clone()))
}

fn number(e: &Expr) -> Result<String, Why> {
    match e {
        Expr::Num(n) => Ok(n.clone()),
        Expr::Unary("-", inner) => Ok(format!("-{}", number(inner)?)),
        _ => why("a number computed at run time"),
    }
}

/// Milliseconds as a duration flag value: `5000` → `5s`.
fn duration(e: &Expr) -> Result<String, Why> {
    let n = number(e)?;
    let Ok(ms) = n.parse::<u64>() else {
        return why(format!("a time of {n}ms"));
    };
    Ok(if ms > 0 && ms % 1000 == 0 {
        format!("{}s", ms / 1000)
    } else {
        format!("{ms}ms")
    })
}

fn point(e: &Expr) -> Result<String, Why> {
    let props = object(e)?;
    Ok(format!(
        "{},{}",
        number(prop(props, "x")?)?,
        number(prop(props, "y")?)?
    ))
}

fn size(e: &Expr) -> Result<String, Why> {
    let props = object(e)?;
    Ok(format!(
        "{}x{}",
        number(prop(props, "width")?)?,
        number(prop(props, "height")?)?
    ))
}

fn object(e: &Expr) -> Result<&[Prop], Why> {
    match e {
        Expr::Object(props) => Ok(props),
        _ => why("an options object computed at run time"),
    }
}

fn prop<'e>(props: &'e [Prop], key: &str) -> Result<&'e Expr, Why> {
    props
        .iter()
        .find_map(|p| match p {
            Prop::Pair(k, v) if k == key => Some(v),
            _ => None,
        })
        .ok_or_else(|| Why(format!("no {key}")))
}

/// The options object's pairs, when every key is one of `allowed`.
fn options<'e>(e: Option<&'e Expr>, allowed: &[&str]) -> Result<Vec<(&'e str, &'e Expr)>, Why> {
    let Some(e) = e else { return Ok(vec![]) };
    let mut out = Vec::new();
    for p in object(e)? {
        match p {
            Prop::Pair(k, v) if allowed.contains(&k.as_str()) => out.push((k.as_str(), v)),
            Prop::Pair(k, _) => return why(format!("the {k} option")),
            _ => return why("options computed at run time"),
        }
    }
    Ok(out)
}

/// `page.pdf`'s `margin: { top, right, bottom, left }` as `--margin`'s
/// four lengths: a number is pixels, a side not given is 0.
fn pdf_margin(v: &Expr) -> Result<String, Why> {
    let props = object(v)?;
    let mut sides = Vec::new();
    for side in ["top", "right", "bottom", "left"] {
        let length = match props.iter().find_map(|p| match p {
            Prop::Pair(k, v) if k == side => Some(v),
            _ => None,
        }) {
            None => "0".to_owned(),
            Some(Expr::Str(s)) => s.trim().to_owned(),
            Some(n @ Expr::Num(_)) => format!("{}px", number(n)?),
            Some(_) => return why("a margin computed at run time"),
        };
        sides.push(length);
    }
    if props
        .iter()
        .any(|p| !matches!(p, Prop::Pair(k, _) if ["top", "right", "bottom", "left"].contains(&k.as_str())))
    {
        return why("a margin other than top, right, bottom and left");
    }
    Ok(sides.join(","))
}

fn bool_switch(c: Cmd, v: &Expr, flag: &str) -> Result<Cmd, Why> {
    Ok(if boolean(v, flag)? {
        c.arg(flag.to_owned())
    } else {
        c
    })
}

fn boolean(v: &Expr, what: &str) -> Result<bool, Why> {
    match v {
        Expr::Name(b) if b == "true" => Ok(true),
        Expr::Name(b) if b == "false" => Ok(false),
        _ => why(format!("{what} computed at run time")),
    }
}

fn timeout_only(c: Cmd, opts: Option<&Expr>) -> Result<Cmd, Why> {
    let mut c = c;
    for (_, v) in options(opts, &["timeout"])? {
        c = c.flag("--timeout", duration(v)?);
    }
    Ok(c)
}

fn timeout_force(c: Cmd, opts: Option<&Expr>) -> Result<Cmd, Why> {
    let mut c = c;
    for (k, v) in options(opts, &["timeout", "force", "noWaitAfter"])? {
        c = match k {
            "timeout" => c.flag("--timeout", duration(v)?),
            "force" => bool_switch(c, v, "--force")?,
            _ => bool_switch(c, v, "--no-wait-after")?,
        };
    }
    Ok(c)
}

fn screenshot_options(env: &Env, c: Cmd, opts: Option<&Expr>) -> Result<Cmd, Why> {
    let mut c = c;
    for (k, v) in options(
        opts,
        &[
            "path",
            "fullPage",
            "type",
            "quality",
            "omitBackground",
            "scale",
            "timeout",
            "animations",
            "caret",
        ],
    )? {
        c = match k {
            "path" => c.flag("-o", word(env, v)?),
            "fullPage" => bool_switch(c, v, "--full")?,
            "type" => c.flag("--format", plain(env, v)?),
            "quality" => c.flag("--quality", number(v)?),
            "omitBackground" => bool_switch(c, v, "--omit-background")?,
            "scale" => c.flag("--scale", plain(env, v)?),
            "timeout" => c.flag("--timeout", duration(v)?),
            _ => c,
        };
    }
    Ok(c)
}

/// `route(url, route => route.fulfill({ … }))` and `abort`.
fn route(env: &Env, args: &[Expr]) -> Out {
    let url = pattern(env, need(args.first(), "a URL")?)?;
    let Some(Expr::Func(handler)) = args.get(1) else {
        return why("a route handler that is not a function");
    };
    let call = match &handler.body {
        Body::Expr(e) => Some((**e).clone()),
        Body::Block(stmts) => match stmts.as_slice() {
            [
                Stmt {
                    kind: StmtKind::Expr(e),
                    ..
                },
            ] => Some(e.clone()),
            _ => None,
        },
    };
    let Some(Expr::Call(callee, call_args)) = call.as_ref().map(Expr::awaited) else {
        return why("a route handler with logic: split it into route rules (--method, --times)");
    };
    let Expr::Member(_, action) = &**callee else {
        return why("a route handler with logic");
    };
    let mut c = cmd(["route"]).arg(url);
    match action.as_str() {
        "fulfill" => {
            for (k, v) in options(
                call_args.first(),
                &["status", "json", "body", "contentType", "headers", "path"],
            )? {
                c = match k {
                    "status" => c.flag("--status", number(v)?),
                    "json" => c.flag("--json-body", json(env, v)?),
                    "body" => c.flag("--body", word(env, v)?),
                    "contentType" => {
                        c.flag("--header", format!("Content-Type: {}", plain(env, v)?))
                    }
                    "headers" => c.words(headers(env, v, "--header")?),
                    _ => c.flag("--body-file", word(env, v)?),
                };
            }
        }
        "abort" => {
            c = c.arg("--abort".to_owned());
            if let Some(reason) = call_args.first() {
                c = c.arg(plain(env, reason)?);
            }
        }
        "continue" | "fallback" if call_args.is_empty() => {
            return why("a route that sends requests on unchanged: nothing to do");
        }
        "continue" => {
            for (k, v) in options(call_args.first(), &["headers", "url"])? {
                c = match k {
                    "headers" => c.words(headers(env, v, "--set-header")?),
                    _ => c.flag("--rewrite-url", word(env, v)?),
                };
            }
        }
        other => return why(format!("route.{other}")),
    }
    for (_, v) in options(args.get(2), &["times"])? {
        c = c.flag("--times", number(v)?);
    }
    Ok(vec![c.line()])
}

/// `{ Name: 'value' }` as `FLAG 'Name: value'` pairs.
fn headers(env: &Env, e: &Expr, flag: &str) -> Result<Vec<Word>, Why> {
    let mut out = Vec::new();
    for p in object(e)? {
        let Prop::Pair(k, v) = p else {
            return why("headers computed at run time");
        };
        let mut w = Word::lit(format!("{k}: "));
        w.0.extend(word(env, v)?.0);
        out.push(Word::lit(flag));
        out.push(w);
    }
    Ok(out)
}

/// A literal as JSON text.
fn json(env: &Env, e: &Expr) -> Result<String, Why> {
    fn value(env: &Env, e: &Expr) -> Result<serde_json::Value, Why> {
        use serde_json::Value;
        Ok(match e {
            Expr::Str(_) | Expr::Template(_) => Value::String(plain(env, e)?),
            Expr::Num(n) => serde_json::from_str(n).map_err(|_| Why(format!("the number {n}")))?,
            Expr::Unary("-", inner) => {
                let n = number(inner)?;
                serde_json::from_str(&format!("-{n}"))
                    .map_err(|_| Why(format!("the number -{n}")))?
            }
            Expr::Name(n) if n == "true" => Value::Bool(true),
            Expr::Name(n) if n == "false" => Value::Bool(false),
            Expr::Name(n) if n == "null" => Value::Null,
            Expr::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|i| value(env, i))
                    .collect::<Result<_, _>>()?,
            ),
            Expr::Object(props) => {
                let mut map = serde_json::Map::new();
                for p in props {
                    let Prop::Pair(k, v) = p else {
                        return why("JSON computed at run time");
                    };
                    map.insert(k.clone(), value(env, v)?);
                }
                Value::Object(map)
            }
            other => match literal(env, other) {
                Some(known) if !matches!(known, Expr::Regex { .. }) => value(env, &known)?,
                _ => return why("JSON computed at run time: put it in a file for --body-file"),
            },
        })
    }
    Ok(value(env, e)?.to_string())
}

fn init_script(env: &Env, e: &Expr) -> Out {
    match e {
        Expr::Object(props) => {
            let path = word(env, prop(props, "path")?)?;
            Ok(vec![cmd(["init-script", "add"]).word(path).line()])
        }
        other => {
            let js = script_source(env, other, true)?;
            Ok(vec![cmd(["init-script", "add", "--code"]).arg(js).line()])
        }
    }
}

/// A function called with a value known now, as JavaScript: `(fn)(3)`,
/// or `el => (fn)(el, 3)` with the element first.
fn called(env: &Env, what: &str, f: &Expr, el: Option<&str>, value: &Expr) -> Result<String, Why> {
    let (Expr::Func(f), Ok(value)) = (f, json(env, value)) else {
        return why(format!(
            "{what} with an argument: put the value into the script"
        ));
    };
    let source = js_source(f)?;
    Ok(match el {
        Some(el) => format!("{el} => ({source})({el}, {value})"),
        None => format!("({source})({value})"),
    })
}

/// JavaScript to run: a string as it is, or a function's source (called
/// when `call` is set, for commands that take an expression).
fn script_source(env: &Env, e: &Expr, call: bool) -> Result<String, Why> {
    match e {
        Expr::Func(f) if call => Ok(format!("({})()", js_source(f)?)),
        Expr::Func(f) => js_source(f),
        other => plain(env, other),
    }
}

/// A function's source to run in the page, which must be JavaScript.
fn js_source(f: &Func) -> Result<String, Why> {
    if super::parse::has_types(&f.source) {
        return why(
            "a function with TypeScript types convert could not remove: write it in JavaScript",
        );
    }
    Ok(f.source.clone())
}

fn keyboard(env: &Env, method: &str, args: &[Expr]) -> Out {
    let text = || -> Result<String, Why> { plain(env, need(args.first(), "a key")?) };
    match method {
        "press" => Ok(vec![cmd(["press"]).arg(keys(&text()?)).line()]),
        "type" | "insertText" => {
            let w = word(env, need(args.first(), "a text")?)?;
            let mut c = cmd(["type"]).word(w);
            for (_, v) in options(args.get(1), &["delay"])? {
                c = c.flag("--delay", duration(v)?);
            }
            Ok(vec![c.line()])
        }
        "down" | "up" => Ok(vec![cmd(["keyboard", method]).arg(keys(&text()?)).line()]),
        other => why(format!("keyboard.{other}")),
    }
}

fn mouse(method: &str, args: &[Expr]) -> Out {
    let xy = || -> Result<(String, String), Why> {
        Ok((
            number(need(args.first(), "x")?)?,
            number(need(args.get(1), "y")?)?,
        ))
    };
    match method {
        "move" => {
            let (x, y) = xy()?;
            let mut c = cmd(["mouse", "move"]).arg(x).arg(y);
            for (_, v) in options(args.get(2), &["steps"])? {
                c = c.flag("--steps", number(v)?);
            }
            Ok(vec![c.line()])
        }
        "down" | "up" => {
            let mut c = cmd(["mouse", method]);
            for (k, v) in options(args.first(), &["button"])? {
                let _ = k;
                c = c.flag(
                    "--button",
                    match v {
                        Expr::Str(s) => s.clone(),
                        _ => return why("a button computed at run time"),
                    },
                );
            }
            Ok(vec![c.line()])
        }
        "click" => {
            let (x, y) = xy()?;
            if args.len() > 2 {
                return why("mouse.click options");
            }
            Ok(vec![
                cmd(["mouse", "down"]).arg(x).arg(y).line(),
                cmd(["mouse", "up"]).line(),
            ])
        }
        "wheel" => {
            let (x, y) = xy()?;
            Ok(vec![cmd(["mouse", "wheel"]).arg(x).arg(y).line()])
        }
        other => why(format!("mouse.{other}")),
    }
}

fn clock(env: &Env, method: &str, args: &[Expr]) -> Out {
    match method {
        "setFixedTime" | "install" | "setSystemTime" => {
            let time = match args.first() {
                Some(Expr::Object(props)) if method == "install" => prop(props, "time")?,
                Some(e) => e,
                None => return why("clock.install without a time: time freeze <ISO time>"),
            };
            let t = clock_time(env, time)?;
            Ok(vec![cmd(["time", "freeze"]).arg(t).line()])
        }
        "fastForward" => Ok(vec![
            cmd(["time", "advance"])
                .arg(clock_duration(env, args)?)
                .line(),
        ]),
        "runFor" => Ok(vec![
            cmd(["time", "run-for"])
                .arg(clock_duration(env, args)?)
                .line(),
        ]),
        // Playwright's pauseAt jumps there from a running clock too; ours
        // runs a paused one there, so `time pause` is the safe translation.
        "pauseAt" => {
            let t = clock_time(env, need(args.first(), "a time")?)?;
            Ok(vec![cmd(["time", "pause"]).arg(t).line()])
        }
        "resume" => Ok(vec![cmd(["time", "resume"]).line()]),
        other => why(format!(
            "clock.{other}: only time freeze, advance, pause, run-for and resume"
        )),
    }
}

/// A clock method's time: a string, or `new Date(…)` of one.
fn clock_time(env: &Env, time: &Expr) -> Result<String, Why> {
    match time {
        Expr::Call(callee, a) if callee.path().as_deref() == Some("Date") => {
            plain(env, need(a.first(), "a date")?)
        }
        other => plain(env, other).map_err(|_| Why("a date computed at run time".into())),
    }
}

/// `fastForward`'s and `runFor`'s duration: ms, or `mm:ss`/`hh:mm:ss`.
fn clock_duration(env: &Env, args: &[Expr]) -> Result<String, Why> {
    match need(args.first(), "a time")? {
        Expr::Num(_) => duration(&args[0]),
        other => {
            let s = plain(env, other)?;
            let parts: Vec<u64> = s.split(':').filter_map(|p| p.parse().ok()).collect();
            let secs = parts.iter().fold(0, |acc, p| acc * 60 + p);
            if parts.len() < 2 {
                return Err(Why(format!("the time {s}")));
            }
            Ok(format!("{secs}s"))
        }
    }
}

/// Playwright's key names are the same, but for `ControlOrMeta`.
fn keys(k: &str) -> String {
    k.replace("ControlOrMeta", "Mod")
}

fn option_value(env: &Env, e: &Expr) -> Result<String, Why> {
    match e {
        Expr::Object(props) => {
            for key in ["value", "label"] {
                if let Ok(v) = prop(props, key) {
                    return plain(env, v);
                }
            }
            why("selectOption by index: select by value or label")
        }
        other => plain(env, other),
    }
}

fn geolocation(e: &Expr) -> Result<Line, Why> {
    let props = object(e)?;
    let mut c = cmd(["geo"]).arg(format!(
        "{},{}",
        number(prop(props, "latitude")?)?,
        number(prop(props, "longitude")?)?
    ));
    if let Ok(acc) = prop(props, "accuracy") {
        c = c.flag("--accuracy", number(acc)?);
    }
    Ok(c.line())
}

fn viewport(value: &Expr, all: &HashMap<&str, &Expr>) -> Result<Line, Why> {
    let mut c = cmd(["viewport"]).arg(size(value)?);
    if let Some(dpr) = all.get("deviceScaleFactor") {
        c = c.flag("--dpr", number(dpr)?);
    }
    if let Some(v) = all.get("isMobile") {
        c = bool_switch(c, v, "--mobile")?;
    }
    if let Some(v) = all.get("hasTouch") {
        c = bool_switch(c, v, "--touch")?;
    }
    Ok(c.line())
}

/// `...devices['iPhone 15']`: the device's name.
fn device(e: &Expr) -> Option<String> {
    match e {
        Expr::Index(object, key) if object.path().as_deref() == Some("devices") => match &**key {
            Expr::Str(name) => Some(name.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// `page.getBy…(…)` and `page.locator(…)` steps.
fn base_locator(env: &Env, method: &str, args: &[Expr]) -> Result<String, Why> {
    let first = || need(args.first(), "a value");
    let opts = args.get(1);
    let text = |prefix: &str| -> Result<String, Why> {
        let exact = options(opts, &["exact"])?
            .iter()
            .any(|(_, v)| matches!(v, Expr::Name(b) if b == "true"));
        Ok(format!("{prefix}={}", locator_text(env, first()?, exact)?))
    };
    match method {
        "getByText" => text("text"),
        "getByLabel" => text("label"),
        "getByPlaceholder" => text("placeholder"),
        "getByAltText" => text("alt"),
        "getByTitle" => text("title"),
        "getByTestId" => match first()? {
            Expr::Regex { .. } => why("getByTestId with a regex"),
            e => Ok(format!("testid={}", plain(env, e)?)),
        },
        "getByRole" => role(env, first()?, opts),
        "locator" => {
            let mut out = selector(&plain(env, first()?)?)?;
            for (k, v) in options(opts, &["hasText"])? {
                let _ = k;
                out = format!("{out} >> has-text={}", locator_text(env, v, false)?);
            }
            Ok(out)
        }
        "frameLocator" => frame_of(&selector(&plain(env, first()?)?)?),
        other => why(format!("{other}(), not a locator")),
    }
}

fn role(env: &Env, name: &Expr, opts: Option<&Expr>) -> Result<String, Why> {
    let mut out = format!("role={}", plain(env, name)?);
    let pairs = options(
        opts,
        &[
            "name",
            "exact",
            "checked",
            "disabled",
            "expanded",
            "pressed",
            "selected",
            "level",
            "includeHidden",
        ],
    )?;
    let exact = pairs
        .iter()
        .any(|(k, v)| *k == "exact" && matches!(v, Expr::Name(b) if b == "true"));
    for (k, v) in pairs {
        match k {
            "exact" => {}
            "name" => {
                let value = locator_text(env, v, exact)?;
                if value.contains(']') && !value.starts_with('"') {
                    return why("a name with ]");
                }
                let _ = write!(out, "[name={value}]");
            }
            "level" => {
                let _ = write!(out, "[level={}]", number(v)?);
            }
            "includeHidden" => {
                let _ = write!(out, "[include-hidden={}]", flag_value(v)?);
            }
            other => {
                let _ = write!(out, "[{other}={}]", flag_value(v)?);
            }
        }
    }
    Ok(out)
}

fn flag_value(v: &Expr) -> Result<&str, Why> {
    match v {
        Expr::Name(b) if b == "true" || b == "false" => Ok(b),
        Expr::Str(s) if s == "mixed" => Ok("mixed"),
        _ => why("a state computed at run time"),
    }
}

/// A text value inside a locator: a part (as Playwright's default), the
/// exact text quoted, or a regex.
fn locator_text(env: &Env, e: &Expr, exact: bool) -> Result<String, Why> {
    if let Expr::Regex { pattern, flags } = &regex_of(env, e) {
        return Ok(format!("/{pattern}/{flags}"));
    }
    let text = plain(env, e)?;
    if exact {
        return Ok(format!(
            "\"{}\"",
            text.replace('\\', "\\\\").replace('"', "\\\"")
        ));
    }
    if text.trim().is_empty()
        || text.contains(">>")
        || text.starts_with(['"', '\'', '/'])
        || text.trim() != text
    {
        return why(format!("the text {text:?} needs care in a locator"));
    }
    Ok(text)
}

/// `.filter({ hasText, hasNotText, has, hasNot, visible })`.
fn filter(env: &Env, before: &str, opts: Option<&Expr>) -> Result<String, Why> {
    let mut out = before.to_owned();
    for (k, v) in options(opts, &["hasText", "visible", "has", "hasNot", "hasNotText"])? {
        match k {
            "hasText" => {
                let _ = write!(out, " >> has-text={}", locator_text(env, v, false)?);
            }
            "hasNotText" => {
                let _ = write!(out, " >> has-not-text={}", locator_text(env, v, false)?);
            }
            "visible" => {
                let _ = write!(out, " >> visible={}", flag_value(v)?);
            }
            _ => {
                // Playwright searches `has` inside each element, as has= does.
                let inner = Converter::locator(env, v)?;
                let step = if k == "has" { "has" } else { "has-not" };
                let _ = write!(
                    out,
                    " >> {step}={}",
                    inner_value(&format!("filter({{ {k} }})"), &inner)?
                );
            }
        }
    }
    Ok(out)
}

/// `a.or(b)`, `a.and(b)`: `A >> or="B"`. `or=` and `and=` search where a's
/// last finder step searched, so b must be built from the same page or
/// the same parent locator; B is b without that parent.
fn combine(env: &Env, before: &str, method: &str, other: Option<&Expr>) -> Result<String, Why> {
    let other = Converter::locator(env, need(other, "a locator")?)?;
    let steps = |text: &str| {
        engine::parse_locator(text)
            .map(|l| l.typed_steps())
            .map_err(|e| Why(e.message))
    };
    let (a, b) = (steps(before)?, steps(&other)?);
    let parent = &a[..a.iter().rposition(|(_, filter)| !filter).unwrap_or(0)];
    let rest = b
        .strip_prefix(parent)
        .filter(|r| r.first().is_some_and(|(_, f)| !f));
    let Some(rest) = rest else {
        return why(format!(
            "locator.{method}() of a locator from another parent: build both from the same page or locator"
        ));
    };
    let inner: Vec<&str> = rest.iter().map(|(part, _)| part.as_str()).collect();
    Ok(format!(
        "{before} >> {method}={}",
        inner_value(&format!("{method}()"), &inner.join(" >> "))?
    ))
}

/// An inner locator as a step's value: quoted when it has a `>>` of its
/// own or would not parse bare. `what` names the Playwright call.
fn inner_value(what: &str, inner: &str) -> Result<String, Why> {
    if inner.starts_with("frame=") {
        return why(format!("{what} with a frame locator inside"));
    }
    if inner.contains(">>") || inner.starts_with(['"', '\'']) || inner.trim() != inner {
        return Ok(format!(
            "\"{}\"",
            inner.replace('\\', "\\\\").replace('"', "\\\"")
        ));
    }
    Ok(inner.to_owned())
}

/// The `frame=` step of an iframe selector: its id or name.
fn frame_of(selector: &str) -> Result<String, Why> {
    let s = selector.strip_prefix("css=").unwrap_or(selector);
    let s = s.strip_prefix("iframe").unwrap_or(s);
    if let Some(id) = s.strip_prefix('#')
        && id
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Ok(format!("frame={id}"));
    }
    for attr in ["[name=", "[title="] {
        if let Some(rest) = s.strip_prefix(attr).and_then(|r| r.strip_suffix(']'))
            && attr == "[name="
        {
            return Ok(format!("frame={}", rest.trim_matches(['"', '\''])));
        }
    }
    why(format!(
        "the frame {selector}: frame= takes the frame's name, id or a part of its URL"
    ))
}

/// A Playwright selector string as a chrome-cdp locator.
fn selector(sel: &str) -> Result<String, Why> {
    let sel = sel.trim();
    if sel.contains(" >> ") {
        let parts: Vec<String> = sel.split(" >> ").map(selector).collect::<Result<_, _>>()?;
        return Ok(parts.join(" >> "));
    }
    for engine in ["css=", "xpath=", "text=", "role=", "nth=", "visible="] {
        if sel.starts_with(engine) {
            return Ok(sel.to_owned());
        }
    }
    if let Some(id) = sel.strip_prefix("data-testid=") {
        return Ok(format!("testid={}", id.trim_matches(['"', '\''])));
    }
    if sel.starts_with("//") || sel.starts_with("..") {
        return Ok(sel.to_owned());
    }
    if sel.starts_with("internal:")
        || sel.starts_with("id=")
        || sel.starts_with("_react=")
        || sel.starts_with("_vue=")
    {
        return why(format!("the {sel} selector"));
    }
    if sel.starts_with('"') || sel.starts_with('\'') {
        return Ok(sel.to_owned());
    }
    for ext in [
        ":has-text(",
        ":text(",
        ":text-is(",
        ":text-matches(",
        ":visible",
        ":nth-match(",
        ":left-of(",
        ":right-of(",
        ":above(",
        ":below(",
        ":near(",
    ] {
        if sel.contains(ext) {
            return why(format!(
                "Playwright's {ext} CSS extension: use has-text=, text=, visible=true or nth="
            ));
        }
    }
    if sel.starts_with(['#', '.', '[']) {
        Ok(sel.to_owned())
    } else {
        Ok(format!("css={sel}"))
    }
}

/// A JavaScript string literal.
fn js_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("'{s}'"))
}

/// `x = …`, `a.b[0] += …`: a statement that assigns.
fn is_assignment(code: &str) -> bool {
    let target = code
        .find(|c: char| !(c.is_alphanumeric() || "_$.[]'\"".contains(c)))
        .map_or(code, |i| &code[..i]);
    let rest = code[target.len()..].trim_start();
    !target.is_empty()
        && ["=", "+=", "-=", "*=", "/=", "??=", "||=", "&&="]
            .iter()
            .any(|op| rest.starts_with(op) && !rest[op.len()..].starts_with(['=', '>']))
}

/// Calls that wait for something to happen after an action.
fn is_wait(e: &Expr) -> bool {
    let Expr::Call(callee, _) = e.awaited() else {
        return false;
    };
    matches!(&**callee, Expr::Member(_, m) if m.starts_with("waitFor"))
}

fn is_require(e: &Expr) -> bool {
    match e.awaited() {
        Expr::Call(callee, _) => callee.path().as_deref() == Some("require"),
        Expr::Member(inner, _) => is_require(inner),
        _ => false,
    }
}

/// The read command for a value-returning locator method.
fn reader(method: &str) -> &'static str {
    match method {
        "inputValue" => "value LOC",
        "getAttribute" => "attr LOC NAME",
        "count" => "count LOC",
        "boundingBox" => "box LOC",
        "innerHTML" => "html LOC",
        "isVisible" | "isHidden" | "isEnabled" | "isChecked" => {
            "expect LOC visible|enabled|checked"
        }
        _ => "text LOC",
    }
}

fn reader_hint(e: &Expr) -> &'static str {
    match e.awaited() {
        Expr::Call(callee, _) => match &**callee {
            Expr::Member(_, m) => reader(m),
            _ => "text LOC",
        },
        _ => "text LOC",
    }
}

/// A test title as a file name: `Shows the cart!` → `shows-the-cart`.
fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

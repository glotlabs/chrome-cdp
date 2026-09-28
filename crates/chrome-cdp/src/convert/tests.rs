//! `convert`: Playwright statements → `.cdp` lines. Every generated line
//! is also checked the way `run` checks a script, so what convert writes
//! always parses.

use std::fmt::Write as _;

use super::convert;
use crate::script;

/// The lines `code` becomes inside a test, without the header and the
/// `# test:` comment.
fn body(code: &str) -> String {
    let src = format!("test('t', async ({{ page, context }}) => {{\n{code}\n}});\n");
    let script = convert("t.spec.ts", &src).script;
    let lines: Vec<&str> = script
        .lines()
        .skip_while(|l| !l.starts_with("# test: t"))
        .skip(1)
        .filter(|l| !l.is_empty())
        .collect();
    lines.join("\n")
}

/// Each case: Playwright code and the lines it becomes.
const CASES: &[(&str, &str)] = &[
    // Navigation and waiting.
    (
        "await page.goto('https://example.com/');",
        "goto https://example.com/",
    ),
    (
        "await page.goto('/login', { waitUntil: 'domcontentloaded', timeout: 60000 });",
        "goto /login --wait domcontentloaded --timeout 60s",
    ),
    (
        "await page.reload(); await page.goBack(); await page.goForward();",
        "reload\nback\nforward",
    ),
    (
        "await page.waitForURL('**/dashboard');",
        "wait --url '**/dashboard'",
    ),
    (
        "await page.waitForURL(/\\/done$/);",
        "wait --url '/\\/done$/'",
    ),
    ("await page.waitForLoadState('networkidle');", "wait --idle"),
    ("await page.waitForLoadState();", "wait --load"),
    (
        "await page.waitForSelector('.spinner', { state: 'hidden' });",
        "wait .spinner --state hidden",
    ),
    ("await page.waitForTimeout(500);", "wait 500ms"),
    (
        "await page.waitForFunction(() => window.ready === true);",
        "wait --fn '(() => window.ready === true)()'",
    ),
    (
        "await page.waitForFunction('window.ready');",
        "wait --fn window.ready",
    ),
    // Locators.
    (
        "await page.getByRole('button', { name: 'Save', exact: true }).click();",
        "click 'role=button[name=\"Save\"]'",
    ),
    (
        "await page.getByRole('heading', { level: 2, name: /intro/i }).click();",
        "click 'role=heading[level=2][name=/intro/i]'",
    ),
    (
        "await page.getByRole('checkbox', { checked: true, includeHidden: true }).click();",
        "click 'role=checkbox[checked=true][include-hidden=true]'",
    ),
    (
        "await page.getByText('Welcome').click();",
        "click text=Welcome",
    ),
    (
        "await page.getByText('Sign in', { exact: true }).click();",
        "click 'text=\"Sign in\"'",
    ),
    (
        "await page.getByPlaceholder('Search').fill('rust');",
        "fill placeholder=Search rust",
    ),
    ("await page.getByAltText('Logo').click();", "click alt=Logo"),
    ("await page.getByTitle('Help').hover();", "hover title=Help"),
    (
        "await page.getByTestId('submit').click();",
        "click testid=submit",
    ),
    ("await page.locator('#go').click();", "click '#go'"),
    (
        "await page.locator('form button').click();",
        "click 'css=form button'",
    ),
    ("await page.locator('//a[1]').click();", "click '//a[1]'"),
    (
        "await page.locator('.row').filter({ hasText: 'Total' }).first().click();",
        "click '.row >> has-text=Total >> nth=0'",
    ),
    (
        "await page.locator('li').nth(2).getByRole('button').click();",
        "click 'css=li >> nth=2 >> role=button'",
    ),
    (
        "await page.locator('li').last().click();",
        "click 'css=li >> nth=-1'",
    ),
    (
        "await page.frameLocator('#payment').getByLabel('Card').fill('4242');",
        "fill 'frame=payment >> label=Card' 4242",
    ),
    (
        "await page.locator('iframe[name=\"pay\"]').contentFrame().locator('#card').fill('1');",
        "fill 'frame=pay >> #card' 1",
    ),
    (
        "const row = page.locator('.row');\nawait row.getByRole('button', { name: 'Delete' }).click();",
        "click '.row >> role=button[name=Delete]'",
    ),
    // Actions.
    (
        "await page.locator('#a').click({ button: 'right', modifiers: ['Shift', 'ControlOrMeta'], position: { x: 10, y: 20 } });",
        "click '#a' --button right --modifiers Shift,Mod --position 10,20",
    ),
    (
        "await page.locator('#a').click({ clickCount: 3, force: true, noWaitAfter: true });",
        "click '#a' --count 3 --force --no-wait-after",
    ),
    ("await page.locator('#a').dblclick();", "dblclick '#a'"),
    ("await page.locator('#a').tap();", "tap '#a'"),
    (
        "await page.locator('#a').focus(); await page.locator('#a').blur();",
        "focus '#a'\nblur '#a'",
    ),
    (
        "await page.getByLabel('Remember').check();",
        "check label=Remember",
    ),
    (
        "await page.getByLabel('Remember').setChecked(false);",
        "uncheck label=Remember",
    ),
    ("await page.locator('#q').clear();", "fill '#q' ''"),
    (
        "await page.locator('#q').pressSequentially('rust', { delay: 50 });",
        "type rust --in '#q' --delay 50ms",
    ),
    (
        "await page.locator('#q').press('ControlOrMeta+A');",
        "press Mod+A --in '#q'",
    ),
    (
        "await page.locator('#c').selectOption('blue');",
        "select '#c' blue",
    ),
    (
        "await page.locator('#c').selectOption(['a', { label: 'Bee' }]);",
        "select '#c' a Bee",
    ),
    (
        "await page.locator('#f').setInputFiles('a.png');",
        "upload '#f' a.png",
    ),
    (
        "await page.locator('#f').setInputFiles(['a.png', 'b.pdf']);",
        "upload '#f' a.png b.pdf",
    ),
    (
        "await page.locator('#f').setInputFiles([]);",
        "upload '#f' --clear",
    ),
    (
        "await page.locator('#a').dragTo(page.locator('#b'));",
        "drag '#a' '#b'",
    ),
    (
        "await page.locator('#a').scrollIntoViewIfNeeded();",
        "scroll '#a'",
    ),
    (
        "await page.locator('#a').screenshot({ path: 'a.png' });",
        "screenshot '#a' -o a.png",
    ),
    (
        "await page.locator('#a').waitFor({ state: 'detached' });",
        "wait '#a' --state detached",
    ),
    (
        "await page.locator('#a').evaluate(el => el.value);",
        "eval --in '#a' 'el => el.value'",
    ),
    ("await page.click('#go');", "click '#go'"),
    ("await page.fill('#q', 'x');", "fill '#q' x"),
    // Keyboard and mouse.
    ("await page.keyboard.press('Enter');", "press Enter"),
    (
        "await page.keyboard.type('hello world');",
        "type 'hello world'",
    ),
    (
        "await page.mouse.move(100, 200, { steps: 5 });",
        "mouse move 100 200 --steps 5",
    ),
    (
        "await page.mouse.click(10, 20);",
        "mouse down 10 20\nmouse up",
    ),
    ("await page.mouse.wheel(0, 600);", "mouse wheel 0 600"),
    // Pages, screenshots, evaluation.
    (
        "await page.screenshot({ path: 'shot.png', fullPage: true });",
        "screenshot -o shot.png --full",
    ),
    (
        "await page.pdf({ path: 'a.pdf', format: 'A4' });",
        "pdf -o a.pdf --format A4",
    ),
    (
        "await page.evaluate(() => localStorage.clear());",
        "eval '() => localStorage.clear()'",
    ),
    (
        "await page.setViewportSize({ width: 390, height: 844 });",
        "viewport 390x844",
    ),
    (
        "await page.emulateMedia({ colorScheme: 'dark' });",
        "media --color-scheme dark",
    ),
    (
        "await page.addInitScript({ path: './stub.js' });",
        "init-script add ./stub.js",
    ),
    (
        "await page.addInitScript(() => { window.FLAG = true; });",
        "init-script add --code '(() => { window.FLAG = true; })()'",
    ),
    (
        "await page.exposeFunction('onDone', () => {});",
        "expose onDone",
    ),
    ("await page.close();", "tab close"),
    // Mocking.
    (
        "await page.route('**/api/items', route => route.fulfill({ status: 201, json: [{ id: 1, ok: true }] }));",
        "route '**/api/items' --status 201 --json-body '[{\"id\":1,\"ok\":true}]'",
    ),
    (
        "await page.route('**/x', async (route) => { await route.fulfill({ body: 'hi', contentType: 'text/html' }); });",
        "route '**/x' --body hi --header 'Content-Type: text/html'",
    ),
    (
        "await page.route(/ads/, r => r.abort());",
        "route /ads/ --abort",
    ),
    (
        "await page.route('**/api/**', r => r.continue({ headers: { Authorization: 'Bearer t' } }));",
        "route '**/api/**' --set-header 'Authorization: Bearer t'",
    ),
    (
        "await page.route('**/a', r => r.fulfill({ status: 500 }), { times: 1 });",
        "route '**/a' --status 500 --times 1",
    ),
    ("await page.unrouteAll();", "unroute --all"),
    (
        "await page.setExtraHTTPHeaders({ 'X-Test': '1' });",
        "route '**' --set-header 'X-Test: 1'",
    ),
    // Context.
    (
        "await context.grantPermissions(['geolocation'], { origin: 'https://a.test' });",
        "permissions grant geolocation --origin https://a.test",
    ),
    ("await page.context().setOffline(true);", "offline on"),
    (
        "await context.storageState({ path: 'auth.json' });",
        "state save auth.json",
    ),
    ("await context.clearCookies();", "cookies clear"),
    (
        "await context.setGeolocation({ latitude: 59.9, longitude: 10.7 });",
        "geo 59.9,10.7",
    ),
    (
        "await page.clock.setFixedTime(new Date('2026-01-31T09:30:00Z'));",
        "time freeze 2026-01-31T09:30:00Z",
    ),
    (
        "await page.clock.setFixedTime('2026-01-31T09:30:00Z');",
        "time freeze 2026-01-31T09:30:00Z",
    ),
    ("await page.clock.fastForward('01:30');", "time advance 90s"),
    // Assertions.
    (
        "await expect(page.locator('#a')).toBeVisible();",
        "expect '#a' visible",
    ),
    (
        "await expect(page.locator('#a')).toBeHidden({ timeout: 2000 });",
        "expect '#a' hidden --timeout 2s",
    ),
    (
        "await expect(page.locator('#a')).not.toBeVisible();",
        "expect '#a' visible --not",
    ),
    (
        "await expect(page.locator('#a')).toBeVisible({ visible: false });",
        "expect '#a' hidden",
    ),
    (
        "await expect(page.locator('#a')).toBeAttached({ attached: false });",
        "expect '#a' detached",
    ),
    (
        "await expect(page.locator('#a')).toBeEnabled();",
        "expect '#a' enabled",
    ),
    (
        "await expect(page.locator('#a')).toBeDisabled();",
        "expect '#a' disabled",
    ),
    (
        "await expect(page.locator('#a')).toBeEditable();",
        "expect '#a' editable",
    ),
    (
        "await expect(page.locator('#a')).toBeChecked({ checked: false });",
        "expect '#a' unchecked",
    ),
    (
        "await expect(page.locator('#a')).toBeFocused();",
        "expect '#a' focused",
    ),
    (
        "await expect(page.locator('#a')).toBeInViewport();",
        "expect '#a' in-viewport",
    ),
    (
        "await expect(page.locator('#a')).toHaveText('Saved');",
        "expect '#a' text Saved --exact",
    ),
    (
        "await expect(page.locator('#a')).toHaveText(/saved/i);",
        "expect '#a' text /saved/i",
    ),
    (
        "await expect(page.locator('#a')).toContainText('Sav');",
        "expect '#a' text Sav",
    ),
    (
        "await expect.soft(page.locator('#a')).toHaveValue('x', { timeout: 500 });",
        "expect '#a' value x --timeout 500ms",
    ),
    (
        "await expect(page.locator('#a')).toHaveValues(['a', 'b']);",
        "expect '#a' values a b",
    ),
    (
        "await expect(page.locator('#a')).toHaveAttribute('href', '/x');",
        "expect '#a' attr href /x",
    ),
    (
        "await expect(page.locator('#a')).toHaveAttribute('open');",
        "expect '#a' attr open",
    ),
    (
        "await expect(page.locator('#a')).toHaveId('main');",
        "expect '#a' attr id main",
    ),
    (
        "await expect(page.locator('#a')).toHaveClass('btn primary');",
        "expect '#a' attr class 'btn primary'",
    ),
    (
        "await expect(page.locator('#a')).toContainClass('primary');",
        "expect '#a' class primary",
    ),
    (
        "await expect(page.locator('#a')).toHaveCSS('color', 'rgb(255, 0, 0)');",
        "expect '#a' css color 'rgb(255, 0, 0)'",
    ),
    (
        "await expect(page.locator('.row')).toHaveCount(3);",
        "expect .row count 3",
    ),
    (
        "await expect(page).toHaveURL('**/dashboard');",
        "expect --url '**/dashboard'",
    ),
    (
        "await expect(page).not.toHaveURL(/login/);",
        "expect --url /login/ --not",
    ),
    (
        "await expect(page).toHaveTitle('Home');",
        "expect --title Home --exact",
    ),
    (
        "await expect(page).toHaveScreenshot();",
        "expect-screenshot t-1",
    ),
    (
        "await expect(page).toHaveScreenshot('home.png', { fullPage: true, mask: [page.locator('.clock')], maxDiffPixels: 10 });",
        "expect-screenshot home --full --mask .clock --max-diff-pixels 10",
    ),
    (
        "await expect(page.locator('#logo')).toHaveScreenshot('logo.png');",
        "expect-screenshot logo '#logo'",
    ),
    // Waits that follow the action that triggers them.
    (
        "const response = page.waitForResponse('**/api/save');\nawait page.getByRole('button', { name: 'Save' }).click();\nawait response;",
        "click 'role=button[name=Save]'\nwait --response '**/api/save'",
    ),
    (
        "const [download] = await Promise.all([page.waitForEvent('download'), page.locator('#dl').click()]);",
        "click '#dl'\nwait --download",
    ),
    (
        "await Promise.all([page.waitForURL('**/done'), page.locator('#go').click()]);",
        "click '#go'\nwait --url '**/done'",
    ),
    // Steps and values.
    (
        "await test.step('log in', async () => {\n  await page.goto('/login');\n});",
        "# step: log in\ngoto /login",
    ),
    (
        "const email = 'ada@example.com';\nawait page.getByLabel('Email').fill(email);",
        "fill label=Email ada@example.com",
    ),
    (
        "await page.getByLabel('Password').fill(process.env.PASSWORD!);",
        "fill label=Password \"${env:PASSWORD}\"",
    ),
    (
        "await page.goto(`${process.env.BASE_URL}/login`);",
        "goto \"${env:BASE_URL}/login\"",
    ),
    (
        "await page.getByLabel('Name').fill(\"it's\");",
        "fill label=Name \"it's\"",
    ),
    (
        "await page.getByLabel('Name').fill('${x} $y');",
        "fill label=Name '${x} $y'",
    ),
    (
        "await page.getByLabel('Name').fill(\"it's ${x}\");",
        "fill label=Name \"it's $${x}\"",
    ),
    ("console.log('hi');\ntest.slow();", ""),
];

#[test]
fn statements_become_commands() {
    let mut failures = Vec::new();
    for (code, want) in CASES {
        let got = body(code);
        let matches = if want.starts_with("# TODO") {
            got.starts_with(want)
        } else {
            got == *want
        };
        if !matches {
            failures.push(format!("{code}\n  want: {want:?}\n  got:  {got:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Each case: code, and a part of the TODO line it becomes.
const TODOS: &[(&str, &str)] = &[
    ("if (x) { await page.goto('/'); }", "(conditions and loops"),
    (
        "for (const u of users) { await page.goto(u); }",
        "(conditions and loops",
    ),
    ("await page.keyboard.down('Shift');", "(holding keys"),
    (
        "await page.locator('li').filter({ has: page.locator('b') }).click();",
        "(filter({ has })",
    ),
    (
        "await page.locator('a').or(page.locator('b')).click();",
        "(locator.or()",
    ),
    (
        "await page.locator('li:has-text(\"x\")').click();",
        "(Playwright's :has-text( CSS extension",
    ),
    (
        "const t = await page.locator('h1').textContent();",
        "(textContent() gives a value: read it with text LOC",
    ),
    ("expect(title).toBe('x');", "(toBe on a value"),
    (
        "await expect(page.locator('#a')).toBeEmpty();",
        "(toBeEmpty: expect LOC text '' --exact",
    ),
    (
        "await page.route('**/a', async route => { if (x) route.abort(); else route.continue(); });",
        "(a route handler with logic",
    ),
    (
        "page.on('dialog', d => d.accept());",
        "(events: dialog accept",
    ),
    (
        "await page.waitForEvent('popup');",
        "(a popup opens as a new tab",
    ),
    (
        "await page.evaluate(n => n * 2, 21);",
        "(evaluate with an argument",
    ),
    ("await helper(page);", "(not a Playwright call"),
    (
        "await expect(page.locator('#a')).toMatchAriaSnapshot(`- button`);",
        "(an ARIA template",
    ),
];

#[test]
fn what_it_cannot_translate_becomes_todo_lines() {
    let mut failures = Vec::new();
    for (code, want) in TODOS {
        let got = body(code);
        if !(got.starts_with("# TODO line 2 ") && got.contains(want)) {
            failures.push(format!("{code}\n  want: {want:?}\n  got:  {got:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_generated_line_is_a_valid_command() {
    let mut src = String::new();
    for (code, _) in CASES.iter().chain(TODOS) {
        let _ = writeln!(
            src,
            "test('{}', async ({{ page, context }}) => {{\n{code}\n}});",
            src.len()
        );
    }
    let script = convert("all.spec.ts", &src).script;
    if let Err(problems) = script::check(&script) {
        panic!("{}\n\n{script}", problems.join("\n"));
    }
}

#[test]
fn the_fixture_converts_to_a_script_that_runs() {
    let src = include_str!("../../../../testdata/scripts/login.spec.ts");
    let converted = convert("login.spec.ts", src);
    assert_eq!(
        converted.script,
        "\
# Converted from login.spec.ts by chrome-cdp convert: check the TODO lines.
# The tests run one after another, in one session.

# test: sign in > welcomes the user
viewport 1024x768
locale en-GB
route '**/api/flags' --json-body '{\"beta\":true}'
goto /script/login.html
fill label=Email ada@example.com
fill label=Password \"${env:APP_PASSWORD}\"
click 'role=button[name=Sign in]'
expect '#welcome' text 'Welcome, ada@example.com' --exact
expect label=Email value ada@example.com
expect --title '/Sign in/'

# test: sign in > asks for the password
viewport 1024x768
locale en-GB
route '**/api/flags' --json-body '{\"beta\":true}'
goto /script/login.html
fill label=Email ada@example.com
press Enter
expect 'text=Password required' visible
expect 'role=heading[level=1]' hidden --not
# TODO line 27 (textContent() gives a value: read it with text LOC, or check it with expect): const heading = await page.locator('h1').textContent();
# TODO line 28 (toBe on a value, not a locator: compare a command's -q output in a shell): expect(heading).toBe('Sign in');
"
    );
    assert_eq!(
        (converted.tests, converted.steps, converted.todos.len()),
        (2, 18, 2)
    );
    assert_eq!(converted.todos[0].line, 27);
    script::check(&converted.script).unwrap();
}

#[test]
fn describe_hooks_and_skips_keep_their_order() {
    let src = "\
import { test, expect } from '@playwright/test';
test.use({ ...devices['iPhone 15'], colorScheme: 'dark', baseURL: 'http://localhost:3000' });
test.describe('cart', () => {
  test.beforeAll(async ({ browser }) => { console.log('once'); });
  test.beforeEach(async ({ page }) => { await page.goto('/cart'); });
  test.afterEach(async ({ page }) => { await page.screenshot(); });
  test.afterAll(async () => {});
  test('adds', async ({ page }) => { await page.getByText('Add').click(); });
  test.skip('removes', async ({ page }) => {});
  test.describe.skip('later', () => {});
});
";
    let script = convert("cart.spec.ts", src).script;
    let want = "\
# Converted from cart.spec.ts by chrome-cdp convert: check the TODO lines.

# beforeAll

# test: cart > adds
emulate 'iPhone 15'
media --color-scheme dark
# TODO line 2 (set base_url in .chrome-cdp.toml; goto /path resolves against it): test.use({ ...devices['iPhone 15'], colorScheme: 'dark', baseURL: 'http://localhost:3000' });
goto /cart
click text=Add
screenshot

# skipped: cart > removes

# skipped: cart > later

# afterAll
";
    assert_eq!(script, want);
}

#[test]
fn a_script_without_tests_converts_as_one_flow() {
    let src = "\
const { chromium } = require('playwright');
(async () => {
  const browser = await chromium.launch({ headless: false });
  const context = await browser.newContext({ viewport: { width: 800, height: 600 }, locale: 'nb-NO' });
  const page = await context.newPage();
  await page.goto('https://example.com');
  const second = await context.newPage();
  await second.goto('https://example.org');
  await browser.close();
})();
";
    let converted = convert("script.js", src);
    assert_eq!(converted.tests, 0);
    assert_eq!(
        converted.script,
        "\
# Converted from script.js by chrome-cdp convert: check the TODO lines.

open --headed
viewport 800x600
locale nb-NO
goto https://example.com
tab new
goto https://example.org
close
"
    );
}

#[test]
fn words_are_quoted_for_the_script_syntax() {
    use super::{Part, Word};
    let lit = |t: &str| Word::lit(t).render();
    assert_eq!(lit("e12"), "e12");
    assert_eq!(lit("#go"), "'#go'");
    assert_eq!(lit(""), "''");
    assert_eq!(lit("a b"), "'a b'");
    assert_eq!(lit("it's"), "\"it's\"");
    assert_eq!(lit("say \"hi\"\n"), r#""say \"hi\"\n""#);
    assert_eq!(lit("it's ${x} \\d"), r#""it's $${x} \\d""#);
    let var = Word(vec![Part::Lit("a\"".into()), Part::Var("env:X".into())]);
    assert_eq!(var.render(), "\"a\\\"${env:X}\"");
}

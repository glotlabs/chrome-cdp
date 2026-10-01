//! `convert`: Playwright statements → `.cdp` lines. What convert writes is
//! also loaded the way `test` loads a scenario (`e2e::scenario::load`), and
//! every step checked as a command, so it always parses.

use std::fmt::Write as _;

use super::convert;
use crate::script;

/// The lines `code` becomes inside a test, without the header and the
/// `@test` line, unindented.
fn body(code: &str) -> String {
    let src = format!("test('t', async ({{ page, context }}) => {{\n{code}\n}});\n");
    let script = convert("t.spec.ts", &src).script;
    let lines: Vec<&str> = script
        .lines()
        .skip_while(|l| !l.starts_with("@test \"t\""))
        .skip(1)
        .take_while(|l| !l.is_empty())
        .map(|l| l.strip_prefix("  ").unwrap_or(l))
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
    // filter({ has, hasNot, hasNotText }), or() and and() (plan 14).
    (
        "await page.getByRole('row').filter({ has: page.getByRole('button', { name: 'Delete' }) }).click();",
        "click 'role=row >> has=role=button[name=Delete]'",
    ),
    (
        "await page.locator('li').filter({ hasNot: page.locator('b').first() }).first().click();",
        "click 'css=li >> has-not=\"css=b >> nth=0\" >> nth=0'",
    ),
    (
        "await page.getByRole('row').filter({ hasNotText: 'Paid' }).first().click();",
        "click 'role=row >> has-not-text=Paid >> nth=0'",
    ),
    (
        "const del = page.getByText('Delete', { exact: true });\nawait page.locator('.row').filter({ has: del }).click();",
        "click '.row >> has=text=\"Delete\"'",
    ),
    (
        "await page.getByRole('button', { name: 'New' }).or(page.getByRole('link', { name: 'New' })).click();",
        "click 'role=button[name=New] >> or=role=link[name=New]'",
    ),
    (
        "await page.getByRole('button').or(page.locator('.row').getByRole('link')).click();",
        "click 'role=button >> or=\".row >> role=link\"'",
    ),
    (
        "await page.getByRole('button').and(page.getByTitle('Subscribe')).click();",
        "click 'role=button >> and=title=Subscribe'",
    ),
    (
        "const row = page.getByRole('row').first();\nawait row.getByRole('button').or(row.getByRole('link')).click();",
        "click 'role=row >> nth=0 >> role=button >> or=role=link'",
    ),
    (
        "await page.locator('#list').locator('li').first().or(page.locator('#list').getByText('None')).click();",
        "click '#list >> css=li >> nth=0 >> or=text=None'",
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
    ("await page.keyboard.down('Shift');", "keyboard down Shift"),
    (
        "await page.keyboard.up('ControlOrMeta');",
        "keyboard up Mod",
    ),
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
        "pdf -o a.pdf --format A4 --margin 0",
    ),
    (
        "await page.pdf({ margin: { top: '1cm', bottom: 20 } });",
        "pdf --margin 1cm,0,20px,0",
    ),
    (
        "await page.pdf({ path: 'a.pdf', scale: 0.8, pageRanges: '1-2, 4', displayHeaderFooter: true, footerTemplate: '<span class=\"pageNumber\"></span>' });",
        "pdf -o a.pdf --scale 0.8 --pages 1-2,4 --margin 0 --footer '<span class=\"pageNumber\"></span>'",
    ),
    (
        "await page.pdf({ headerTemplate: '<b>x</b>' });",
        "pdf --margin 0",
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
    (
        "await page.exposeFunction('getConfig', async () => ({ beta: true, n: [1] }));",
        "expose getConfig --returns '{\"beta\":true,\"n\":[1]}'",
    ),
    (
        "await page.exposeFunction('twice', (x) => x * 2);",
        "expose twice",
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
    (
        "await page.clock.pauseAt(new Date('2026-01-31T10:00:00Z'));",
        "time pause 2026-01-31T10:00:00Z",
    ),
    ("await page.clock.runFor(2000);", "time run-for 2s"),
    ("await page.clock.runFor('00:30');", "time run-for 30s"),
    ("await page.clock.resume();", "time resume"),
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
    // Values known now: constants, lists, objects.
    (
        "const ITEMS = ['a b', 'c'];\nawait page.getByLabel('New').fill(ITEMS[1]);",
        "fill label=New c",
    ),
    (
        "const links = { home: '/home' };\nawait page.goto(links.home);",
        "goto /home",
    ),
    (
        "const ITEMS = ['a b', 'c'];\nawait expect(page.getByTestId('title')).toHaveText(ITEMS.slice(0, 1));",
        "expect testid=title count 1\nexpect 'testid=title >> nth=0' text 'a b' --exact",
    ),
    (
        "await expect(page.locator('li')).toHaveText(['One', /two/i], { timeout: 1000 });",
        "expect css=li count 2 --timeout 1s\nexpect 'css=li >> nth=0' text One --exact --timeout 1s\nexpect 'css=li >> nth=1' text /two/i --timeout 1s",
    ),
    (
        "await expect(page.locator('li')).toHaveClass(['done', '']);",
        "expect css=li count 2\nexpect 'css=li >> nth=0' attr class done\nexpect 'css=li >> nth=1' class '/^$/'",
    ),
    (
        "await expect(page.getByPlaceholder('Search')).toBeEmpty();",
        "expect placeholder=Search value ''",
    ),
    // Arguments of page functions, as JSON.
    (
        "await page.evaluate(n => n * 2, 21);",
        "eval '(n => n * 2)(21)'",
    ),
    (
        "await page.waitForFunction(([a, b]) => a < b, [1, 2], { timeout: 2000 });",
        "wait --fn '(([a, b]) => a < b)([1,2])' --timeout 2s",
    ),
    (
        "await page.locator('#a').evaluate((el, c) => el.classList.add(c), 'on');",
        "eval --in '#a' 'el => ((el, c) => el.classList.add(c))(el, \"on\")'",
    ),
    // TypeScript is left out of what runs in the page.
    (
        "await page.waitForFunction((n: number): boolean => (window as any).count === n!, 3);",
        "wait --fn '((n) => (window).count === n)(3)'",
    ),
    // Loops over lists known now.
    (
        "for (const name of ['a', 'b']) {\n  await page.getByLabel(name).check();\n}",
        "check label=a\ncheck label=b",
    ),
    // Locators kept in variables of a context.
    (
        "const ctx = page.context();\nawait ctx.clearCookies();",
        "cookies clear",
    ),
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
    (
        "await page.pdf({ displayHeaderFooter: true });",
        "displayHeaderFooter without templates",
    ),
    ("if (x) { await page.goto('/'); }", "(conditions and loops"),
    (
        "for (const u of users) { await page.goto(u); }",
        "(conditions and loops",
    ),
    (
        "await page.locator('#zone').dispatchEvent('drop', { dataTransfer });",
        "(dispatching drag events: drop files on the element with upload LOC FILE --drop",
    ),
    (
        "const row = page.locator('.row'); await row.getByRole('button').or(page.getByRole('link')).click();",
        "(locator.or() of a locator from another parent",
    ),
    (
        "await page.locator('.row').getByRole('button').and(page.getByRole('link')).click();",
        "(locator.and() of a locator from another parent",
    ),
    (
        "await page.locator('li').filter({ has: page.frameLocator('#pay').locator('b') }).click();",
        "(filter({ has }) with a frame locator inside",
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
        "await page.evaluate(n => n * 2, count);",
        "(evaluate with an argument",
    ),
    (
        "await helper(page);",
        "(helper() is not defined in this file: inline its steps",
    ),
    (
        "await expect(page.getByTestId('row')).toContainText(['a', 'b']);",
        "(a list of texts",
    ),
    (
        "if (x) { await page.waitForFunction((n: number) => n > 1, 2); }",
        "(conditions and loops",
    ),
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
    if let Err(problems) = script::check_scenario(&script) {
        panic!("{}\n\n{script}", problems.join("\n"));
    }
}

#[test]
fn the_fixture_converts_to_a_scenario_that_loads() {
    let src = include_str!("../../../../testdata/scripts/login.spec.ts");
    let converted = convert("login.spec.ts", src);
    assert_eq!(
        converted.script,
        "\
# Converted from login.spec.ts by chrome-cdp convert: check the TODO lines.
# Run it with chrome-cdp test: each test gets a fresh browser context.

@viewport 1024x768

@before
  locale en-GB
  route '**/api/flags' --json-body '{\"beta\":true}'
  goto /script/login.html

@test \"sign in > welcomes the user\"
  fill label=Email ada@example.com
  fill label=Password \"${env:APP_PASSWORD}\"
  click 'role=button[name=Sign in]'
  expect '#welcome' text 'Welcome, ada@example.com' --exact
  expect label=Email value ada@example.com
  expect --title '/Sign in/'

@test \"sign in > asks for the password\"
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
        (2, 13, 2)
    );
    assert_eq!(converted.todos[0].line, 27);
    assert_eq!(script::check_scenario(&converted.script).unwrap().len(), 13);
}

#[test]
fn describe_hooks_skips_and_settings_become_directives() {
    let src = "\
import { test, expect } from '@playwright/test';
test.use({ colorScheme: 'dark', baseURL: 'http://localhost:3000' });
test.describe('cart', () => {
  test.describe.configure({ retries: 2 });
  test.beforeAll(async ({ browser }) => { console.log('once'); });
  test.beforeEach(async ({ page }) => { await page.goto('/cart'); });
  test.afterEach(async ({ page }) => { await page.getByText('Clear').click(); });
  test.afterAll(async ({ request }) => { await request.delete('/api/cart'); });
  test('adds', async ({ page }) => { await page.getByText('Add').click(); });
  test.skip('removes', async ({ page }) => {});
  test.fixme('empties', async ({ page }) => { await page.getByText('Empty').click(); });
  test.describe.skip('later', () => {
    test('pays', async ({ page }) => { await page.getByText('Pay').click(); });
  });
  test.describe('wide', () => {
    test.use({ viewport: { width: 1600, height: 900 }, locale: 'nb-NO' });
    test.beforeEach(async ({ page }) => { await page.getByText('Wide').click(); });
    test.only('shows all', async ({ page }) => {
      test.setTimeout(60000);
      await page.getByText('All').click();
    });
    test('hides none', async ({ page }) => {
      test.skip();
      await page.getByText('None').click();
    });
    test('reads', async ({ page }) => {
      const t = await page.locator('h1').textContent();
    });
  });
});
";
    let converted = convert("cart.spec.ts", src);
    let want = "\
# Converted from cart.spec.ts by chrome-cdp convert: check the TODO lines.
# Run it with chrome-cdp test: each test gets a fresh browser context.

@retries 2

@before
  media --color-scheme dark
  # TODO line 2 (set base_url in .chrome-cdp.toml; goto /path resolves against it): test.use({ colorScheme: 'dark', baseURL: 'http://localhost:3000' });
  goto /cart

@after
  click text=Clear

# afterAll: no step to run
# TODO line 8 (request is not a locator the script knows): await request.delete('/api/cart');

@test \"cart > adds\"
  click text=Add

@test \"cart > removes\" @skip

@test \"cart > empties\" @skip \"fixme\"
  click text=Empty

@test \"cart > later > pays\" @skip
  click text=Pay

@test \"cart > wide > shows all\" @only @timeout 60s @viewport 1600x900
  locale nb-NO
  click text=Wide
  click text=All

@test \"cart > wide > hides none\" @skip @viewport 1600x900
  locale nb-NO
  click text=Wide
  click text=None

@test \"cart > wide > reads\" @viewport 1600x900
  locale nb-NO
  click text=Wide
  # TODO line 27 (textContent() gives a value: read it with text LOC, or check it with expect): const t = await page.locator('h1').textContent();
";
    assert_eq!(converted.script, want);
    assert_eq!(converted.tests, 7);
    script::check_scenario(&converted.script).unwrap();
}

#[test]
fn a_test_without_steps_is_skipped_so_the_scenario_loads() {
    let src = "\
test('reads', async ({ page }) => {
  const t = await page.locator('h1').textContent();
});
test('empty', async ({ page }) => {});
";
    let script = convert("t.spec.ts", src).script;
    assert!(
        script.contains(
            "\n@test \"reads\" @skip \"convert: rewrite its TODO lines\"\n  # TODO line 2 "
        ),
        "{script}"
    );
    assert!(
        script.contains("\n@test \"empty\" @skip \"convert: no steps\"\n"),
        "{script}"
    );
    script::check_scenario(&script).unwrap();
}

#[test]
fn hooks_with_unnamed_screenshots_stay_in_each_test() {
    let src = "\
test.afterEach(async ({ page }) => { await expect(page).toHaveScreenshot(); });
test('a', async ({ page }) => { await page.goto('/a'); });
test('b', async ({ page }) => { await page.goto('/b'); });
";
    let script = convert("t.spec.ts", src).script;
    assert!(!script.contains("@after"), "{script}");
    assert!(
        script.contains("@test \"a\"\n  goto /a\n  expect-screenshot a-1\n"),
        "{script}"
    );
    assert!(
        script.contains("@test \"b\"\n  goto /b\n  expect-screenshot b-1\n"),
        "{script}"
    );
    script::check_scenario(&script).unwrap();
}

#[test]
fn an_inner_viewport_keeps_the_files_device() {
    let src = "\
test.use({ ...devices['Pixel 7'] });
test.describe('wide', () => {
  test.use({ viewport: { width: 1600, height: 900 } });
  test('a', async ({ page }) => { await page.goto('/a'); });
});
test('b', async ({ page }) => { await page.goto('/b'); });
";
    let script = convert("t.spec.ts", src).script;
    assert!(script.contains("\n@device \"Pixel 7\"\n"), "{script}");
    assert!(
        script.contains("\n@test \"wide > a\"\n  viewport 1600x900\n  goto /a\n"),
        "{script}"
    );
    script::check_scenario(&script).unwrap();
}

#[test]
fn conditional_skips_become_todo_lines() {
    let src = "\
test.skip(({ isMobile }) => isMobile, 'desktop only');
test('a', async ({ page }) => {
  test.skip(process.env.CI, 'not in CI');
  await page.goto('/a');
});
";
    let converted = convert("t.spec.ts", src);
    let reasons: Vec<(usize, &str)> = converted
        .todos
        .iter()
        .map(|t| (t.line, t.reason.as_str()))
        .collect();
    assert_eq!(
        reasons,
        [
            (
                1,
                "a runner call with a condition: write @skip on the tests it skips"
            ),
            (
                3,
                "a runner call with a condition: write @skip on the @test line when it holds"
            ),
        ]
    );
    assert!(
        converted.script.contains("\n@test \"a\"\n"),
        "{}",
        converted.script
    );
    script::check_scenario(&converted.script).unwrap();
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

#[test]
fn conditions_on_the_browser_hold_or_not_in_chrome() {
    let src = "\
test.skip(({ browserName }) => browserName !== 'chromium', 'Chromium only');
test.describe('d', () => {
  test.fixme(({ browserName }) => browserName === 'chromium', 'broken in Chrome');
  test('b', async ({ page }) => { await page.goto('/b'); });
});
test('a', async ({ page, browserName }) => {
  test.skip(browserName === 'webkit' || browserName === 'firefox', 'slow there');
  await page.goto('/a');
});
";
    let converted = convert("t.spec.ts", src);
    assert!(converted.todos.is_empty(), "{:?}", converted.todos);
    assert!(
        converted
            .script
            .contains("\n@test \"d > b\" @skip \"broken in Chrome\"\n"),
        "{}",
        converted.script
    );
    assert!(
        converted.script.contains("\n@test \"a\"\n  goto /a\n"),
        "{}",
        converted.script
    );
    script::check_scenario(&converted.script).unwrap();
}

#[test]
fn details_objects_give_tags_and_skips() {
    let src = "\
test.describe('cart', { tag: '@smoke' }, () => {
  test('adds', { tag: ['@fast', '@smoke'] }, async ({ page }) => { await page.goto('/a'); });
  test('pays', { annotation: { type: 'fixme', description: 'payment sandbox down' } }, async ({ page }) => {
    await page.goto('/b');
  });
  test('lists', { annotation: [{ type: 'issue', description: 'https://x/1' }] }, async ({ page }) => {
    await page.goto('/c');
  });
});
";
    let converted = convert("t.spec.ts", src);
    assert!(converted.todos.is_empty(), "{:?}", converted.todos);
    let script = &converted.script;
    assert!(script.contains("\n@tags smoke\n"), "{script}");
    assert!(
        script.contains("\n@test \"cart > adds\" @tags fast\n"),
        "{script}"
    );
    assert!(
        script.contains("\n@test \"cart > pays\" @skip \"payment sandbox down\"\n"),
        "{script}"
    );
    assert!(script.contains("\n@test \"cart > lists\"\n"), "{script}");
    script::check_scenario(script).unwrap();
}

#[test]
fn helpers_of_the_file_are_inlined() {
    let src = "\
import { test, expect, type Page } from '@playwright/test';

test('adds', async ({ page }) => {
  await addTodos(page, ['a', 'b']);
  await expect(row(page, 1)).toHaveText('b');
  await checkCount(page, 2);
  await openMenu({ page, label: 'Settings' });
});

async function addTodos(page: Page, items: string[]) {
  const input = page.getByPlaceholder('New todo');
  for (const item of items) {
    await input.fill(item);
    await input.press('Enter');
  }
}

const row = (page: Page, i: number) => page.getByTestId('row').nth(i);

async function checkCount(page: Page, expected: number) {
  return await page.waitForFunction(e => document.querySelectorAll('li').length === e, expected);
}

const openMenu = async ({ page, label }: { page: Page; label: string }) => {
  await page.getByRole('button', { name: label }).click();
  if (label === 'x') console.log('never');
};
";
    let converted = convert("t.spec.ts", src);
    let want = "\
@test \"adds\"
  fill 'placeholder=New todo' a
  press Enter --in 'placeholder=New todo'
  fill 'placeholder=New todo' b
  press Enter --in 'placeholder=New todo'
  expect 'testid=row >> nth=1' text b --exact
  wait --fn \"(e => document.querySelectorAll('li').length === e)(2)\"
  click 'role=button[name=Settings]'
  # TODO line 26 (in openMenu(), called on line 7: conditions and loops: split into scripts, or drive chrome-cdp from a shell): if (label === 'x') console.log('never');
";
    assert!(converted.script.ends_with(want), "{}", converted.script);
    script::check_scenario(&converted.script).unwrap();
}

#[test]
fn values_the_file_changes_are_not_constants() {
    let src = "\
let log = [];
const names = ['a'];
names.push('b');
test('t', async ({ page }) => {
  let step = 'one';
  step = 'two';
  await page.getByLabel(step).click();
  for (const n of names) { await page.getByLabel(n).click(); }
  await page.getByLabel(log[0]).click();
});
";
    let converted = convert("t.spec.ts", src);
    let reasons: Vec<(usize, &str)> = converted
        .todos
        .iter()
        .map(|t| (t.line, t.reason.as_str()))
        .collect();
    assert_eq!(
        reasons,
        [
            (5, "step changes later: the script needs one value"),
            (6, "an assignment: the script has no variables to change"),
            (
                7,
                "step is set on line 5, which convert could not translate (step changes later: the script needs one value)"
            ),
            (
                8,
                "conditions and loops: split into scripts, or drive chrome-cdp from a shell"
            ),
            (9, "a value computed at run time"),
        ]
    );
}

#[test]
fn todo_lines_say_where_an_unknown_value_came_from() {
    let src = "\
import translations from './i18n.json';
test.afterAll(async () => await cleanUp());
test('t', async ({ page }) => {
  const save = page.getByRole('button', { name: translations.save });
  await save.click();
  await expect(save).toBeEnabled();
  const loop = { [key]: 1 };
});
";
    let converted = convert("t.spec.ts", src);
    let todos: Vec<(usize, &str, &str)> = converted
        .todos
        .iter()
        .map(|t| (t.line, t.reason.as_str(), t.code.as_str()))
        .collect();
    let unknown =
        "save is set on line 4, which convert could not translate (a value computed at run time)";
    assert_eq!(
        todos,
        [
            (
                2,
                "cleanUp() is not defined in this file: inline its steps",
                "async () => await cleanUp()"
            ),
            (
                4,
                "a value computed at run time",
                "const save = page.getByRole('button', { name: translations.save });"
            ),
            (5, unknown, "await save.click();"),
            (6, unknown, "await expect(save).toBeEnabled();"),
            (
                7,
                "a value the script cannot keep",
                "const loop = { [key]: 1 };"
            ),
        ]
    );
}

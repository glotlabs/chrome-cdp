// A Playwright test of site/script/login.html, for `chrome-cdp convert`.
import { test, expect } from '@playwright/test';

test.describe('sign in', () => {
  test.use({ viewport: { width: 1024, height: 768 }, locale: 'en-GB' });

  test.beforeEach(async ({ page }) => {
    await page.route('**/api/flags', (route) => route.fulfill({ json: { beta: true } }));
    await page.goto('/script/login.html');
  });

  test('welcomes the user', async ({ page }) => {
    const email = page.getByLabel('Email');
    await email.fill('ada@example.com');
    await page.getByLabel('Password').fill(process.env.APP_PASSWORD!);
    await page.getByRole('button', { name: 'Sign in' }).click();
    await expect(page.locator('#welcome')).toHaveText('Welcome, ada@example.com');
    await expect(email).toHaveValue('ada@example.com');
    await expect(page).toHaveTitle(/Sign in/);
  });

  test('asks for the password', async ({ page }) => {
    await page.getByLabel('Email').fill('ada@example.com');
    await page.keyboard.press('Enter');
    await expect(page.getByText('Password required')).toBeVisible();
    await expect(page.getByRole('heading', { level: 1 })).not.toBeHidden();
    const heading = await page.locator('h1').textContent();
    expect(heading).toBe('Sign in');
  });
});

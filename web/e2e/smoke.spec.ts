import { expect, test } from '@playwright/test'

test('opens the dashboard and moves between screens', async ({ page }) => {
  await page.goto('/')
  await expect(page.getByRole('heading', { level: 1, name: 'Dashboard' })).toBeVisible()
  await expect(page).toHaveTitle('Dashboard · dalsegno')

  const navigation = page.getByRole('navigation', { name: 'Main' })
  await navigation.getByRole('link', { name: 'Jobs' }).click()
  await expect(page).toHaveURL('/jobs')
  await expect(page.getByRole('heading', { level: 1, name: 'Jobs' })).toBeVisible()
  await expect(navigation.getByRole('link', { name: 'Jobs' })).toHaveAttribute(
    'aria-current',
    'page',
  )
})

test('opens a screen straight from a link, as a notification does', async ({ page }) => {
  await page.goto('/changes/7')
  await expect(page.getByRole('heading', { level: 1, name: 'Change' })).toBeVisible()
})

test('reaches the navigation from the keyboard', async ({ page }) => {
  await page.goto('/')
  await page.keyboard.press('Tab')
  await expect(page.getByRole('link', { name: 'Skip to content' })).toBeFocused()
  await page.keyboard.press('Tab')
  await expect(page.getByRole('link', { name: 'dalsegno' })).toBeFocused()
})

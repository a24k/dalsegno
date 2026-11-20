import { expect, test } from '@playwright/test'

// The service starts on an empty data directory, so there are no streams or changes, and the
// sweep it queues at start is the one job.

test('the dashboard shows each part, even when empty', async ({ page }) => {
  await page.goto('/')

  await expect(page.getByRole('heading', { level: 2, name: 'Streams' })).toBeVisible()
  await expect(page.getByText('No streams yet.')).toBeVisible()
  await expect(page.getByText('No change is waiting.')).toBeVisible()
  await expect(page.getByText(/Job \d+, sweep:/)).toBeVisible()
})

test('the jobs screen lists the sweep the service ran', async ({ page }) => {
  await page.goto('/jobs')

  const row = page.getByRole('row', { name: /sweep/ })
  await expect(row).toBeVisible()
  await expect(row.getByText('succeeded')).toBeVisible()
})

test('a setting is saved, applies, and goes back to its default', async ({ page }) => {
  await page.goto('/settings')

  await page.getByLabel('Requests in flight').fill('2')
  await page.getByRole('button', { name: 'Save settings' }).click()
  await expect(page.getByRole('main').getByRole('status')).toContainText(
    'Saved. The new values apply at once.',
  )

  await page.reload()
  await expect(page.getByLabel('Requests in flight')).toHaveValue('2')
  await page.getByRole('button', { name: 'Use the default for requests in flight' }).click()
  await expect(page.getByLabel('Requests in flight')).toHaveValue('4')
})

test('a value out of range is refused with the reason', async ({ page }) => {
  await page.goto('/settings')

  await page.getByLabel('Request starts per second').fill('0.001')
  // The browser would stop the form at its own minimum, so this asks the service directly.
  await page.getByLabel('Request starts per second').evaluate((input) => {
    input.removeAttribute('min')
  })
  await page.getByRole('button', { name: 'Save settings' }).click()

  await expect(page.getByRole('alert')).toContainText('requests_per_second: out of range')
})

test('diagnostics run in the service and show its tools', async ({ page }) => {
  await page.goto('/settings')

  await page.getByRole('button', { name: 'Run diagnostics' }).click()

  await expect(page.getByRole('heading', { level: 3, name: 'Tools' })).toBeVisible()
  await expect(page.getByRole('row', { name: /ffmpeg/ })).toBeVisible()
  await expect(page.getByText('none since the service started')).toBeVisible()
})

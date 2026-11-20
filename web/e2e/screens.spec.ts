import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
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

test('the streams screen loads with its empty state', async ({ page }) => {
  await page.goto('/streams')

  await expect(page.getByRole('heading', { level: 1, name: 'Streams' })).toBeVisible()
  await expect(page.getByText('No streams yet.')).toBeVisible()
  await expect(page.getByLabel('Video ID or URL')).toBeVisible()
})

test('adding something that is not a video shows the reason', async ({ page }) => {
  await page.goto('/streams')

  await page.getByLabel('Video ID or URL').fill('not a video')
  await page.getByRole('button', { name: 'Add the stream' }).click()

  await expect(page.getByRole('alert')).toBeVisible()
  await expect(page.getByText('No streams yet.')).toBeVisible()
})

test('a stream that does not exist is not found', async ({ page }) => {
  await page.goto('/streams/9999')

  await expect(page.getByRole('heading', { level: 1, name: 'Not found' })).toBeVisible()
})

test('a change that does not exist is not found', async ({ page }) => {
  await page.goto('/changes/1')

  await expect(page.getByRole('heading', { level: 1, name: 'Not found' })).toBeVisible()
  await expect(page.getByText('There is no such change.')).toBeVisible()
})

test('the header shows the version of the running service', async ({ page }) => {
  const manifest = readFileSync(resolve(import.meta.dirname, '../../Cargo.toml'), 'utf8')
  const workspace = manifest.split('[workspace.package]')[1] ?? ''
  const version = /^version\s*=\s*"([^"]+)"/m.exec(workspace)?.[1]
  expect(version).toBeDefined()

  await page.goto('/')

  await expect(page.locator('header .version')).toHaveText(version ?? '')
})

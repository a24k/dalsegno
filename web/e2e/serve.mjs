// Starts `dalsegno serve` on an empty, temporary data directory for the end-to-end tests,
// and removes the directory when Playwright stops it.

import { spawn } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const binary =
  process.env.DALSEGNO_BIN ?? resolve(import.meta.dirname, '../../target/debug/dalsegno')
const bind = process.env.DALSEGNO_BIND ?? '127.0.0.1:8738'
const data = mkdtempSync(join(tmpdir(), 'dalsegno-e2e-'))

// Nothing of the developer's own setup reaches the service: no config file under HOME, and no
// DALSEGNO_* setting such as a webhook that would post the test runs.
const inherited = Object.entries(process.env).filter(([name]) => !name.startsWith('DALSEGNO_'))
const env = {
  ...Object.fromEntries(inherited),
  HOME: data,
  DALSEGNO_DATA: data,
  DALSEGNO_BIND: bind,
}

const service = spawn(binary, ['serve'], { env, stdio: 'inherit' })

function cleanUp() {
  rmSync(data, { recursive: true, force: true })
}

service.on('error', (error) => {
  console.error(`dalsegno could not start from ${binary}: ${error.message}`)
  cleanUp()
  process.exit(1)
})

service.on('exit', (code) => {
  cleanUp()
  process.exit(code ?? 0)
})

for (const signal of ['SIGINT', 'SIGTERM']) {
  // The service stops cleanly on an interrupt.
  process.on(signal, () => service.kill('SIGINT'))
}

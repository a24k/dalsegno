import { describe, expect, test } from 'vitest'
import { EMPTY_PROGRAM_FORM, formOf, knownClipBody, programBody, seconds } from './forms'
import type { Program } from './types'

const program: Program = {
  id: 1,
  short_name: 'example',
  channel_id: null,
  name_template: '{short_name}',
  addition_suffix: ' addition',
  artist: null,
  content_date: '2026-11-01',
  description: null,
  output_dir: '/out',
  stream_ids: [2],
}

describe('programBody', () => {
  test('leaves the template and suffix out when they are empty', () => {
    const body = programBody({ ...EMPTY_PROGRAM_FORM, shortName: ' a ', outputDir: '/o' })
    expect(body).toEqual({
      short_name: 'a',
      output_dir: '/o',
      artist: '',
      content_date: '',
      description: '',
    })
  })

  test('sends the template and suffix when they are given', () => {
    const body = programBody({ ...formOf(program) })
    expect(body.name_template).toBe('{short_name}')
    expect(body.addition_suffix).toBe(' addition')
  })
})

describe('formOf', () => {
  test('turns missing optional fields into empty strings', () => {
    const form = formOf(program)
    expect(form.artist).toBe('')
    expect(form.description).toBe('')
    expect(form.contentDate).toBe('2026-11-01')
  })
})

describe('seconds', () => {
  test('reads a number of 0 or more', () => {
    expect(seconds('12.5')).toBe(12.5)
    expect(seconds('0')).toBe(0)
  })

  test('refuses an empty, negative or non-numeric entry', () => {
    expect(seconds('')).toBeNull()
    expect(seconds('-1')).toBeNull()
    expect(seconds('abc')).toBeNull()
  })
})

describe('knownClipBody', () => {
  test('builds the body from valid entries', () => {
    expect(knownClipBody(3, ' Intro ', '10', '40')).toEqual({
      body: { name: 'Intro', stream: 3, start: 10, end: 40 },
    })
  })

  test('says what is wrong with the entries', () => {
    expect(knownClipBody(3, '', '1', '2')).toHaveProperty('problem')
    expect(knownClipBody(3, 'a', 'x', '2')).toHaveProperty('problem')
    expect(knownClipBody(3, 'a', '5', '5')).toHaveProperty('problem')
  })
})

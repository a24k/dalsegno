// Turning what a form holds into the bodies the API takes.

import type { Program } from './types'

/** The fields of the program form, as the inputs hold them. */
export interface ProgramForm {
  shortName: string
  outputDir: string
  nameTemplate: string
  additionSuffix: string
  artist: string
  contentDate: string
  description: string
}

/** A form with nothing in it, for a new program. */
export const EMPTY_PROGRAM_FORM: ProgramForm = {
  shortName: '',
  outputDir: '',
  nameTemplate: '',
  additionSuffix: '',
  artist: '',
  contentDate: '',
  description: '',
}

/** The form for a stored program. */
export function formOf(program: Program): ProgramForm {
  return {
    shortName: program.short_name,
    outputDir: program.output_dir,
    nameTemplate: program.name_template,
    additionSuffix: program.addition_suffix,
    artist: program.artist ?? '',
    contentDate: program.content_date ?? '',
    description: program.description ?? '',
  }
}

/**
 * The body of `POST /programs` and `PUT /programs/{id}`. A template or suffix left empty is
 * left out, so the service uses its default; both are sent as typed, as a suffix may start
 * with a space. An empty optional field is sent as an empty
 * string, which the service stores as none.
 */
export function programBody(form: ProgramForm): Record<string, string> {
  const body: Record<string, string> = {
    short_name: form.shortName.trim(),
    output_dir: form.outputDir.trim(),
    artist: form.artist.trim(),
    content_date: form.contentDate.trim(),
    description: form.description.trim(),
  }
  if (form.nameTemplate.trim() !== '') {
    body.name_template = form.nameTemplate
  }
  if (form.additionSuffix.trim() !== '') {
    body.addition_suffix = form.additionSuffix
  }
  return body
}

/** The body of `POST /programs/{id}/known-clips`, or the reason the entries cannot be one. */
export function knownClipBody(
  stream: number,
  name: string,
  from: string,
  to: string,
): { body: Record<string, unknown> } | { problem: string } {
  const start = seconds(from)
  const end = seconds(to)
  if (name.trim() === '') {
    return { problem: 'Give the clip a name.' }
  }
  if (start === null || end === null) {
    return { problem: 'Give the start and the end as seconds of stream time.' }
  }
  if (end <= start) {
    return { problem: 'The end must come after the start.' }
  }
  return { body: { name: name.trim(), stream, start, end } }
}

/** Seconds typed in a field, or null when it is empty or not a number of 0 or more. */
export function seconds(text: string): number | null {
  if (text.trim() === '') {
    return null
  }
  const value = Number(text)
  return Number.isFinite(value) && value >= 0 ? value : null
}

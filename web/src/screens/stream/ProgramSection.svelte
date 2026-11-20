<script lang="ts">
  import { api } from '../../lib/api'
  import type { Feedback } from '../../lib/feedback.svelte'
  import { EMPTY_PROGRAM_FORM, formOf, programBody, type ProgramForm } from '../../lib/forms'
  import type { Program, Stream } from '../../lib/types'
  import type { StreamDetail } from './detail.svelte'
  import ProgramFields from './ProgramFields.svelte'

  let { detail, stream, feedback }: { detail: StreamDetail; stream: Stream; feedback: Feedback } =
    $props()

  let editing = $state<ProgramForm>({ ...EMPTY_PROGRAM_FORM })
  let creating = $state<ProgramForm>({ ...EMPTY_PROGRAM_FORM })
  let target = $state('')

  let others = $derived(detail.programs.filter((program) => program.id !== stream.program_id))

  $effect(() => {
    if (detail.program) {
      editing = formOf(detail.program)
    }
  })

  function save(event: SubmitEvent): void {
    event.preventDefault()
    const program = detail.program
    if (!program) {
      return
    }
    void feedback.run(
      async () => {
        await api.put<Program>(`/programs/${program.id}`, programBody(editing))
        await detail.loadProgram()
        await detail.loadPrograms()
      },
      'The program is saved.',
      'The program could not be saved.',
    )
  }

  async function link(programId: number): Promise<void> {
    await detail.show(await api.post<Stream>(`/streams/${stream.id}/link`, { program: programId }))
    await detail.loadPrograms()
  }

  function linkExisting(event: SubmitEvent): void {
    event.preventDefault()
    void feedback.run(() => link(Number(target)), 'The stream is linked.', 'The link failed.')
  }

  function create(event: SubmitEvent): void {
    event.preventDefault()
    void feedback.run(
      async () => {
        const program = await api.post<Program>('/programs', programBody(creating))
        await link(program.id)
        creating = { ...EMPTY_PROGRAM_FORM }
      },
      'The program is made and linked.',
      'The program could not be made.',
    )
  }
</script>

<section aria-labelledby="program-heading">
  <h2 id="program-heading">Program</h2>
  {#if detail.program}
    <p class="muted">
      Program {detail.program.id}, shared with
      {detail.program.stream_ids.length - 1} other
      {detail.program.stream_ids.length === 2 ? 'stream' : 'streams'}.
    </p>
    <form onsubmit={save}>
      <ProgramFields bind:form={editing} prefix="edit" />
      <button type="submit">Save program</button>
    </form>
  {:else}
    <p>This stream has no program yet. Link it to one or make a new one.</p>
  {/if}

  <h3>Link to another program</h3>
  {#if others.length === 0}
    <p class="muted">There is no other program to link to.</p>
  {:else}
    <form onsubmit={linkExisting}>
      <label for="link-program">Program</label>
      <select id="link-program" required bind:value={target}>
        <option value="">Choose a program</option>
        {#each others as program (program.id)}
          <option value={String(program.id)}>{program.short_name} ({program.id})</option>
        {/each}
      </select>
      <button type="submit">Link to this program</button>
    </form>
  {/if}

  <h3>New program</h3>
  <form onsubmit={create}>
    <ProgramFields bind:form={creating} prefix="new" />
    <button type="submit">Make the program and link it</button>
  </form>
</section>

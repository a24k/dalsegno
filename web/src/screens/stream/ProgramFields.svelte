<script lang="ts">
  import type { ProgramForm } from '../../lib/forms'

  let { form = $bindable(), prefix }: { form: ProgramForm; prefix: string } = $props()
</script>

<div class="field">
  <label for="{prefix}-short-name">Short name</label>
  <input id="{prefix}-short-name" type="text" required bind:value={form.shortName} />
</div>
<div class="field">
  <label for="{prefix}-output-dir">Output directory</label>
  <input id="{prefix}-output-dir" type="text" required bind:value={form.outputDir} />
</div>
<div class="field">
  <label for="{prefix}-template">Name template</label>
  <input id="{prefix}-template" type="text" bind:value={form.nameTemplate} />
  <p class="muted">Leave it empty for the default.</p>
</div>
<div class="field">
  <label for="{prefix}-suffix">Addition suffix</label>
  <input id="{prefix}-suffix" type="text" bind:value={form.additionSuffix} />
  <p class="muted">Leave it empty for the default.</p>
</div>
<div class="field">
  <label for="{prefix}-artist">Artist</label>
  <input id="{prefix}-artist" type="text" bind:value={form.artist} />
</div>
<div class="field">
  <label for="{prefix}-date">Content date</label>
  <input id="{prefix}-date" type="text" placeholder="2026-11-01" bind:value={form.contentDate} />
</div>
<div class="field">
  <label for="{prefix}-description">Description</label>
  <textarea id="{prefix}-description" rows="3" bind:value={form.description}></textarea>
</div>

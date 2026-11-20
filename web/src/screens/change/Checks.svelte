<script lang="ts">
  import type { SelfCheck } from '../../lib/types'

  let { checks }: { checks: SelfCheck[] } = $props()
</script>

<section aria-labelledby="checks-heading">
  <h2 id="checks-heading">Self-checks</h2>
  {#if checks.length === 0}
    <p>No self-check has run for this change.</p>
  {:else}
    <table>
      <thead>
        <tr>
          <th scope="col">Check</th>
          <th scope="col">Expected</th>
          <th scope="col">Actual</th>
          <th scope="col">Tolerance</th>
          <th scope="col">Result</th>
        </tr>
      </thead>
      <tbody>
        {#each checks as check (check.name)}
          <tr>
            <th scope="row">{check.name}</th>
            <td>{check.expected.toFixed(1)}</td>
            <td>{check.actual.toFixed(1)}</td>
            <td>±{check.tolerance.toFixed(1)}</td>
            <td>
              <span class="state" data-state={check.passed ? 'passed' : 'failed'}>
                {check.passed ? 'passed' : 'failed'}
              </span>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</section>

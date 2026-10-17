<script lang="ts">
import { api, type LogEntry } from "./api";

export let refreshKey = 0;
let entries: LogEntry[] = [];

$: if (refreshKey >= 0) load();

async function load() {
  try { entries = await api.logs(300); } catch { entries = []; }
}
function lvl(l: string) {
  return l === "error" ? "err" : l === "warn" ? "warn" : "ok";
}
function lvlLabel(l: string) {
  return l === "error" ? "ПОМИЛКА" : l === "warn" ? "УВАГА" : "ІНФО";
}
</script>

<div style="max-width: 1100px;">
  <h2 style="margin-top:4px;">Журнал операцій</h2>
  <div style="margin-bottom:10px;"><button class="btn" onclick={load}>Оновити</button></div>
  {#if !entries.length}
    <div class="empty">Записів ще немає.</div>
  {:else}
    <table class="plain">
      <thead><tr><th>Час</th><th>Рівень</th><th>Дія</th><th>Повідомлення</th></tr></thead>
      <tbody>
        {#each entries as e (e.id)}
          <tr class={e.level === "error" ? "row-failed" : ""}>
            <td class="mono">{new Date(e.created_at).toLocaleString("uk-UA")}</td>
            <td><span class="badge {lvl(e.level)}">{lvlLabel(e.level)}</span></td>
            <td>{e.action}</td>
            <td>{e.message}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

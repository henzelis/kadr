<script lang="ts">
export let view: string;
export let stats: Record<string, number>;
export let roots: { id: string; path: string }[];
export let setView: (v: string) => void;

const views = [
  { id: "library", label: "Усі медіа", icon: "▦", countKey: "total" },
  { id: "review", label: "До перевірки", icon: "◍", countKey: "needs_review" },
  { id: "nodate", label: "Без дати", icon: "◷", countKey: "unknown_date" },
  { id: "nogeo", label: "Без геолокації", icon: "◎", countKey: "no_geo" },
  { id: "dups", label: "Дублікати", icon: "⧉", countKey: "duplicate_groups" },
  { id: "plan", label: "План сортування", icon: "⇄", countKey: null },
  { id: "log", label: "Журнал", icon: "☰", countKey: null },
  { id: "settings", label: "Налаштування", icon: "⚙", countKey: null },
];
</script>

<nav class="sidebar" aria-label="Розділи">
  {#each views as v}
    <button class="nav-item" class:active={view === v.id} onclick={() => setView(v.id)}>
      <span aria-hidden="true">{v.icon}</span>
      <span>{v.label}</span>
      {#if v.countKey && stats?.[v.countKey]}
        <span class="nav-count">{stats[v.countKey]}</span>
      {/if}
    </button>
  {/each}
  <hr />
  <div class="roots">
    <div style="margin-bottom:6px;font-weight:600;">Джерела</div>
    {#each roots as r (r.id)}
      <div class="root-chip" title={r.path}>
        <span aria-hidden="true">📁</span><span class="path">{r.path}</span>
      </div>
    {/each}
    {#if !roots.length}
      <div class="kbd-hint">Ще не додано. У «Налаштуваннях».</div>
    {/if}
  </div>
</nav>

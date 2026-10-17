<script lang="ts">
import { open } from "@tauri-apps/plugin-dialog";
import { api, type AppSettings, type ScanProgress, type SourceRoot } from "./api";

export let view: string;
export let stats: Record<string, number>;
export let roots: SourceRoot[];
export let settings: AppSettings | null = null;
export let setView: (v: string) => void;
export let onRefresh: () => Promise<void> = async () => {};
export let openSettings: () => void = () => {};
export let scanProgress: ScanProgress | null = null;

const mediaNav = [
  { id: "library", label: "Усі медіа", icon: "▦", countKey: "total" },
  { id: "review", label: "До перевірки", icon: "◷", countKey: "needs_review" },
  { id: "nodate", label: "Без дати", icon: "◷", countKey: "unknown_date" },
  { id: "nogeo", label: "Без геолокації", icon: "⌖", countKey: "no_geo" },
  { id: "dups", label: "Дублікати", icon: "▧", countKey: "duplicate_groups" },
];
const workNav = [
  { id: "plan", label: "План сортування", icon: "⇢", countKey: "total" },
  { id: "log", label: "Журнал операцій", icon: "↺", countKey: null },
];

function pct(a: number, b: number): number {
  return b > 0 ? Math.round((a / b) * 100) : 0;
}
async function addSource() {
  const dir = await open({ directory: true, title: "Виберіть вихідну папку з фото/відео" });
  if (!dir) return;
  await api.addRoot(String(dir));
  await onRefresh();
}
async function changeDest() {
  const dir = await open({ directory: true, title: "Виберіть папку архіву (куди складати)" });
  if (!dir || !settings) return;
  try {
    await api.updateSettings({ ...settings, destination_root: String(dir) });
    await onRefresh();
  } catch { /* покаже toast з App */ }
}

function scanPct(): number {
  const p = scanProgress;
  if (!p || p.finished) return stats.total ? 100 : 0;
  return p.total > 0 ? Math.min(99, Math.round((p.processed * 100) / p.total)) : 0;
}
</script>

<aside class="sidebar">
  <div class="workspace-label">Медіатека</div>
  <nav class="nav-list" aria-label="Розділи медіатеки">
    {#each mediaNav as v (v.id)}
      <button class="nav-item" class:active={view === v.id} onclick={() => setView(v.id)}>
        <span class="nav-icon" aria-hidden="true">{v.icon}</span>
        <span class="nav-label">{v.label}</span>
        {#if v.countKey && stats?.[v.countKey]}
          <span class="nav-count">{stats[v.countKey]}</span>
        {/if}
      </button>
    {/each}
  </nav>
  <div class="sidebar-divider"></div>
  <div class="workspace-label">Робочий процес</div>
  <nav class="nav-list">
    {#each workNav as v (v.id)}
      <button class="nav-item" class:active={view === v.id} onclick={() => setView(v.id)}>
        <span class="nav-icon" aria-hidden="true">{v.icon}</span>
        <span class="nav-label">{v.label}</span>
        {#if v.id === "plan" && stats?.total}
          <span class="nav-count">{stats.total}</span>
        {/if}
      </button>
    {/each}
  </nav>
  <div class="sidebar-divider"></div>
  <div class="folder-card">
    <div class="folder-card-head">
      <span>Папки</span>
      <button class="edit-text" style="font-size:15px" onclick={openSettings}
        aria-label="Редагувати папки" title="Редагувати папки">···</button>
    </div>
    <div class="folder-row">
      <span>↳</span>
      <div>
        <div style="font-size:10px;color:var(--muted);margin-bottom:3px">Джерело</div>
        <div class="folder-path">
          {roots.length ? roots[0].path : "—"}
          {#if roots.length > 1}<span style="color:var(--muted)"> · {roots.length} папки</span>{/if}
        </div>
      </div>
    </div>
    <div class="folder-row">
      <span>↳</span>
      <div>
        <div style="font-size:10px;color:var(--muted);margin-bottom:3px">Цільовий архів</div>
        <div class="folder-path">{settings?.destination_root || "—"}</div>
      </div>
    </div>
    <div class="folder-actions">
      <button class="tiny-button" onclick={addSource}>
        {roots.length ? "Додати джерело" : "Вибрати джерело"}
      </button>
      <button class="tiny-button" onclick={changeDest}>Змінити ціль</button>
    </div>
  </div>
  <div class="sidebar-bottom">
    <div class="scan-line">
      <span>Індекс медіатеки</span>
      {#if scanProgress && !scanProgress.finished}
        <strong style="color:var(--accent-strong)">{scanPct()}%</strong>
      {:else}
        <strong style="color:var(--accent-strong)">{stats.total ? "100%" : "—"}</strong>
      {/if}
    </div>
    <div class="progress-track">
      <div class="progress-fill" style="width:{scanProgress && !scanProgress.finished ? scanPct() : stats.total ? 100 : 0}%"></div>
    </div>
    <div class="scan-caption">
      {#if scanProgress && !scanProgress.finished}
        Оброблено {scanProgress.processed} з {scanProgress.total || scanProgress.found}…
      {:else if stats.total}
        {stats.total} файлів · {pct(stats.has_date ?? stats.total - (stats.unknown_date ?? 0), stats.total)}% дату ·
        {pct(stats.total - (stats.no_geo ?? 0), stats.total)}% GPS
      {:else}
        Індекс порожній
      {/if}
      {#if scanProgress && !scanProgress.finished}
        <div style="margin-top:6px">
          <button class="tiny-button" onclick={() => api.cancelScan()}>Скасувати сканування</button>
        </div>
      {/if}
    </div>
  </div>
</aside>

<script lang="ts">
import "./app.css";
import { onMount } from "svelte";
import {
  api, fmtBytes, fmtDate,
  onScanProgress, onApplyProgress,
  type AppSettings, type ApplyProgress, type BuiltPlan, type LibraryStats,
  type MediaFilter, type MediaItem, type ScanProgress, type SourceRoot, type ToolStatus,
} from "./lib/api";
import Sidebar from "./lib/Sidebar.svelte";
import SettingsView from "./lib/SettingsView.svelte";
import LogView from "./lib/LogView.svelte";
import PlanView from "./lib/PlanView.svelte";

let view = "library";
let stats: Record<string, number> = {};
let roots: SourceRoot[] = [];
let tools: ToolStatus | null = null;
let settings: AppSettings | null = null;

let items: MediaItem[] = [];
let total = 0;
let offset = 0;
const PAGE = 120;
let filterSearch = "";
let kindFilter = "";
let sortSel = "date";
let nogeoMode = "no_geo"; // or coords_only
let selected: MediaItem | null = null;
let selection = new Set<string>();
let thumbs = new Map<string, string>();
let groups: MediaItem[][] = [];
let plan: BuiltPlan | null = null;
let applyProgress: ApplyProgress | null = null;
let scanProgress: ScanProgress | null = null;
let toasts: { msg: string; kind: string }[] = [];
let logKey = 0;
let theme = localStorage.getItem("kadr-theme") ?? "auto";

function notify(msg: string, kind: "ok" | "err" = "ok") {
  toasts = [...toasts, { msg, kind }];
  setTimeout(() => (toasts = toasts.filter((t) => t.msg !== msg)), 4000);
}

function currentFilter(): MediaFilter {
  const f: MediaFilter = {
    search: filterSearch || null,
    kind: kindFilter || null,
    sort: sortSel,
    limit: PAGE,
    offset,
  };
  if (view === "review") f.needs_review = true;
  if (view === "nodate") f.date_status = "unknown";
  if (view === "nogeo") f.place_status = nogeoMode;
  return f;
}

async function loadLibrary(append = false) {
  if (view !== "library" && view !== "review" && view !== "nodate" && view !== "nogeo") return;
  try {
    const page = await api.queryMedia(currentFilter());
    items = append ? [...items, ...page.items] : page.items;
    total = page.total;
    if (!append) offset = 0;
    offset = items.length;
    loadThumbs(page.items);
  } catch (e) {
    notify(String(e), "err");
  }
}
async function loadStats() {
  try { stats = (await api.stats()) as unknown as Record<string, number>; } catch {}
}
async function loadGroups() {
  if (view !== "dups") return;
  try { groups = await api.dupGroups(); } catch (e) { notify(String(e), "err"); }
}

let thumbQueue: string[] = [];
let thumbWorkers = 0;
function loadThumbs(batch: MediaItem[]) {
  for (const m of batch) {
    if (m.media_kind === "sidecar") continue;
    if (!thumbs.has(m.id)) {
      thumbs.set(m.id, "");
      thumbQueue.push(m.id);
    }
  }
  pumpThumbs();
}
function pumpThumbs() {
  while (thumbWorkers < 4 && thumbQueue.length) {
    const id = thumbQueue.shift()!;
    thumbWorkers++;
    api.thumbB64(id)
      .then((b) => {
        thumbs.set(id, b ?? "");
        thumbs = new Map(thumbs);
      })
      .catch(() => thumbs.set(id, ""))
      .finally(() => { thumbWorkers--; pumpThumbs(); });
  }
}

async function runScan() {
  try {
    scanProgress = { job_id: "", found: 0, processed: 0, skipped: 0, errors: 0, total: 0, elapsed_ms: 0, current: "", finished: false, cancelled: false, error: null };
    await api.startScan();
    notify("Сканування запущено");
  } catch (e) {
    scanProgress = null;
    notify(String(e), "err");
  }
}

function scanPct(p: ScanProgress): number {
  return p.total > 0 ? Math.min(99, Math.round((p.processed * 100) / p.total)) : 0;
}
function scanEta(p: ScanProgress): string {
  if (!p.total || !p.processed || !p.elapsed_ms) return "";
  const rate = p.processed / (p.elapsed_ms / 1000);
  if (rate <= 0) return "";
  const rem = Math.max(0, Math.round((p.total - p.processed) / rate));
  return rem > 90 ? `${Math.round(rem / 60)} хв` : `${rem} с`;
}

async function buildPlan(mode?: string) {
  try {
    plan = await api.buildPlan(mode);
    const map = new Map<string, MediaItem>();
    for (const it of plan.items) {
      const m = items.find((x) => x.id === it.media_id);
      if (m) map.set(it.media_id, m);
    }
    for (const m of items) map.set(m.id, m);
    mediaById = map;
  } catch (e) {
    notify(String(e), "err");
  }
}
let mediaById = new Map<string, MediaItem>();

async function gotoPlan() {
  view = "plan";
  if (!plan) await buildPlan();
}

function select(m: MediaItem, e: MouseEvent | KeyboardEvent) {
  if (e.ctrlKey || e.metaKey) {
    if (selection.has(m.id)) selection.delete(m.id);
    else selection.add(m.id);
    selection = new Set(selection);
    selected = null;
  } else {
    selected = m;
  }
}

let editPlace = "";
let editDate = "";
$: if (selected) {
  editPlace = selected.place_label ?? "";
  editDate = selected.captured_at_local ?? "";
}

async function applyPlace(ids: string[]) {
  try {
    await api.setManualPlace(ids, editPlace);
    notify(`Місце оновлено (${ids.length})`);
    await loadLibrary();
    selected = null;
    await loadStats();
  } catch (e) { notify(String(e), "err"); }
}
async function applyDate() {
  if (!selected) return;
  try {
    await api.setDateOverride(selected.id, editDate);
    notify("Дату оновлено");
    await loadLibrary();
    await loadStats();
  } catch (e) { notify(String(e), "err"); }
}

function kindLabel(k: string) {
  return k === "photo" ? "фото" : k === "video" ? "відео" : k === "sidecar" ? "супровід" : "інше";
}
function dateBadge(m: MediaItem): [string, string] {
  switch (m.date_status) {
    case "confirmed": return ["ok", m.date_source === "container" ? "контейнер" : "EXIF"];
    case "fallback": return ["warn", "дата ФС"];
    case "needs_review": return ["warn", "перевірити"];
    default: return ["muted", "без дати"];
  }
}
function geoBadge(m: MediaItem): [string, string] {
  if (m.place_status === "resolved") return ["ok", m.place_label ?? "місце"];
  if (m.place_status === "coords_only") return ["warn", "GPS без назви"];
  if (m.place_status === "needs_review") return ["warn", "GPS?"];
  return ["muted", "без GPS"];
}
function baseName(p: string) { return p.split(/[\\/]/).pop() ?? p; }

function setView(v: string) {
  view = v;
  selected = null;
  offset = 0;
  if (v === "dups") loadGroups();
  else if (v === "log") logKey++;
  else loadLibrary();
}

function setTheme(t: string) {
  theme = t;
  localStorage.setItem("kadr-theme", t);
  document.documentElement.dataset.theme = t;
}

onMount(async () => {
  document.documentElement.dataset.theme = theme;
  await onScanProgress((p) => {
    scanProgress = p;
    if (p.finished) {
      if (p.error) notify(p.error, "err");
      else notify(`Сканування завершено: ${p.processed} файлів`, "ok");
      loadStats();
      loadLibrary();
    }
  });
  await onApplyProgress((p) => {
    applyProgress = p;
    if (p.finished) {
      if (p.error) notify(p.error, "err");
      else notify(`Готово: ${p.done}, помилок: ${p.failed}`, p.failed ? "err" : "ok");
      loadStats();
      if (plan) api.getPlan(plan.plan_id).then(([items2, summary]) => (plan = { ...plan!, items: items2, summary }));
    }
  });
  try {
    tools = await api.toolStatus();
    settings = await api.getSettings();
    roots = await api.listRoots();
    await loadStats();
    await loadLibrary();
  } catch (e) {
    notify(String(e), "err");
  }
});

function loadMore() {
  loadLibrary(true);
}
</script>

<div class="app-root">
  <header class="titlebar">
    <div class="brand">
      <div class="brand-mark">K</div>
      <div>
        <div class="brand-name">Kadr</div>
        <div class="brand-sub">локальний медіаархіватор · мережа вимкнена</div>
      </div>
    </div>
    <div style="flex:1;"></div>
    {#if tools && (!tools.exiftool_path || !tools.ffprobe_path)}
      <span class="badge warn" title="Частина метаданих буде недоступна">⚠ допоміжні інструменти</span>
    {/if}
    <button class="btn" onclick={runScan} disabled={!!scanProgress && !scanProgress.finished}>
      ▶ Сканувати
    </button>
    <button class="btn primary" onclick={gotoPlan}>Переглянути план</button>
    <button class="btn ghost" title="Світла/темна тема"
      onclick={() => setTheme(theme === "dark" ? "light" : theme === "light" ? "auto" : "dark")}>
      {theme === "dark" ? "☾" : theme === "light" ? "☀" : "◐"}
    </button>
  </header>

  {#if scanProgress && !scanProgress.finished}
    <div class="banner">
      <span>⏳</span>
      <div class="scaninfo">
        <span>Сканування: {scanProgress.processed}{#if scanProgress.total} з {scanProgress.total}{/if} ·
          {scanProgress.found} знайдено · {scanProgress.skipped} пропущено ·
          {scanProgress.errors} помилок</span>
        {#if scanProgress.total > 0}
          <div class="pbar"><div class="pfill" style="width:{scanPct(scanProgress)}%"></div></div>
          <span class="pmeta">{scanPct(scanProgress)}% · лишилось ≈ {scanEta(scanProgress)}</span>
        {:else}
          <div class="pbar"><div class="pfill indet"></div></div>
        {/if}
        {#if scanProgress.current}<span class="pcur">{scanProgress.current}</span>{/if}
      </div>
      <button class="btn danger" onclick={() => api.cancelScan()}>Скасувати</button>
    </div>
  {:else if scanProgress && scanProgress.error}
    <div class="banner err"><span>⚠</span><span>{scanProgress.error}</span></div>
  {/if}

  <div class="main">
    <Sidebar {view} {stats} {roots} {setView} />

    <div class="content">
      {#if view === "library" || view === "review" || view === "nodate" || view === "nogeo"}
        <div class="topbar">
          <input class="search" placeholder="Пошук: назва, місце, шлях…"
            bind:value={filterSearch}
            onkeydown={(e) => e.key === "Enter" && loadLibrary()} />
          {#each [["", "усе"], ["photo", "фото"], ["video", "відео"]] as [k, label] (k)}
            <button class="chip" class:on={kindFilter === k}
              onclick={() => { kindFilter = k; loadLibrary(); }}>{label}</button>
          {/each}
          {#if view === "nogeo"}
            <button class="chip" class:on={nogeoMode === "no_geo"}
              onclick={() => { nogeoMode = "no_geo"; loadLibrary(); }}>GPS немає</button>
            <button class="chip" class:on={nogeoMode === "coords_only"}
              onclick={() => { nogeoMode = "coords_only"; loadLibrary(); }}>GPS є, назви немає</button>
          {/if}
          <select class="chip" bind:value={sortSel} onchange={() => loadLibrary()} aria-label="Сортування">
            <option value="date">за датою</option>
            <option value="name">за назвою</option>
            <option value="size">за розміром</option>
          </select>
          <span class="hint">{total} файлів</span>
        </div>
      {/if}

      <div class="body-wrap">
        <div class="scroll">
          {#if view === "library" || view === "review" || view === "nodate" || view === "nogeo"}
            {#if !items.length}
              <div class="empty">
                {roots.length
                  ? "Немає файлів за цим фільтром. Запустіть «Сканувати»."
                  : "Ласкаво просимо! Додайте вихідні папку та папку архіву в «Налаштуваннях», потім «Сканувати»."}
              </div>
            {:else}
              <div class="grid">
                {#each items as m (m.id)}
                  <div class="card" class:selected={selected?.id === m.id || selection.has(m.id)}
                    tabindex="0" role="button"
                    onclick={(e) => select(m, e)}
                    onkeydown={(e) => (e.key === "Enter" || e.key === " ") && select(m, e)}>
                    <div class="thumb">
                      {#if thumbs.get(m.id)}
                        <img src="data:image/jpeg;base64,{thumbs.get(m.id)}" alt={m.file_name} loading="lazy" />
                      {:else}
                        <span class="kind-ph" title="{kindLabel(m.media_kind)} без мініатюри">
                          {m.media_kind === "video" ? "🎬" : m.extension.toUpperCase().slice(0, 4)}
                        </span>
                      {/if}
                    </div>
                    <div class="meta">
                      <div class="fname" title={m.path}>{m.file_name}</div>
                      <div class="sub">
                        <span class="badge {dateBadge(m)[0]}">{dateBadge(m)[1]}</span>
                        <span class="badge {geoBadge(m)[0]}">{geoBadge(m)[1]}</span>
                      </div>
                    </div>
                  </div>
                {/each}
              </div>
              {#if items.length < total}
                <div style="text-align:center;margin:16px;">
                  <button class="btn" onclick={loadMore}>Завантажити ще ({total - items.length})</button>
                </div>
              {/if}
            {/if}
          {:else if view === "dups"}
            <h2 style="margin-top:4px;">Точні дублікати (SHA-256)</h2>
            {#if !groups.length}
              <div class="empty">Груп дублікатів не знайдено.
                <div style="margin-top:10px;"><button class="btn" onclick={() => api.ensureHashes().then(loadGroups)}>Дотиснути хеші</button></div>
              </div>
            {:else}
              {#each groups as g, gi (gi)}
                <div style="margin-bottom:14px;">
                  <span class="badge warn">група {gi + 1} · {g.length} файли · {fmtBytes(g[0].size_bytes)}</span>
                  {#each g as m (m.id)}
                    <div class="root-chip" title={m.path} role="button" tabindex="0"
                      onclick={() => select(m, { ctrlKey: false } as any)}
                      onkeydown={(e) => e.key === "Enter" && select(m, { ctrlKey: false } as any)}>
                      <span class="path">{m.path}</span><span class="hint">{fmtDate(m.captured_at_local)}</span>
                    </div>
                  {/each}
                </div>
              {/each}
            {/if}
          {:else if view === "plan"}
            <PlanView {plan} {buildPlan} refreshStats={loadStats} {applyProgress} {notify} {mediaById}
              destRoot={settings?.destination_root ?? ""} />
          {:else if view === "log"}
            <LogView refreshKey={logKey} />
          {:else if view === "settings" && settings}
            <SettingsView {settings} {roots} tools={tools!} />
          {/if}
        </div>

        {#if selected}
          <aside class="inspector">
            <h3>{selected.file_name}</h3>
            <div class="field"><label>Тип</label>
              <span class="val">{kindLabel(selected.media_kind)} · {selected.extension.toUpperCase()}
                {#if selected.video_codec} · {selected.video_codec}{/if}
                {#if selected.width} · {selected.width}×{selected.height}{/if}
                {#if selected.duration_ms} · {Math.round(selected.duration_ms / 1000)} c{/if}</span></div>
            <div class="field"><label>Шлях</label><span class="val mono">{selected.path}</span></div>
            <div class="field"><label>Розмір</label><span class="val">{fmtBytes(selected.size_bytes)}</span></div>
            <div class="field">
              <label>Дата зйомки ({selected.date_source ?? "немає"} · {selected.date_status})</label>
              <span class="val">{fmtDate(selected.captured_at_local)}</span>
              {#if selected.normalization_note}<span class="hint">{selected.normalization_note}</span>{/if}
              <div style="display:flex;gap:6px;margin-top:4px;">
                <input bind:value={editDate} placeholder="2025-08-12 14:03:22" style="flex:1;" />
                <button class="btn" onclick={applyDate}>Зберегти дату</button>
              </div>
            </div>
            <div class="field">
              <label>Місце ({selected.location_source ?? "немає"} · {selected.place_status})</label>
              {#if selected.latitude != null && selected.longitude != null}
                <span class="val">◎ GPS: {selected.latitude.toFixed(5)}, {selected.longitude.toFixed(5)}
                  {#if selected.altitude != null}(↑{selected.altitude} м){/if}</span>
              {/if}
              <input bind:value={editPlace} placeholder="напр. Київ" style="margin-top:4px;" />
              <div style="display:flex;gap:6px;margin-top:4px;">
                <button class="btn" onclick={() => applyPlace([selected!.id])}>Зберегти місце</button>
                {#if selection.size > 1}
                  <button class="btn ghost" onclick={() => applyPlace([...selection])}>
                    Для {selection.size} вибраних
                  </button>
                {/if}
              </div>
            </div>
            {#if mediaById.get(selected.id)}
              <!-- destination shown from last plan -->
            {/if}
            {#if plan}
              {@const pi = plan.items.find((p) => p.media_id === selected?.id)}
              {#if pi}
                <div class="field"><label>Цільовий шлях (план)</label>
                  <span class="val mono">{pi.destination_path}</span></div>
              {/if}
            {/if}
            {#if selected.is_sidecar}
              <div class="badge muted">супровідний файл — переноситься з основним</div>
            {/if}
            {#if selected.last_error}<div class="badge err">{selected.last_error}</div>{/if}
          </aside>
        {:else if selection.size}
          <aside class="inspector">
            <h3>Вибрано: {selection.size}</h3>
            <div class="field">
              <label>Спільне місце</label>
              <input bind:value={editPlace} placeholder="напр. Карпати" />
              <button class="btn" style="margin-top:6px;" onclick={() => applyPlace([...selection])}>
                Призначити місце
              </button>
            </div>
          </aside>
        {/if}
      </div>
    </div>
  </div>
</div>

<div class="toast-stack" aria-live="polite">
  {#each toasts as t}
    <div class="toast {t.kind}">{t.msg}</div>
  {/each}
</div>

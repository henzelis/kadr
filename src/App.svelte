<script lang="ts">
import "./app.css";
import { onMount } from "svelte";
import { open } from "@tauri-apps/plugin-dialog";
import { getVersion } from "@tauri-apps/api/app";
import {
  api, fmtBytes, fmtDate,
  onScanProgress, onApplyProgress,
  type AppSettings, type ApplyProgress, type BuiltPlan,
  type MediaFilter, type MediaItem, type ScanProgress, type SourceRoot, type ToolStatus,
} from "./lib/api";
import Sidebar from "./lib/Sidebar.svelte";
import Onboarding from "./lib/Onboarding.svelte";
import SettingsModal from "./lib/SettingsModal.svelte";
import LogView from "./lib/LogView.svelte";
import PlanView from "./lib/PlanView.svelte";

let view = "library";
let stats: Record<string, number> = {};
let roots: SourceRoot[] = [];
let tools: ToolStatus | null = null;
let settings: AppSettings | null = null;
let settingsOpen = false;
function openSettings() { settingsOpen = true; }
let version = "";

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

const titles: Record<string, [string, string, string]> = {
  library: ["Бібліотека", "Ваша медіатека", "Усі фотографії та відео з вибраних локальних папок."],
  review: ["До перевірки", "До перевірки", "Елементи, які потребують вашої уваги перед сортуванням."],
  nodate: ["Без дати", "Файли без дати", "Файли, для яких не знайдено надійної дати зйомки."],
  nogeo: ["Без геолокації", "Без геолокації", "Медіа без GPS-координат у доступних метаданих."],
  dups: ["Дублікати", "Дублікати", "Файли з однаковим контрольним хешем — перевірте перед дією."],
  plan: ["План сортування", "План сортування", "Перегляньте заплановані шляхи перед будь-якими змінами на диску."],
  log: ["Журнал операцій", "Журнал операцій", "Що Kadr робив із вашими файлами; звідси можна скасувати."],
};
$: head = titles[view] ?? titles.library;
$: datePct = stats.total ? Math.round(((stats.total - (stats.unknown_date ?? 0)) * 100) / stats.total) : 0;
$: gpsPct = stats.total ? Math.round(((stats.total - (stats.no_geo ?? 0)) * 100) / stats.total) : 0;

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
  if (!["library", "review", "nodate", "nogeo"].includes(view)) return;
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

async function refreshAll() {
  roots = await api.listRoots();
  settings = await api.getSettings();
  await loadStats();
}

async function addSource() {
  const dir = await open({ directory: true, title: "Оберіть папку з фото та відео" });
  if (!dir) return;
  try {
    await api.addRoot(String(dir));
    await refreshAll();
    if (!roots.length || (scanProgress && scanProgress.finished)) notify("Папку додано. Натисніть «Сканувати».");
  } catch (e) { notify(String(e), "err"); }
}

async function buildPlan(mode?: string) {
  try {
    plan = await api.buildPlan(mode);
    const map = new Map<string, MediaItem>();
    for (const m of items) map.set(m.id, m);
    mediaById = map;
  } catch (e) {
    notify(String(e), "err");
  }
}
let mediaById = new Map<string, MediaItem>();

async function gotoPlan() {
  view = "plan";
  selected = null;
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
let editingDate = false;
let editingPlace = false;
$: if (selected) {
  editPlace = selected.place_label ?? "";
  editDate = selected.captured_at_local ?? "";
}

async function applyPlace(ids: string[]) {
  try {
    await api.setManualPlace(ids, editPlace);
    notify(`Місце оновлено (${ids.length})`);
    editingPlace = false;
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
    editingDate = false;
    await loadLibrary();
    await loadStats();
  } catch (e) { notify(String(e), "err"); }
}

const RAW_EXTS = new Set(["arw", "cr2", "cr3", "nef", "raf", "dng", "rw2", "orf", "pef", "srw"]);
function isRaw(m: MediaItem) { return m.media_kind === "photo" && RAW_EXTS.has(m.extension); }
function fmtDur(ms: number | null): string {
  if (!ms) return "";
  const s = Math.round(ms / 1000);
  return `${String(Math.floor(s / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
}
function placeShort(m: MediaItem): string {
  if (m.place_label) return m.place_label;
  if (m.place_status === "coords_only") return "GPS записано";
  return "Без геолокації";
}
// One honest state badge per card (mockup's metadata-state)
function stateBadge(m: MediaItem): [string, string] {
  if (m.date_status === "unknown") return ["missing", "Без дати"];
  if (m.place_status === "coords_only" || m.place_status === "needs_review")
    return ["review", "Назву місця не визначено"];
  if (m.place_status === "no_geo") return ["missing", "Без GPS"];
  if (m.date_status === "fallback") return ["review", "Дата з файлової системи"];
  if (m.place_status === "resolved") return ["", "GPS знайдено"];
  return ["", "Метадані доступні"];
}
function dateChip(m: MediaItem): [string, string] {
  if (m.date_status === "confirmed") return ["", `✓ ${m.date_source === "container" ? "Контейнер" : "EXIF · DateTimeOriginal"}`];
  if (m.date_status === "fallback") return ["warn", "! Дата файлової системи"];
  if (m.date_status === "needs_review") return ["warn", "! Потребує перевірки"];
  return ["warn", "! Дата не знайдена"];
}
function placeSourceText(m: MediaItem): string {
  if (m.manual_place_override) return "Ручне призначення користувача";
  if (m.location_source === "geocoder") return "GPS + локальна геобаза";
  if (m.location_source) return m.location_source;
  return m.latitude != null ? "GPS є, назву не визначено" : "GPS не знайдено";
}

function setView(v: string) {
  view = v;
  selected = null;
  offset = 0;
  if (v === "dups") loadGroups();
  else if (v === "log") logKey++;
  else if (v !== "plan") loadLibrary();
}

function setTheme(t: string) {
  theme = t;
  localStorage.setItem("kadr-theme", t);
  document.documentElement.dataset.theme = t;
}

onMount(async () => {
  document.documentElement.dataset.theme = theme;
  try { version = await getVersion(); } catch { version = "0.4"; }
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
</script>

<div class="app-root">
  <header class="titlebar">
    <div class="brand">
      <div class="brand-mark">k.</div>
      <div>
        <div class="brand-name">kadr</div>
        <div class="brand-sub">Local media archive</div>
      </div>
    </div>
    <div class="top-center">
      <span class="online-dot"></span><span>Локальний режим</span>
      <span style="opacity:.45">·</span><span>Файли не завантажуються в хмару</span>
    </div>
    <div class="top-right">
      {#if tools && (!tools.exiftool_path || !tools.ffprobe_path)}
        <span class="tool-tag" title="Частина метаданих буде недоступна. Перевстановіть Kadr або вкажіть інструменти в налаштуваннях.">⚠ ІНСТРУМЕНТИ</span>
      {/if}
      <button class="icon-button" title="Світла/темна тема"
        onclick={() => setTheme(theme === "dark" ? "light" : theme === "light" ? "auto" : "dark")}>◐</button>
      <button class="icon-button" title="Налаштування" onclick={() => (settingsOpen = true)}>⚙</button>
      <button class="btn primary" onclick={addSource}><span>＋</span> Додати папки</button>
    </div>
  </header>

  <div class="layout">
    <Sidebar {view} {stats} {roots} {settings} {setView} {openSettings} onRefresh={refreshAll} {scanProgress} />

    <main class="main">
      <div class="main-header">
        <div>
          <div class="eyebrow"><span>Робоча область</span><span>›</span><span>{head[0]}</span></div>
          <h1>{head[1]}</h1>
          <div class="subtitle">{head[2]}</div>
        </div>
        <div class="hero-right">
        <ol class="steps">
          <li class:done={(stats.total ?? 0) > 0}><b>1</b>Сканування</li>
          <li class:done={!!settings?.destination_root}><b>2</b>Організація</li>
          <li class:done={view === "plan"}><b>3</b>Перевірка</li>
          <li><b>4</b>Підтвердження</li>
        </ol>
        <div class="header-actions">
          <button class="btn secondary" onclick={runScan}
            disabled={!!scanProgress && !scanProgress.finished}>↻ <span>Сканувати</span></button>
          {#if view === "plan"}
            <button class="btn secondary" onclick={() => setView("library")}>До медіатеки <span>→</span></button>
          {:else}
            <button class="btn primary" onclick={gotoPlan}>Переглянути план <span>→</span></button>
          {/if}
        </div>
        </div>
      </div>

      <div class="scroll">
        {#if ["library", "review", "nodate", "nogeo"].includes(view)}
          {#if view === "library" && !roots.length}
            <Onboarding {roots} {settings} onRefresh={refreshAll} onStart={runScan} />
          {:else}
            <div class="stats-row">
              <div class="stat-chip"><div class="stat-symbol">▦</div><div>
                <div class="stat-main">{stats.total ?? 0}</div>
                <div class="stat-label">файлів проіндексовано</div></div></div>
              <div class="stat-chip"><div class="stat-symbol">◷</div><div>
                <div class="stat-main">{datePct}%</div>
                <div class="stat-label">мають дату зйомки</div></div></div>
              <div class="stat-chip"><div class="stat-symbol">⌖</div><div>
                <div class="stat-main">{gpsPct}%</div>
                <div class="stat-label">мають GPS-координати</div></div></div>
            </div>

            <div class="source-bar">
              <div class="source-icon">▣</div>
              <div class="source-details">
                <div class="source-label">Джерело</div>
                <div class="source-path">{roots[0]?.path ?? "—"}
                  {#if roots.length > 1}<span style="color:var(--muted);font-weight:500">· {roots.length} папки</span>{/if}</div>
              </div>
              <div class="source-arrow">→</div>
              <div class="source-icon">▤</div>
              <div class="source-details">
                <div class="source-label">Структура призначення</div>
                <div class="source-path">Рік <span style="color:var(--muted)">/</span> Місяць <span style="color:var(--muted)">/</span> Місце</div>
              </div>
              <button class="icon-button" title="Налаштувати структуру" style="margin-left:auto;flex:0 0 32px"
                onclick={() => (settingsOpen = true)}>⚙</button>
            </div>

            <div class="view-controls">
              <div class="filter-pills" role="group" aria-label="Тип медіа">
                {#each [["", `Усі ${stats.total ?? ""}`.trim()], ["photo", "Фото"], ["video", "Відео"]] as [k, label] (k)}
                  <button class="filter-pill" class:active={kindFilter === k}
                    onclick={() => { kindFilter = k; loadLibrary(); }}>{label}</button>
                {/each}
                {#if view === "nogeo"}
                  <button class="filter-pill" class:active={nogeoMode === "no_geo"}
                    onclick={() => { nogeoMode = "no_geo"; loadLibrary(); }}>GPS немає</button>
                  <button class="filter-pill" class:active={nogeoMode === "coords_only"}
                    onclick={() => { nogeoMode = "coords_only"; loadLibrary(); }}>GPS є, назви немає</button>
                {/if}
              </div>
              <div class="view-right">
                <div class="search-wrap"><span class="search-icon">⌕</span>
                  <input class="search" placeholder="Пошук у медіатеці…" aria-label="Пошук у медіатеці"
                    bind:value={filterSearch}
                    onkeydown={(e) => e.key === "Enter" && loadLibrary()} /></div>
                <select class="sort-select" bind:value={sortSel} onchange={() => loadLibrary()} aria-label="Сортування">
                  <option value="date">Дата зйомки ↓</option>
                  <option value="name">Назва A–Я</option>
                  <option value="size">Розмір файлу</option>
                </select>
              </div>
            </div>

            <div class="section-heading"><span>{head[1] === "Ваша медіатека" ? "Нещодавні медіа" : head[1]}</span>
              <small>Показано {items.length} з {total}</small></div>

            {#if !items.length}
              <div class="empty-state"><strong>Нічого не знайдено</strong>
                Запустіть «Сканувати» або змініть фільтри.</div>
            {:else}
              <div class="media-grid">
                {#each items as m (m.id)}
                  <article class="media-card" class:selected={selected?.id === m.id || selection.has(m.id)}
                    tabindex="0" role="button" aria-label="{m.file_name}, {placeShort(m)}"
                    onclick={(e) => select(m, e)}
                    onkeydown={(e) => (e.key === "Enter" || e.key === " ") && select(m, e)}>
                    <div class="media-visual">
                      {#if thumbs.get(m.id)}
                        <img src="data:image/jpeg;base64,{thumbs.get(m.id)}" alt={m.file_name} loading="lazy" />
                      {:else}
                        <span class="kind-ph">{m.media_kind === "video" ? "ВІДЕО" : m.extension.toUpperCase().slice(0, 4)}</span>
                      {/if}
                      <span class="kind-badge"><span class="tiny-dot"></span>
                        {m.media_kind === "video" ? "ВІДЕО" : "ФОТО"}{#if isRaw(m)} · RAW{/if}</span>
                      <span class="selection-check">✓</span>
                      {#if m.duration_ms}<span class="duration">{fmtDur(m.duration_ms)}</span>{/if}
                    </div>
                    <div class="media-info">
                      <div class="media-name-row">
                        <span class="media-name" title={m.file_name}>{m.file_name}</span>
                        <span class="format-tag">{m.extension.toUpperCase()}</span>
                      </div>
                      <div class="media-sub">
                        <span>◷</span><span>{m.captured_at_local ? fmtDate(m.captured_at_local).slice(0, 10) : "Дата невідома"}</span>
                        <span style="opacity:.5">·</span><span class="place">{placeShort(m)}</span>
                      </div>
                      <span class="metadata-state {stateBadge(m)[0]}">
                        <span class="state-dot"></span>{stateBadge(m)[1]}</span>
                    </div>
                  </article>
                {/each}
              </div>
              {#if items.length < total}
                <div style="text-align:center;margin:16px;">
                  <button class="btn secondary" onclick={() => loadLibrary(true)}>
                    Показати ще ({total - items.length})
                  </button>
                </div>
              {/if}
            {/if}
          {/if}
        {:else if view === "dups"}
          {#if !groups.length}
            <div class="empty-state"><strong>Груп дублікатів не знайдено</strong>
              Дублікати шукаються за SHA-256.
              <div style="margin-top:10px;"><button class="btn secondary"
                onclick={() => api.ensureHashes().then(loadGroups)}>Обчислити хеші</button></div>
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
            destRoot={settings?.destination_root ?? ""} onBack={() => setView("library")} />
        {:else if view === "log"}
          <LogView refreshKey={logKey} />
        {/if}
      </div>
    </main>

    <aside class="inspector" id="inspector">
      {#if selected}
        {@const pi = plan?.items.find((p) => p.media_id === selected?.id)}
        <div class="inspector-head">
          <div><div class="inspector-title">Властивості файла</div>
            <div class="inspector-kicker">Деталі вибраного елемента</div></div>
          <button class="icon-button" title="Очистити вибір" onclick={() => (selected = null)}>×</button>
        </div>
        <div class="photo-preview">
          {#if thumbs.get(selected.id)}
            <img src="data:image/jpeg;base64,{thumbs.get(selected.id)}" alt={selected.file_name} />
          {:else}
            <span>«{selected.extension.toUpperCase()}»</span>
          {/if}
        </div>
        <div class="detail-filename">{selected.file_name}</div>
        <div class="detail-path">{selected.path}</div>

        <div class="detail-section">
          <div class="detail-section-title"><span>Дата й час</span>
            <button class="edit-text" onclick={() => (editingDate = !editingDate)}>Змінити</button></div>
          <div class="detail-row"><div class="detail-label">Дата зйомки</div>
            <div class="detail-value">{selected.captured_at_local ? fmtDate(selected.captured_at_local) : "Невідома дата"}</div></div>
          <div class="detail-row"><div class="detail-label">Джерело дати</div>
            <div class="detail-value"><span class="source-chip {dateChip(selected)[0] === "warn" ? "warn" : ""}">{dateChip(selected)[1]}</span></div></div>
          {#if selected.normalization_note}<div class="hint">{selected.normalization_note}</div>{/if}
          {#if editingDate}
            <div class="inline-edit">
              <input bind:value={editDate} placeholder="2025-08-12 14:03:22" aria-label="Нова дата" />
              <button class="btn primary" onclick={applyDate}>Зберегти</button>
            </div>
          {/if}
        </div>

        <div class="detail-section">
          <div class="detail-section-title"><span>Місце зйомки</span>
            <button class="edit-text" onclick={() => (editingPlace = !editingPlace)}>Редагувати</button></div>
          <div class="detail-row"><div class="detail-label">Визначене місце</div>
            <div class="detail-value">{selected.place_label ?? (selected.latitude != null ? "Назву місця не визначено" : "Без геолокації")}</div></div>
          <div class="detail-row"><div class="detail-label">Джерело</div>
            <div class="detail-value">{placeSourceText(selected)}</div></div>
          {#if selected.latitude != null && selected.longitude != null}
            <div class="gps-panel">
              <div class="gps-pin">⌖</div>
              <div>
                <div class="gps-coord">{selected.latitude.toFixed(4)}° {selected.latitude >= 0 ? "N" : "S"}, {selected.longitude.toFixed(4)}° {selected.longitude >= 0 ? "E" : "W"}</div>
                <div class="gps-caption">GPS-координати з метаданих</div>
              </div>
            </div>
          {:else}
            <div class="gps-panel"><div class="gps-pin">⌖</div>
              <div><div class="gps-caption">У метаданих GPS-координат не знайдено</div></div></div>
          {/if}
          {#if editingPlace}
            <div class="inline-edit">
              <input bind:value={editPlace} placeholder="напр. Карпати" aria-label="Назва місця" />
              <button class="btn primary" onclick={() => applyPlace([selected!.id])}>Зберегти</button>
            </div>
            {#if selection.size > 1}
              <button class="btn ghost" style="margin-top:6px;" onclick={() => applyPlace([...selection])}>
                Застосувати до {selection.size} вибраних
              </button>
            {/if}
          {/if}
        </div>

        <div class="detail-section">
          <div class="detail-section-title"><span>Файл</span></div>
          <div class="detail-row"><div class="detail-label">Формат</div>
            <div class="detail-value">{selected.extension.toUpperCase()} · {selected.media_kind === "video" ? "Відео" : "Зображення"}{#if selected.video_codec} · {selected.video_codec.toUpperCase()}{/if}</div></div>
          <div class="detail-row"><div class="detail-label">Розмір</div>
            <div class="detail-value">{fmtBytes(selected.size_bytes)}</div></div>
          <div class="detail-row"><div class="detail-label">Роздільність</div>
            <div class="detail-value">{selected.width ? `${selected.width} × ${selected.height}${selected.duration_ms ? ` · ${fmtDur(selected.duration_ms)}` : ""}` : "—"}</div></div>
          {#if selected.sha256}
            <div class="detail-row"><div class="detail-label">SHA-256</div>
              <div class="detail-value mono" style="font-size:9px">{selected.sha256.slice(0, 4)}…{selected.sha256.slice(-4)}</div></div>
          {/if}
          {#if selected.is_sidecar}
            <div class="badge muted" style="margin-top:6px;">супровідний файл — переноситься з основним</div>
          {/if}
          {#if selected.last_error}<div class="badge err" style="margin-top:6px;">{selected.last_error}</div>{/if}
        </div>

        {#if pi}
          <div class="destination-box">
            <div class="destination-head"><span>Цільовий шлях</span><span>✓ Preview</span></div>
            <div class="destination-path">{pi.destination_path}</div>
            <div class="destination-hint">Шлях сформовано за метаданими. Вихідний файл не змінюється під час попереднього перегляду.</div>
          </div>
        {/if}
      {:else if selection.size}
        <div class="inspector-head">
          <div><div class="inspector-title">Вибрано: {selection.size}</div>
            <div class="inspector-kicker">Ctrl+клік, щоб додати чи зняти файл</div></div>
          <button class="icon-button" title="Зняти вибір" onclick={() => (selection = new Set())}>×</button>
        </div>
        <div class="detail-section">
          <div class="detail-section-title"><span>Спільне місце зйомки</span></div>
          <div class="inline-edit">
            <input bind:value={editPlace} placeholder="напр. Карпати" aria-label="Назва місця" />
            <button class="btn primary" onclick={() => applyPlace([...selection])}>Призначити</button>
          </div>
        </div>
      {:else}
        <div class="inspector-head">
          <div><div class="inspector-title">Властивості файла</div>
            <div class="inspector-kicker">Деталі вибраного елемента</div></div>
        </div>
        <div class="empty-state" style="padding:30px 12px;"><strong>Файл не вибрано</strong>
          Натисніть на картку медіа, щоб побачити дату, GPS і куди файл потрапить у архіві.</div>
      {/if}
    </aside>
  </div>

  <footer class="footer">
    <div class="footer-left">
      {#if scanProgress && !scanProgress.finished}
        <span>● <span style="color:var(--amber);font-weight:700">Сканування…</span></span>
        <span>·</span><span>{scanProgress.processed} оброблено</span>
      {:else if scanProgress?.error}
        <span>● <span style="color:var(--red);font-weight:700">Помилка сканування</span></span>
      {:else}
        <span>● <span style="color:var(--accent-strong);font-weight:700">Готово до роботи</span></span>
        <span>·</span><span>{stats.total ?? 0} файлів у локальному індексі</span>
      {/if}
    </div>
    <div class="footer-right">
      <span>SQLite · Локальний індекс</span><span>·</span><span>Kadr {version}</span>
    </div>
  </footer>
</div>

{#if settingsOpen && settings}
  <SettingsModal settings={settings} {roots} tools={tools ?? { exiftool_path: null, exiftool_version: null, ffprobe_path: null, ffprobe_version: null }}
    onSaved={(s) => (settings = s)} onClose={() => (settingsOpen = false)} onRefresh={refreshAll} />
{/if}

{#each toasts as t}
  <div class="toast" class:err={t.kind === "err"}>{t.msg}</div>
{/each}

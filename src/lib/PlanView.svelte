<script lang="ts">
import { save } from "@tauri-apps/plugin-dialog";
import {
  api, fmtBytes, fmtDate,
  type ApplyProgress, type BuiltPlan, type PlanItem,
} from "./api";

export let plan: BuiltPlan | null;
export let buildPlan: (mode?: string) => Promise<void>;
export let refreshStats: () => void;
export let applyProgress: ApplyProgress | null;
export let notify: (msg: string, kind?: "ok" | "err") => void;
export let mediaById: Map<string, any>;
export let destRoot = "";
export let onBack: () => void = () => {};

function itemCap(it: PlanItem) {
  return mediaById?.get(it.media_id)?.captured_at_local ?? null;
}
function itemPlace(it: PlanItem) {
  const m = mediaById?.get(it.media_id);
  if (!m) return "—";
  if (m.place_label) return m.place_label;
  if (m.latitude != null) return `${m.latitude.toFixed(4)}, ${m.longitude.toFixed(4)}`;
  return "Без геолокації";
}

let mode: "copy" | "move" = "copy";
let planSearch = "";
let confirmOpen = false;
let editing: PlanItem | null = null;
let editPlace = "";
let editDate = "";
let busy = false;

$: applying = applyProgress && !applyProgress.finished;
$: doneAll = applyProgress && applyProgress.finished && !applyProgress.error;
$: filtered = (plan?.items ?? []).filter((it) => {
  const q = planSearch.trim().toLowerCase();
  return !q || it.source_path.toLowerCase().includes(q) || it.destination_path.toLowerCase().includes(q);
});

const statusBadge: Record<string, [string, string]> = {
  pending: ["warn", "Перевірити"],
  excluded: ["warn", "виключено"],
  done: ["ready", "Готово"],
  skipped: ["warn", "пропущено"],
  failed: ["err", "помилка"],
  undone: ["ready", "скасовано (undo)"],
  needs_user: ["warn", "потрібне рішення"],
};

async function rebuild() {
  busy = true;
  try { await buildPlan(mode); } finally { busy = false; }
}
async function setMode(next: "copy" | "move") {
  if (next === mode) return;
  if (next === "move") {
    if (!confirm("Переміщення ВИДАЛИТЬ оригінали після успішної перевірки копії.\nПродовжити?")) return;
  }
  mode = next;
  if (plan) {
    try { await api.setPlanMode(plan.plan_id, mode); await rebuild(); } catch (e) { notify(String(e), "err"); }
  }
}
async function doApply() {
  confirmOpen = false;
  if (!plan) return;
  try {
    await api.applyPlan(plan.plan_id, true);
  } catch (e) { notify(String(e), "err"); }
}
async function undo() {
  if (!plan) return;
  if (!confirm("Повернути файли? Готові копії/переміщення будуть скасовані, якщо цільові файли не змінювались ззовні.")) return;
  try { await api.undoOperation(plan.plan_id); } catch (e) { notify(String(e), "err"); }
}
async function retry() {
  if (!plan) return;
  try {
    const n = await api.retryFailed(plan.plan_id);
    notify(`Повторено елементів: ${n}`);
    await rebuild();
  } catch (e) { notify(String(e), "err"); }
}
async function exportCsv(format: string) {
  if (!plan) return;
  const path = await save({
    defaultPath: `kadr-plan-${format}.${format}`,
    filters: [{ name: "Звіт", extensions: [format] }],
  });
  if (!path) return;
  try {
    const n = await api.exportReport(plan.plan_id, String(path), format);
    notify(`Експортовано рядків: ${n}`, "ok");
  } catch (e) { notify(String(e), "err"); }
}
async function toggleExclude(item: PlanItem) {
  const ex = item.status === "pending";
  try {
    await api.excludePlanItem(item.id, ex);
    item.status = ex ? "excluded" : "pending";
    plan = { ...plan! };
  } catch (e) { notify(String(e), "err"); }
}
function openEdit(item: PlanItem) {
  editing = item;
  editPlace = "";
  editDate = "";
}
async function saveEdit() {
  if (!editing || !plan) return;
  try {
    await api.updatePlanItem(plan.plan_id, editing.id, editPlace || undefined, editDate || undefined);
    const [items, summary] = await api.getPlan(plan.plan_id);
    plan = { ...plan, items, summary };
    editing = null;
  } catch (e) { notify(String(e), "err"); }
}
function baseName(p: string) {
  return p.split(/[\\/]/).pop() ?? p;
}
function ext(p: string) {
  return (p.split(".").pop() ?? "").toUpperCase().slice(0, 4);
}
function destPreview(p: string, root: string) {
  return p.startsWith(root) ? "." + p.slice(root.length) : p;
}
</script>

<div class="view-controls">
  <div>
    <div class="section-heading" style="margin:0">Перевірте, куди потрапить кожен файл</div>
    <div style="font-size:11px;color:var(--muted);margin-top:5px">План ще не змінює файли на диску.</div>
  </div>
  <div class="view-right">
    <div class="search-wrap"><span class="search-icon">⌕</span>
      <input class="search" bind:value={planSearch} placeholder="Пошук у плані" aria-label="Пошук у плані" /></div>
    <button class="btn secondary" onclick={rebuild} disabled={busy || applying}>
      {busy ? "Побудова…" : "↻ План"}
    </button>
    <button class="btn ghost" onclick={() => exportCsv("csv")}>CSV</button>
    <button class="btn ghost" onclick={() => exportCsv("json")}>JSON</button>
  </div>
</div>

{#if !plan}
  <div class="empty-state"><strong>План ще не побудовано</strong>
    Завершіть сканування та виберіть папку архіву — потім «Переглянути план».
    Жоден файл не зміниться до вашого підтвердження.
    <div style="margin-top:12px;"><button class="btn primary" onclick={rebuild} disabled={busy}>
      {busy ? "Побудова…" : "Побудувати план"}</button></div>
  </div>
{:else}
  <div class="plan-summary">
    <div class="plan-stat"><div class="plan-stat-num">{plan.summary.to_copy + plan.summary.to_move}</div>
      <div class="plan-stat-label">готові до сортування</div></div>
    <div class="plan-stat"><div class="plan-stat-num">{plan.summary.unknown_date + plan.summary.unknown_place}</div>
      <div class="plan-stat-label">без дати або назви місця</div></div>
    <div class="plan-stat"><div class="plan-stat-num">{plan.summary.conflicts}</div>
      <div class="plan-stat-label">потенційні конфлікти / дублікати</div></div>
  </div>

  <div class="plan-note">ⓘ <strong>Безпечний попередній перегляд.</strong>
    Kadr не змінює жодного файла, доки ви не підтвердите виконання.
    Обсяг: {fmtBytes(plan.summary.expected_bytes)} · всього файлів: {plan.summary.total}.</div>

  {#if applyProgress}
    <div class="banner" class:err={!!applyProgress.error} style="margin-bottom:12px;">
      <span>{applyProgress.error ? "⚠" : applying ? "⏳" : "✓"}</span>
      <span style="flex:1">
        {applyProgress.error ??
          `${applyProgress.done} готово, ${applyProgress.failed} помилок${applyProgress.current ? " — " + baseName(applyProgress.current) : ""}`}
        <div class="progress-track" style="margin-top:6px;">
          <div class="progress-fill"
            style="width:{applyProgress.total ? Math.round(((applyProgress.done + applyProgress.failed) / applyProgress.total) * 100) : 0}%"></div>
        </div>
      </span>
      {#if applying}
        <button class="btn danger" onclick={() => api.cancelOperation()}>Скасувати</button>
      {/if}
    </div>
  {/if}

  <div class="plan-table-wrap">
    <table class="plan-table">
      <thead><tr>
        <th>Вихідний файл</th><th>Дата зйомки</th><th>Місце</th><th>Цільовий шлях</th><th>Статус</th><th></th>
      </tr></thead>
      <tbody>
        {#each filtered.slice(0, 500) as it (it.id)}
          <tr>
            <td><div class="plan-file"><span class="file-mini">{ext(it.source_path)}</span>
              <span>{baseName(it.source_path)}{#if it.sidecar_of} <span class="plan-badge warn">sidecar</span>{/if}</span>
              <input type="checkbox" checked={it.status === "excluded"} title="Виключити файл з плану"
                aria-label="Виключити" onchange={() => toggleExclude(it)} /></div>
              {#each it.warnings as w}<div style="color:var(--amber);font-size:9px;">⚠ {w.message}</div>{/each}
            </td>
            <td>{fmtDate(itemCap(it))}</td>
            <td>{itemPlace(it)}</td>
            <td class="plan-destination">{destPreview(it.destination_path, destRoot || "")}</td>
            <td><span class="plan-badge {statusBadge[it.status]?.[0]}">{statusBadge[it.status]?.[1]}</span>
              {#if it.error}<div style="color:var(--red);font-size:9px;">{it.error}</div>{/if}</td>
            <td><button class="edit-text" onclick={() => openEdit(it)} title="Змінити дату/місце" aria-label="Змінити дату/місце">✎</button></td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
  {#if filtered.length > 500}
    <div class="hint" style="margin-top:8px;">Показано перші 500 з {filtered.length}. Знайдіть потрібне через пошук у плані.</div>
  {/if}

  <div class="plan-bottom">
    <div class="operation-mode">
      <span class="mode-label">Режим операції</span>
      <button class="mode-btn" class:active={mode === "copy"} onclick={() => setMode("copy")}>⧉ Копіювати</button>
      <button class="mode-btn" class:active={mode === "move"} onclick={() => setMode("move")}>↗ Перемістити</button>
    </div>
    <div style="display:flex;gap:8px;flex-wrap:wrap;">
      {#if doneAll}
        <button class="btn secondary" onclick={undo}>↺ Скасувати операцію</button>
        <button class="btn secondary" onclick={retry}>Повторити помилки</button>
      {/if}
      <button class="btn secondary" onclick={onBack}>← До медіатеки</button>
      <button class="btn primary" onclick={() => (confirmOpen = true)} disabled={applying}>
        {mode === "copy" ? "Підтвердити сортування" : "Підтвердити переміщення"} →
      </button>
    </div>
  </div>
{/if}

{#if confirmOpen && plan}
  <div class="modal-backdrop" role="dialog" aria-modal="true">
    <div class="modal-card">
      <div class="modal-head">
        <div>
          <div class="modal-title">Підтвердження{mode === "move" ? " — небезпечна дія" : ""}</div>
          <div class="modal-subtitle">
            {mode === "copy"
              ? "Kadr СКОПІЮЄ файли в папку архіву. Оригінали залишаться недоторканими."
              : "Kadr ПЕРЕМІСТИТЬ файли: оригінали буде видалено лише після успішної перевірки копії (SHA-256)."}
          </div>
        </div>
        <button class="icon-button" aria-label="Закрити" onclick={() => (confirmOpen = false)}>×</button>
      </div>
      <p style="margin:0;">Файлів: <b>{plan.items.filter((i) => i.status === "pending").length}</b> ·
        обсяг: <b>{fmtBytes(plan.summary.expected_bytes)}</b> ·
        режим: <b>{mode === "copy" ? "Копіювати" : "Перемістити"}</b></p>
      <div class="modal-actions">
        <button class="btn secondary" onclick={() => (confirmOpen = false)}>Скасувати</button>
        <button class="btn {mode === "move" ? "danger" : "primary"}" onclick={doApply}>
          {mode === "copy" ? "Копіювати" : "Перемістити"}
        </button>
      </div>
    </div>
  </div>
{/if}

{#if editing}
  <div class="modal-backdrop" role="dialog" aria-modal="true">
    <div class="modal-card">
      <div class="modal-head">
        <div><div class="modal-title">{baseName(editing.source_path)}</div>
          <div class="modal-subtitle">Перевизначення не змінюють метадані оригінального файла.</div></div>
        <button class="icon-button" aria-label="Закрити" onclick={() => (editing = null)}>×</button>
      </div>
      <div class="form-row"><label>Місце (перевизначення)</label>
        <input bind:value={editPlace} placeholder="напр. Карпати" /></div>
      <div class="form-row"><label>Дата зйомки</label>
        <input bind:value={editDate} placeholder="2025-08-12 14:03:22" /></div>
      <div class="modal-actions">
        <button class="btn secondary" onclick={() => (editing = null)}>Скасувати</button>
        <button class="btn primary" onclick={saveEdit}>Зберегти</button>
      </div>
    </div>
  </div>
{/if}

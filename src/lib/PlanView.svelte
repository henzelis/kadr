<script lang="ts">
import { save } from "@tauri-apps/plugin-dialog";
import {
  api, fmtBytes, fmtDate,
  type ApplyProgress, type BuiltPlan, type PlanItem, type PlanSummary,
} from "./api";

export let plan: BuiltPlan | null;
export let buildPlan: (mode?: string) => Promise<void>;
export let refreshStats: () => void;
export let applyProgress: ApplyProgress | null;
export let notify: (msg: string, kind?: "ok" | "err") => void;
export let mediaById: Map<string, any>;
export let destRoot = "";

function itemCap(it: PlanItem) {
  return mediaById?.get(it.media_id)?.captured_at_local ?? null;
}
function itemPlace(it: PlanItem) {
  const m = mediaById?.get(it.media_id);
  if (!m) return "—";
  if (m.place_label) return m.place_label;
  if (m.latitude != null) return `${m.latitude.toFixed(4)}, ${m.longitude.toFixed(4)}`;
  return "—";
}

let mode: "copy" | "move" = "copy";
let confirmOpen = false;
let editing: PlanItem | null = null;
let editPlace = "";
let editDate = "";
let busy = false;

$: applying = applyProgress && !applyProgress.finished;
$: doneAll = applyProgress && applyProgress.finished && !applyProgress.error;

const statusBadge: Record<string, [string, string]> = {
  pending: ["muted", "очікує"],
  excluded: ["muted", "виключено"],
  done: ["ok", "готово"],
  skipped: ["warn", "пропущено"],
  failed: ["err", "помилка"],
  undone: ["ok", "скасовано (undo)"],
  needs_user: ["warn", "потрібне рішення"],
};

async function rebuild() {
  busy = true;
  try { await buildPlan(mode); } finally { busy = false; }
}
async function toggleMode() {
  const next = mode === "copy" ? "move" : "copy";
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
    defaultPath: `kadr-plan-${format}.csv`.replace("csv", format),
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
    plan = plan; // trigger update
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
function destPreview(p: string, root: string) {
  return p.startsWith(root) ? "." + p.slice(root.length) : p;
}
</script>

<div style="display:flex;flex-direction:column;gap:14px;">
  <div style="display:flex;align-items:center;gap:12px;flex-wrap:wrap;">
    <h2 style="margin:0;">План сортування</h2>
    <div class="chip" class:on={mode === "copy"} style="cursor:default;">
      {mode === "copy" ? "✓ Копіювати (безпечно)" : "Копіювати"}
    </div>
    <button class="btn" onclick={toggleMode}>
      {mode === "move" ? "✓ Перемістити" : "Перемістити"}
    </button>
    <button class="btn" onclick={rebuild} disabled={busy || applying}>
      {busy ? "Побудова…" : "Побудувати план"}
    </button>
    {#if plan}
      <button class="btn primary" onclick={() => (confirmOpen = true)} disabled={applying}>
        Застосувати
      </button>
      {#if doneAll}
        <button class="btn" onclick={undo} disabled={applying}>Undo</button>
        <button class="btn" onclick={retry} disabled={applying}>Повторити помилки</button>
      {/if}
      <button class="btn ghost" onclick={() => exportCsv("csv")}>CSV</button>
      <button class="btn ghost" onclick={() => exportCsv("json")}>JSON</button>
    {/if}
  </div>

  {#if !plan}
    <div class="empty">
      План ще не побудовано. Перевірте, що сканування завершено та папку архіву вибрано в «Налаштуваннях»,
      натисніть «Побудувати план». Жодних файлів не змінюється до «Застосувати».
    </div>
  {:else}
    <div style="display:flex;gap:8px;flex-wrap:wrap;">
      <span class="badge muted">Всього: {plan.summary.total}</span>
      <span class="badge ok">до копіювання: {plan.summary.to_copy}</span>
      <span class="badge warn">до переміщення: {plan.summary.to_move}</span>
      <span class="badge muted">конфлікти: {plan.summary.conflicts}</span>
      <span class="badge warn">без дати: {plan.summary.unknown_date}</span>
      <span class="badge warn">без місця: {plan.summary.unknown_place}</span>
      <span class="badge muted">обсяг: {fmtBytes(plan.summary.expected_bytes)}</span>
    </div>

    {#if applyProgress}
      <div class="banner" class:err={!!applyProgress.error}>
        <span>{applyProgress.error ? "⚠" : applying ? "⏳" : "✓"}</span>
        <span>
          {applyProgress.error ??
            `${applyProgress.done} готово, ${applyProgress.failed} помилок ${applyProgress.current ? "— " + baseName(applyProgress.current) : ""}`}
        </span>
        <div class="progress-track">
          <div class="progress-fill"
            style="width:{applyProgress.total ? Math.round(((applyProgress.done + applyProgress.failed) / applyProgress.total) * 100) : 0}%"></div>
        </div>
        {#if applying}
          <button class="btn danger" onclick={() => api.cancelOperation()}>Скасувати</button>
        {/if}
      </div>
    {/if}

    <table class="plain">
      <thead>
        <tr><th></th><th>Файл</th><th>Дата</th><th>Місце</th><th>Цільовий шлях</th><th>Стан</th><th>Попередження</th><th></th></tr>
      </thead>
      <tbody>
        {#each plan.items.slice(0, 500) as it (it.id)}
          <tr class={it.status === "done" ? "row-done" : it.status === "failed" ? "row-failed" : ""}>
            <td><input type="checkbox" checked={it.status === "excluded"}
              aria-label="Виключити" onchange={() => toggleExclude(it)} /></td>
            <td title={it.source_path}>{baseName(it.source_path)}{#if it.sidecar_of} <span class="badge muted">sidecar</span>{/if}</td>
            <td>{fmtDate(itemCap(it))}</td>
            <td>{itemPlace(it)}</td>
            <td class="mono">{destPreview(it.destination_path, destRoot || '')}</td>
            <td><span class="badge {statusBadge[it.status]?.[0]}">{statusBadge[it.status]?.[1]}</span>
              {#if it.error}<div class="hint">{it.error}</div>{/if}</td>
            <td>{#each it.warnings as w}<div class="hint">⚠ {w.message}</div>{/each}</td>
            <td><button class="btn ghost" onclick={() => openEdit(it)} aria-label="Змінити дату/місце">✎</button></td>
          </tr>
        {/each}
      </tbody>
    </table>
    {#if plan.items.length > 500}
      <div class="hint">Показано перші 500 з {plan.items.length}. Увімкніть фільтри в галереї для вузького плану.</div>
    {/if}
  {/if}
</div>

{#if confirmOpen && plan}
  <div class="modal-backdrop" role="dialog" aria-modal="true">
    <div class="modal">
      <h3>Підтвердження{mode === "move" ? " — небезпечна дія" : ""}</h3>
      <p>
        {mode === "copy"
          ? "Kadr СКОПІЮЄ файли в папку архіву. Оригінали залишаться недоторканими."
          : "Kadr ПЕРЕМІСТИТЬ файли: оригінали буде видалено лише після успішної перевірки копії (SHA-256)."}
      </p>
      <p>
        Файлів: <b>{plan.items.filter((i) => i.status === "pending").length}</b> ·
        обсяг: <b>{fmtBytes(plan.summary.expected_bytes)}</b> ·
        режим: <b>{mode === "copy" ? "Копіювати" : "Перемістити"}</b>
      </p>
      <div style="display:flex;gap:10px;justify-content:flex-end;">
        <button class="btn" onclick={() => (confirmOpen = false)}>Скасувати</button>
        <button class="btn {mode === 'move' ? 'danger' : 'primary'}" onclick={doApply}>
          {mode === "copy" ? "Копіювати" : "Перемістити"}
        </button>
      </div>
    </div>
  </div>
{/if}

{#if editing}
  <div class="modal-backdrop" role="dialog" aria-modal="true">
    <div class="modal">
      <h3>{baseName(editing.source_path)}</h3>
      <div class="field">
        <label>Місце (перевизначення)</label>
        <input bind:value={editPlace} placeholder="напр. Карпати" />
      </div>
      <div class="field">
        <label>Дата зйомки</label>
        <input bind:value={editDate} placeholder="2025-08-12 14:03:22" />
      </div>
      <div class="hint">Перевизначення не змінюють метадані оригінального файлу.</div>
      <div style="display:flex;gap:10px;justify-content:flex-end;margin-top:12px;">
        <button class="btn" onclick={() => (editing = null)}>Скасувати</button>
        <button class="btn primary" onclick={saveEdit}>Зберегти</button>
      </div>
    </div>
  </div>
{/if}

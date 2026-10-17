<script lang="ts">
import { open } from "@tauri-apps/plugin-dialog";
import { api, type AppSettings, type SourceRoot, type ToolStatus } from "./api";

export let settings: AppSettings;
export let roots: SourceRoot[];
export let tools: ToolStatus;
export let onSaved: (s: AppSettings) => void = () => {};
export let onClose: () => void = () => {};
export let onRefresh: () => Promise<void> = async () => {};

let err = "";

async function addRoot() {
  const dir = await open({ directory: true, title: "Виберіть вихідну папку з фото/відео" });
  if (!dir) return;
  try {
    await api.addRoot(String(dir));
    await onRefresh();
  } catch (e) { err = String(e); }
}
async function removeRoot(id: string) {
  try {
    await api.removeRoot(id);
    await onRefresh();
  } catch (e) { err = String(e); }
}
async function pickDest() {
  const dir = await open({ directory: true, title: "Виберіть папку архіву (куди складати)" });
  if (!dir) return;
  settings = { ...settings, destination_root: String(dir) };
}
async function saveAll() {
  err = "";
  try {
    settings = await api.updateSettings(settings);
    onSaved(settings);
    onClose();
  } catch (e) { err = String(e); }
}
</script>

<svelte:window on:keydown={(e) => e.key === "Escape" && onClose()} />
<div class="modal-backdrop" role="dialog" aria-modal="true" aria-labelledby="modalTitle"
  onclick={(e) => e.target === e.currentTarget && onClose()}>
  <div class="modal-card">
    <div class="modal-head">
      <div>
        <div class="modal-title" id="modalTitle">Налаштування сортування</div>
        <div class="modal-subtitle">Kadr не змінює файли — тут лише правила побудови архіву.</div>
      </div>
      <button class="icon-button" aria-label="Закрити" onclick={onClose}>×</button>
    </div>

    {#if !tools.exiftool_path || !tools.ffprobe_path}
      <div class="banner err" style="margin-bottom:12px;"><span>⚠</span>
        <span>{!tools.exiftool_path ? "ExifTool не знайдено — розширені метадані (EXIF/GPS/RAW) недоступні. " : ""}{!tools.ffprobe_path ? "FFprobe не знайдено — теги відео недоступні." : ""} Перевстановіть Kadr.</span></div>
    {/if}

    <div class="form-row">
      <label>Вихідні папки</label>
      {#each roots as r (r.id)}
        <div class="root-chip" style="justify-content:space-between;">
          <span class="path" title={r.path}>{r.path}</span>
          <button class="btn ghost" style="min-height:26px;padding:0 8px;" onclick={() => removeRoot(r.id)}
            aria-label="Видалити джерело">✕</button>
        </div>
      {/each}
      <div style="margin-top:8px;"><button class="btn secondary" onclick={addRoot}>＋ Додати папку</button></div>
    </div>

    <div class="form-row">
      <label>Папка архіву (ціль)</label>
      <div style="display:flex;gap:8px;">
        <input value={settings.destination_root} placeholder="напр. D:/Media-Archive" style="flex:1;"
          oninput={(e) => (settings = { ...settings, destination_root: e.currentTarget.value })} />
        <button class="btn secondary" onclick={pickDest}>Вибрати…</button>
      </div>
      <div class="hint" style="margin-top:6px;">Не обирайте ту саму теку, яку індексуєте.</div>
    </div>

    <div class="form-row">
      <label for="monthFormat">Формат папки місяця</label>
      <select id="monthFormat" value={settings.month_format}
        onchange={(e) => (settings = { ...settings, month_format: e.currentTarget.value as any })}>
        <option value="number_name">08 - Серпень</option>
        <option value="number">08</option>
        <option value="name">Серпень</option>
      </select>
    </div>

    <div class="form-row">
      <label for="conflictPolicy">При однакових назвах файлів</label>
      <select id="conflictPolicy" value={settings.conflict_policy}
        onchange={(e) => (settings = { ...settings, conflict_policy: e.currentTarget.value as any })}>
        <option value="suffix">Додати суфікс « (1)» (безпечно)</option>
        <option value="skip">Пропустити конфлікт</option>
        <option value="ask">Залишити мій вибір у плані</option>
      </select>
    </div>

    <div class="form-row">
      <label for="template">Шаблон структури (для досвідчених)</label>
      <input id="template" class="mono" value={settings.template}
        oninput={(e) => (settings = { ...settings, template: e.currentTarget.value })} />
    </div>

    <label class="check-row">
      <input type="checkbox" checked={settings.fs_date_fallback}
        onchange={(e) => (settings = { ...settings, fs_date_fallback: e.currentTarget.checked })} />
      <span><strong style="color:var(--text)">Дозволити дату модифікації файла як fallback</strong><br>
        Такі дати позначатимуться як дані файлової системи, а не підтверджена дата зйомки.</span>
    </label>
    <label class="check-row">
      <input type="checkbox" checked={settings.coords_as_label}
        onchange={(e) => (settings = { ...settings, coords_as_label: e.currentTarget.checked })} />
      <span><strong style="color:var(--text)">Використовувати координати як назву місця, якщо назви немає</strong></span>
    </label>
    <label class="check-row">
      <input type="checkbox" checked={settings.compute_hashes_on_scan ?? false}
        onchange={(e) => (settings = { ...settings, compute_hashes_on_scan: e.currentTarget.checked })} />
      <span><strong style="color:var(--text)">Обчислювати SHA-256 під час сканування</strong><br>
        Потрібно лише для пошуку дублікатів; на великих архівах суттєво уповільнює сканування.</span>
    </label>

    {#if err}<div style="color:var(--red);font-size:12px;margin-top:12px;">{err}</div>{/if}

    <div class="modal-actions">
      <button class="btn secondary" onclick={onClose}>Скасувати</button>
      <button class="btn primary" onclick={saveAll}>Зберегти параметри</button>
    </div>
  </div>
</div>

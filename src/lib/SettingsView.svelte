<script lang="ts">
import { open, save } from "@tauri-apps/plugin-dialog";
import { api, type AppSettings, type SourceRoot, type ToolStatus } from "./api";

export let settings: AppSettings;
export let roots: SourceRoot[];
export let tools: ToolStatus;

let toast = "";
let err = "";

async function addRoot() {
  const dir = await open({ directory: true, title: "Виберіть вихідну папку з фото/відео" });
  if (!dir) return;
  try {
    await api.addRoot(String(dir));
    roots = await api.listRoots();
  } catch (e) { err = String(e); }
}
async function removeRoot(id: string) {
  try {
    await api.removeRoot(id);
    roots = await api.listRoots();
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
    toast = "Налаштування збережено";
    setTimeout(() => (toast = ""), 2500);
  } catch (e) { err = String(e); }
}
</script>

<div style="max-width: 720px; display:flex; flex-direction:column; gap:22px;">
  <h2 style="margin:0;">Налаштування</h2>

  {#if tools.exiftool_path}
    <div class="banner"><span>✓</span><span>ExifTool: {tools.exiftool_version ?? "невідома версія"}</span></div>
  {:else}
    <div class="banner err"><span>⚠</span>
      <span>ExifTool не знайдено — розширені метадані (EXIF/GPS/RAW) будуть недоступні.
      Встановіть ExifTool і перезапустіть Kadr.</span></div>
  {/if}
  {#if tools.ffprobe_path}
    <div class="banner"><span>✓</span><span>FFprobe: {tools.ffprobe_version ?? ""}</span></div>
  {:else}
    <div class="banner err"><span>⚠</span><span>FFprobe не знайдено — технічні теги відео недоступні.
      Встановіть FFmpeg/FFprobe.</span></div>
  {/if}

  <section>
    <h3>Вихідні папки</h3>
    {#each roots as r (r.id)}
      <div class="root-chip" style="justify-content:space-between;">
        <span class="path" title={r.path}>{r.path}</span>
        <button class="btn ghost" onclick={() => removeRoot(r.id)} aria-label="Видалити джерело">✕</button>
      </div>
    {/each}
    <div style="margin-top:8px;"><button class="btn" onclick={addRoot}>＋ Додати папку</button></div>
  </section>

  <section>
    <h3>Папка архіву (ціль)</h3>
    <div class="field">
      <input value={settings.destination_root} placeholder="напр. D:/Media-Archive або /home/you/Media-Archive"
        oninput={(e) => (settings = { ...settings, destination_root: e.currentTarget.value })} />
    </div>
    <button class="btn" onclick={pickDest}>Виберіть теку…</button>
    <div class="hint" style="margin-top:6px;">
      Не обирайте ту саму теку, яку індексуєте — Kadr попередить у плані.
    </div>
  </section>

  <section>
    <h3>Схема папок</h3>
    <div class="field">
      <label>Шаблон</label>
      <input class="mono" value={settings.template} placeholder="{'{year}/{month_number} - {month_name}/{place}/{filename}'}"
        oninput={(e) => (settings = { ...settings, template: e.currentTarget.value })} />
    </div>
    <div class="field">
      <label>Формат місяця</label>
      <select value={settings.month_format}
        onchange={(e) => (settings = { ...settings, month_format: e.currentTarget.value as any })}>
        <option value="number_name">08 - Серпень</option>
        <option value="number">08</option>
        <option value="name">Серпень</option>
      </select>
    </div>
    <div class="field">
      <label>Конфлікт назв</label>
      <select value={settings.conflict_policy}
        onchange={(e) => (settings = { ...settings, conflict_policy: e.currentTarget.value as any })}>
        <option value="suffix">Додати суфікс « (1)» (безпечно)</option>
        <option value="skip">Пропустити конфлікт</option>
        <option value="ask">Залишити користувачу</option>
      </select>
    </div>
    <label style="display:flex;gap:8px;align-items:center;margin:8px 0;">
      <input type="checkbox" checked={settings.fs_date_fallback}
        onchange={(e) => (settings = { ...settings, fs_date_fallback: e.currentTarget.checked })} />
      Дата файлової системи як резерв (позначається як непідтверджена)
    </label>
    <label style="display:flex;gap:8px;align-items:center;margin:8px 0;">
      <input type="checkbox" checked={settings.coords_as_label}
        onchange={(e) => (settings = { ...settings, coords_as_label: e.currentTarget.checked })} />
      Використовувати координати як назву місця, якщо назви немає
    </label>
    <label style="display:flex;gap:8px;align-items:center;margin:8px 0;">
      <input type="checkbox" checked={settings.compute_hashes_on_scan ?? true}
        onchange={(e) => (settings = { ...settings, compute_hashes_on_scan: e.currentTarget.checked })} />
      Обчислювати SHA-256 під час сканування (потрібно для дублікатів)
    </label>
  </section>

  <div style="display:flex;gap:10px;align-items:center;">
    <button class="btn primary" onclick={saveAll}>Зберегти</button>
    {#if err}<span style="color:var(--red);">{err}</span>{/if}
    {#if toast}<span style="color:var(--accent-strong);">{toast}</span>{/if}
  </div>
</div>

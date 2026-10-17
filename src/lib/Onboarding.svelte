<script lang="ts">
import { open } from "@tauri-apps/plugin-dialog";
import { api, type AppSettings, type SourceRoot } from "./api";

export let roots: SourceRoot[] = [];
export let settings: AppSettings | null = null;
export let onRefresh: () => Promise<void> = async () => {};
export let onStart: () => void = () => {};

let err = "";

function norm(p: string): string {
  return p.toLowerCase().replace(/[\\/]+$/, "");
}
function sameTree(a: string, b: string): boolean {
  const na = norm(a), nb = norm(b);
  return na === nb || na.startsWith(nb + "/") || nb.startsWith(na + "/");
}

async function pickSource() {
  err = "";
  const dir = await open({ directory: true, title: "Крок 1 — оберіть папку з фото та відео" });
  if (!dir) return;
  try {
    await api.addRoot(String(dir));
    await onRefresh();
  } catch (e) { err = String(e); }
}

async function pickDest() {
  err = "";
  const dir = await open({ directory: true, title: "Крок 2 — оберіть папку архіву (куди складати)" });
  if (!dir) return;
  for (const r of roots) {
    if (sameTree(String(dir), r.path)) {
      err = "Папка архіву не може збігатися з вихідною папкою чи бути всередині неї (і навпаки) — оберіть іншу.";
      return;
    }
  }
  try {
    const base = settings ?? (await api.getSettings());
    settings = await api.updateSettings({ ...base, destination_root: String(dir) });
    await onRefresh();
  } catch (e) { err = String(e); }
}

const ready = () => roots.length > 0 && !!settings?.destination_root;
</script>

<section class="wizard">
  <h2>Давайте організуємо ваші фото</h2>
  <p class="sub">Kadr працює лише на вашому комп'ютері — нічого нікуди не надсилається.
    Архів складається за схемою <b>Рік / Місяць / Місце</b>. Три кроки — і далі все сам.</p>

  <div class="step" class:done={roots.length > 0}>
    <span class="num">{roots.length > 0 ? "✓" : "1"}</span>
    <div class="step-body">
      <h3>Папка з фото</h3>
      <p>{roots.length ? `Додано папок: ${roots.length} — можна додати ще (наприклад, зовнішній диск).`
        : "Оберіть теку з фотографіями — Kadr осмотре її та всі підтеки."}</p>
    </div>
    <button class="btn" onclick={pickSource}>{roots.length ? "Додати ще" : "Обрати папку"}</button>
  </div>

  <div class="step" class:done={!!settings?.destination_root}>
    <span class="num">{settings?.destination_root ? "✓" : "2"}</span>
    <div class="step-body">
      <h3>Папка для результату</h3>
      <p>{settings?.destination_root ?? "Куди складати відсортоване. Оригінали залишаються на місці — безпечний режим «Копіювати»."}</p>
    </div>
    <button class="btn" onclick={pickDest}>{settings?.destination_root ? "Змінити" : "Обрати папку"}</button>
  </div>

  <div class="step hint-step">
    <span class="num">3</span>
    <div class="step-body">
      <h3>Аналіз</h3>
      <p>Kadr прочитає дати й GPS, сам назве місця зйомки офлайн і запропонує план.
        Ви побачите кожен шлях до будь-якої дії, і все можна скасувати.</p>
    </div>
    <button class="btn primary" disabled={!ready()} onclick={onStart}>▶ Почати аналіз</button>
  </div>

  {#if err}<div class="wizard-err">⚠ {err}</div>{/if}
</section>

<style>
.wizard { max-width: 660px; margin: 5vh auto; display: flex; flex-direction: column; gap: 14px; }
.wizard h2 { margin: 0 0 2px; }
.sub { margin: 0 0 10px; opacity: .8; }
.step { display: flex; gap: 14px; align-items: center; background: var(--panel, #fff); border: 1px solid var(--line); border-radius: 12px; padding: 14px 16px; }
.step.done { border-color: var(--accent); }
.num { flex: 0 0 34px; height: 34px; display: grid; place-items: center; border-radius: 50%; background: var(--accent-soft); color: var(--accent-strong); font-weight: 700; }
.step-body { flex: 1; min-width: 0; }
.step-body h3 { margin: 0 0 2px; font-size: 15px; }
.step-body p { margin: 0; font-size: 13px; opacity: .75; overflow-wrap: anywhere; }
.hint-step { background: transparent; border-style: dashed; }
.wizard-err { color: var(--red); font-size: 13px; }
</style>

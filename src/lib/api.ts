import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// ── Types mirroring the Rust serde DTOs (snake_case on the wire) ──

export interface MediaItem {
  id: string;
  source_root_id: string;
  path: string;
  normalized_path: string;
  file_name: string;
  extension: string;
  media_kind: "photo" | "video" | "sidecar" | "other";
  size_bytes: number;
  mtime: number;
  sha256: string | null;
  metadata_status: "ok" | "unavailable" | "needs_review" | "unsupported";
  thumbnail_path: string | null;
  captured_at_raw: string | null;
  captured_at_local: string | null;
  captured_at_utc: string | null;
  timezone_offset: number | null;
  date_source: string | null;
  date_status: "confirmed" | "needs_review" | "fallback" | "unknown";
  normalization_note: string | null;
  latitude: number | null;
  longitude: number | null;
  altitude: number | null;
  place_label: string | null;
  location_source: string | null;
  place_status: "resolved" | "coords_only" | "no_geo" | "needs_review";
  manual_place_override: boolean;
  duration_ms: number | null;
  width: number | null;
  height: number | null;
  video_codec: string | null;
  audio_codec: string | null;
  last_error: string | null;
  is_sidecar: boolean;
  sidecar_parent_id: string | null;
}

export interface MediaFilter {
  search?: string | null;
  kind?: string | null;
  date_status?: string | null;
  place_status?: string | null;
  needs_review?: boolean | null;
  duplicates_only?: boolean | null;
  root_id?: string | null;
  limit?: number | null;
  offset?: number | null;
  sort?: string | null;
}

export interface SourceRoot { id: string; path: string; status: string; added_at: number }

export interface ToolStatus {
  exiftool_path: string | null;
  exiftool_version: string | null;
  ffprobe_path: string | null;
  ffprobe_version: string | null;
}

export interface AppSettings {
  destination_root: string;
  template: string;
  month_format: "number_name" | "number" | "name";
  conflict_policy: "suffix" | "skip" | "ask";
  unknown_date_label: string | null;
  no_geo_label: string | null;
  fs_date_fallback: boolean;
  coords_as_label: boolean;
  compute_hashes_on_scan: boolean | null;
}

export interface PlanWarning { code: string; message: string }
export interface PlanItem {
  id: string; plan_id: string; media_id: string;
  source_path: string; destination_path: string;
  mode: "copy" | "move";
  status: "pending" | "excluded" | "done" | "skipped" | "failed" | "undone" | "needs_user";
  size_bytes: number;
  sha256_before: string | null;
  sha256_after: string | null;
  warnings: PlanWarning[];
  sidecar_of: string | null;
  place_override: string | null;
  error: string | null;
  finished_at: number | null;
}
export interface PlanSummary {
  total: number; to_copy: number; to_move: number; skipped: number;
  conflicts: number; unknown_date: number; unknown_place: number; expected_bytes: number;
}
export interface BuiltPlan { plan_id: string; summary: PlanSummary; items: PlanItem[] }

export interface ScanProgress {
  job_id: string; found: number; processed: number; skipped: number; errors: number;
  current: string; finished: boolean; cancelled: boolean; error: string | null;
}
export interface ApplyProgress {
  plan_id: string; total: number; done: number; failed: number; skipped: number;
  current: string; finished: boolean; cancelled: boolean; error: string | null;
}

export interface LibraryStats {
  total: number; photos: number; videos: number; unknown_date: number; no_geo: number;
  coords_only: number; needs_review: number; duplicate_groups: number; total_bytes: number;
}

export interface LogEntry { id: number; job_id: string; level: string; action: string; message: string; created_at: number }

// ── Commands ──

export const api = {
  toolStatus: () => invoke<ToolStatus>("get_tool_status"),
  getSettings: () => invoke<AppSettings>("get_settings"),
  updateSettings: (settings: AppSettings) => invoke<AppSettings>("update_settings", { settings }),
  addRoot: (path: string) => invoke<SourceRoot>("add_source_root", { path }),
  removeRoot: (id: string) => invoke<void>("remove_source_root", { id }),
  listRoots: () => invoke<SourceRoot[]>("list_source_roots"),
  logs: (limit?: number) => invoke<LogEntry[]>("get_logs", { limit }),
  startScan: (rootId?: string) => invoke<string>("start_scan", { rootId: rootId ?? null }),
  cancelScan: () => invoke<void>("cancel_scan"),
  queryMedia: (filter: MediaFilter) =>
    invoke<{ items: MediaItem[]; total: number }>("query_media", { filter }),
  mediaDetails: (id: string) => invoke<MediaItem | null>("get_media_details", { id }),
  thumbB64: (id: string) => invoke<string | null>("get_thumbnail_base64", { id }),
  dupGroups: () => invoke<MediaItem[][]>("duplicate_groups"),
  ensureHashes: (limit?: number) => invoke<number>("ensure_hashes", { limit }),
  setManualPlace: (mediaIds: string[], place: string) =>
    invoke<void>("set_manual_place", { mediaIds, place }),
  setDateOverride: (id: string, date: string) => invoke<void>("set_date_override", { id, date }),
  buildPlan: (mode?: string) => invoke<BuiltPlan>("build_sort_plan", { mode: mode ?? null }),
  getPlan: (planId: string) =>
    invoke<[PlanItem[], PlanSummary]>("get_sort_plan", { planId }),
  updatePlanItem: (planId: string, itemId: string, place?: string, date?: string) =>
    invoke<string>("update_plan_item", { planId, itemId, place: place ?? null, date: date ?? null }),
  excludePlanItem: (itemId: string, excluded: boolean) =>
    invoke<void>("exclude_plan_item", { itemId, excluded }),
  setPlanMode: (planId: string, mode: string) => invoke<void>("set_plan_mode", { planId, mode }),
  applyPlan: (planId: string, confirmation: boolean) =>
    invoke<void>("apply_sort_plan", { planId, confirmation }),
  cancelOperation: () => invoke<void>("cancel_operation"),
  undoOperation: (planId: string) => invoke<void>("undo_operation", { planId }),
  retryFailed: (planId: string) => invoke<number>("retry_failed_items", { planId }),
  exportReport: (planId: string, outPath: string, format: string) =>
    invoke<number>("export_report", { planId, outPath, format }),
  stats: () => invoke<LibraryStats>("library_stats"),
};

export const onScanProgress = (cb: (p: ScanProgress) => void) =>
  listen<ScanProgress>("scan-progress", (e) => cb(e.payload));
export const onApplyProgress = (cb: (p: ApplyProgress) => void) =>
  listen<ApplyProgress>("apply-progress", (e) => cb(e.payload));

export function fmtBytes(n: number): string {
  if (!n) return "0 Б";
  const u = ["Б", "КБ", "МБ", "ГБ", "ТБ"];
  const i = Math.min(Math.floor(Math.log(n) / Math.log(1024)), 4);
  return `${(n / 1024 ** i).toFixed(i ? 1 : 0)} ${u[i]}`;
}
export function fmtDate(iso: string | null): string {
  if (!iso) return "—";
  return iso.replace("T", " ").slice(0, 16);
}

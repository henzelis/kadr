use crate::app_state::{err_str, AppState};
use kadr_core::persistence::db::{self, MediaFilter};
use kadr_core::services::{duplicate, thumbnail};
use std::sync::atomic::Ordering;
use tauri::State;

#[derive(serde::Serialize)]
pub struct MediaPage {
    pub items: Vec<kadr_core::domain::media_item::MediaItem>,
    pub total: i64,
}

#[tauri::command]
pub fn query_media(state: State<AppState>, filter: MediaFilter) -> Result<MediaPage, String> {
    let conn = state.conn().map_err(err_str)?;
    let (items, total) = db::query_media(&conn, &filter).map_err(err_str)?;
    Ok(MediaPage { items, total })
}

#[tauri::command]
pub fn get_media_details(
    state: State<AppState>,
    id: String,
) -> Result<Option<kadr_core::domain::media_item::MediaItem>, String> {
    let conn = state.conn().map_err(err_str)?;
    db::get_media_item(&conn, &id).map_err(err_str)
}

/// Thumbnail as base64 data URL part; None → UI shows a type placeholder.
#[tauri::command]
pub fn get_thumbnail_base64(state: State<AppState>, id: String) -> Result<Option<String>, String> {
    let conn = state.conn().map_err(err_str)?;
    let path = state
        .thumbs
        .thumbnail_for(&conn, &id)
        .ok_or("Немає мініатюри")?;
    let bytes = std::fs::read(&path).map_err(|_| "Немає мініатюри".to_string())?;
    Ok(Some(to_base64(&bytes)))
}

fn to_base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[tauri::command]
pub fn duplicate_groups(
    state: State<AppState>,
) -> Result<Vec<Vec<kadr_core::domain::media_item::MediaItem>>, String> {
    let conn = state.conn().map_err(err_str)?;
    duplicate::duplicate_groups(&conn).map_err(err_str)
}

#[tauri::command]
pub fn ensure_hashes(state: State<AppState>, limit: Option<i64>) -> Result<usize, String> {
    if state.scan_running.load(Ordering::Relaxed) {
        return Err("Зачекайте завершення сканування".into());
    }
    let conn = state.conn().map_err(err_str)?;
    let cancel = state.op_cancel.clone();
    cancel.store(false, Ordering::Relaxed);
    duplicate::ensure_hashes(&conn, cancel.as_ref(), limit.unwrap_or(5000), |_, _| {})
        .map_err(err_str)
}

/// Kick off background thumbnail generation for gallery items.
#[tauri::command]
pub fn generate_thumbnails(state: State<AppState>, limit: Option<i64>) -> Result<usize, String> {
    let conn = state.conn().map_err(err_str)?;
    let cancel = state.scan_cancel.clone();
    cancel.store(false, Ordering::Relaxed);
    let svc: &thumbnail::ThumbService = &state.thumbs;
    Ok(svc.generate_pending(&conn, cancel.as_ref(), limit.unwrap_or(300), |_, _| {}))
}

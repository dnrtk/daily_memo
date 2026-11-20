//! 複数端末の同期(Git)のコマンド。処理は memo-sync に置き、ここは作業フォルダ・設定の受け渡しだけ。
//! git はネットワーク待ちがあるので、すべて別スレッドで実行する。

use std::path::PathBuf;

use memo_core::workspace::Workspace;
use memo_sync::{
    conflict, remote, repo, CommitInfo, ConflictDetail, ConflictInfo, ErrorKind, Probe, Resolution, SyncError, SyncOptions,
    SyncReport, SyncStatus,
};
use serde::Serialize;
use tauri::Manager;

use crate::{info, AppState, CmdResult, WorkspaceInfo};

/// 同期・接続・解消の結果。失敗しても最新の状態は返す(状態表示を更新するため)
#[derive(Serialize)]
pub struct SyncOutcome {
    report: Option<SyncReport>,
    error: Option<SyncError>,
    status: SyncStatus,
    /// カテゴリ設定が他の端末から変わったときの作業フォルダ情報
    workspace: Option<WorkspaceInfo>,
}

fn root_of(app: &tauri::AppHandle) -> Result<PathBuf, SyncError> {
    let state = app.state::<AppState>();
    let guard = state.workspace.lock().map_err(|e| SyncError::state(e.to_string()))?;
    guard
        .as_ref()
        .map(|ws| ws.root().to_path_buf())
        .ok_or_else(|| SyncError::state("作業フォルダが開かれていません"))
}

fn device_of(app: &tauri::AppHandle) -> String {
    let name = app
        .state::<AppState>()
        .settings
        .lock()
        .map(|s| s.sync.device_name.trim().to_string())
        .unwrap_or_default();
    if name.is_empty() {
        memo_sync::default_device_name()
    } else {
        name
    }
}

/// 同期の処理は同時に 1 つだけ(自動同期と手動同期が重なったら後から来た方は Busy)
fn exclusive<T>(app: &tauri::AppHandle, f: impl FnOnce() -> Result<T, SyncError>) -> Result<T, SyncError> {
    let state = app.state::<AppState>();
    let _guard = state
        .sync_lock
        .try_lock()
        .map_err(|_| SyncError::new(ErrorKind::Busy, "同期中です"))?;
    f()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())
}

/// 他の端末からカテゴリ設定が届いたら、作業フォルダを開き直して反映する
fn reload_if_categories_changed(app: &tauri::AppHandle, root: &std::path::Path, report: &SyncReport) -> Option<WorkspaceInfo> {
    let touched = report.changed.iter().any(|c| c.path == ".memo/categories.yaml");
    if !touched {
        return None;
    }
    let ws = Workspace::open(root).ok()?;
    let out = info(&ws);
    let state = app.state::<AppState>();
    let mut guard = state.workspace.lock().ok()?;
    // 同期中に別の作業フォルダへ切り替えていたら触らない
    if guard.as_ref().is_some_and(|cur| cur.root() == root) {
        *guard = Some(ws);
        Some(out)
    } else {
        None
    }
}

fn outcome(app: &tauri::AppHandle, root: Option<PathBuf>, result: Result<SyncReport, SyncError>) -> SyncOutcome {
    let status = root.as_deref().map(repo::status).unwrap_or_default();
    match result {
        Ok(report) => {
            let workspace = root.as_deref().and_then(|r| reload_if_categories_changed(app, r, &report));
            SyncOutcome { report: Some(report), error: None, status, workspace }
        }
        Err(error) => SyncOutcome { report: None, error: Some(error), status, workspace: None },
    }
}

#[tauri::command]
pub async fn sync_status(app: tauri::AppHandle) -> CmdResult<SyncStatus> {
    blocking(move || root_of(&app).map(|r| repo::status(&r)).unwrap_or_default()).await
}

#[tauri::command]
pub async fn sync_probe(url: String) -> CmdResult<Result<Probe, SyncError>> {
    blocking(move || remote::probe(&url)).await
}

#[tauri::command]
pub async fn sync_connect(app: tauri::AppHandle, url: String) -> CmdResult<SyncOutcome> {
    blocking(move || {
        let root = root_of(&app).ok();
        let result = match &root {
            Some(r) => exclusive(&app, || repo::connect(r, &url, &device_of(&app))),
            None => Err(SyncError::state("作業フォルダが開かれていません")),
        };
        outcome(&app, root, result)
    })
    .await
}

#[tauri::command]
pub async fn sync_run(app: tauri::AppHandle, message: Option<String>) -> CmdResult<SyncOutcome> {
    blocking(move || {
        let root = root_of(&app).ok();
        let result = match &root {
            Some(r) => exclusive(&app, || repo::sync(r, &SyncOptions { device: device_of(&app), message })),
            None => Err(SyncError::state("作業フォルダが開かれていません")),
        };
        outcome(&app, root, result)
    })
    .await
}

#[tauri::command]
pub async fn sync_disable(app: tauri::AppHandle) -> CmdResult<SyncStatus> {
    blocking(move || {
        let root = root_of(&app).map_err(|e| e.message)?;
        exclusive(&app, || repo::disable(&root)).map_err(|e| e.message)?;
        Ok(repo::status(&root))
    })
    .await?
}

#[tauri::command]
pub async fn sync_history(app: tauri::AppHandle, limit: usize) -> CmdResult<Vec<CommitInfo>> {
    blocking(move || root_of(&app).map(|r| repo::history(&r, limit.min(100))).unwrap_or_default()).await
}

#[tauri::command]
pub async fn sync_conflicts(app: tauri::AppHandle) -> CmdResult<Vec<ConflictInfo>> {
    blocking(move || {
        let root = root_of(&app).map_err(|e| e.message)?;
        conflict::list(&root).map_err(|e| e.message)
    })
    .await?
}

#[tauri::command]
pub async fn sync_conflict_detail(app: tauri::AppHandle, path: String) -> CmdResult<ConflictDetail> {
    blocking(move || {
        let root = root_of(&app).map_err(|e| e.message)?;
        conflict::detail(&root, &path, &device_of(&app)).map_err(|e| e.message)
    })
    .await?
}

/// 戻り値は残りの競合数
#[tauri::command]
pub async fn sync_resolve(app: tauri::AppHandle, path: String, resolution: Resolution) -> CmdResult<usize> {
    blocking(move || {
        let root = root_of(&app).map_err(|e| e.message)?;
        exclusive(&app, || conflict::resolve(&root, &path, &resolution)).map_err(|e| e.message)
    })
    .await?
}

/// 端末名の既定値(設定画面のプレースホルダ用)
#[tauri::command]
pub fn sync_default_device() -> String {
    memo_sync::default_device_name()
}

//! 原版「导入音效」向导（`draw_window_sound_import` + `sound_import`）。
//!
//! 两条路照原版：本地从 `.minecraft/assets/indexes/*.json` 数/拷音效，
//! 下载模式走 Mojang 版本清单 → 版本 JSON → 资产索引 → 逐文件下载。
//! HTTP 由宿主注入的 [`MinecraftFetcher`] 提供（原版 `http_get` 底层就是 WinINet）。
//!
//! 偏差记账：
//! - 原版异步回调驱动（http_get 每步一个 async 事件），重写版是后台线程一次跑完
//!   阶段 2..4、UI 每帧抄进度，可见结果一致；原版下载中可以重复点按钮叠加请求，
//!   这里有线程在跑就不再叠加。
//! - 原版覆盖确认（message_yesnocancel）卡住整个游戏循环等用户回答；重写版线程在
//!   同一个确认点上挂起等 UI 的模态框，期间不发下一个请求。
//! - 原版 `json_parse` 解析失败会直接崩游戏；重写版按「未找到该索引所指向的文件」处理。
//! - 原版 `mc_default_path` 基于 GM 存档目录（`game_save_id` 去尾段 + `.minecraft`），
//!   重写版没有 game_save_id，取 `%LOCALAPPDATA%\.minecraft`，公式等价。
//! - 原版「更改」的 `get_directory` 返回值里会混进对话框标题、需要
//!   `string_replace_all` 清理；重写版目录对话框直接返回干净路径。
//! - 原版 objects 遍历按 `variable_struct_get_names` 的字母序倒着走，拷贝/下载
//!   顺序保持这个序。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use serde_json::Value;

use crate::commands::Command;
use crate::state::{EditorState, ModalQueue};
use crate::tr;

/// Mojang 版本清单（原版 stage 1 的 `http_get` 目标）。
const MANIFEST_URL: &str = "https://launchermeta.mojang.com/mc/game/version_manifest.json";

/// 原版 control_create.gml:605-618 的资产索引友好名表（键 → 显示名，位置即排序键）。
const INDEX_NAMES: &[(&str, &str)] = &[
    ("pre-1.6", "Pre-1.6"),
    ("legacy", "1.6-1.7"),
    ("1.7.3", "1.7.3"),
    ("1.7.4", "1.7.4"),
    ("1.7.10", "1.7.10"),
    ("14w25a", "14w25a"),
    ("14w31a", "1.8 Pre-Release 1"),
    ("1.8", "1.8"),
    ("1.9", "1.9"),
    ("1.9-aprilfools", "1.RV-Pre1"),
    ("1.10", "1.10"),
    ("1.11", "1.11"),
    ("1.12", "1.12"),
    ("1.13", "1.13"),
    ("1.13.1", "1.13.1"),
    ("1.14", "1.14"),
    ("1.14-af", "3D Shareware v1.34"),
    ("1.15", "1.15"),
    ("1.16", "1.16"),
    ("1.17", "1.17"),
    ("1.18", "1.18"),
    ("1.19", "1.19"),
    ("1", "22w42a"),
    ("2", "1.19.3"),
    ("3", "1.19.4"),
    ("4", "23w14a"),
    ("5", "1.20"),
    ("6", "23w31a"),
    ("7", "1.20.2 Pre-Release 1"),
    ("8", "1.20.2"),
    ("9", "23w42a"),
    ("10", "23w43a"),
    ("11", "23w45a"),
    ("12", "1.20.3"),
    ("13", "24w06a"),
    ("14", "24w09a"),
    ("15", "24w11a"),
    ("16", "1.20.5"),
    ("17", "1.21"),
    ("18", "1.21.2"),
    ("19", "1.21.4"),
    ("20", "25w02a"),
    ("21", "25w04a"),
    ("22", "25w05a"),
    ("23", "25w07a"),
    ("24", "1.21.5"),
    ("25", "25w20a"),
    ("26", "1.21.8"),
    ("27", "1.21.10"),
    ("28", "25w46a"),
    ("29", "1.21.11"),
    ("30", "26.1"),
    ("_", "26.1+"),
];

/// `get_asset_index_friendly_name`：查表；查不到用表里 `_` 的值兜底（"26.1+ (id)"）。
pub fn friendly_name(asset_index: &str) -> String {
    if asset_index.is_empty() {
        return String::new();
    }
    for (k, v) in INDEX_NAMES {
        if *k == asset_index {
            return (*v).to_string();
        }
    }
    format!("{} ({})", INDEX_NAMES[INDEX_NAMES.len() - 1].1, asset_index)
}

/// 原版 `string_format_thousands` 在这里只喂非负整数：千位逗号。
pub fn format_thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `get_assets_dir`：`.minecraft` 目录接上 `assets`。
pub fn get_assets_dir(mc_install_path: &str) -> PathBuf {
    let mut p = PathBuf::from(mc_install_path);
    p.push("assets");
    p
}

/// 排序键：表内位置（原版 `ds_list_find_index(sound_import_asset_index_names_sort, elm)`）。
fn index_position(id: &str) -> Option<usize> {
    INDEX_NAMES.iter().position(|(k, _)| *k == id)
}

/// `find_asset_indexes`：扫 `assets/indexes/*.json` 去掉扩展名。
/// 排序照原版比较器：不在表里的排最前，表内的按表序（老版本在前）。
pub fn find_asset_indexes(mc_install_path: &str) -> Vec<String> {
    let dir = get_assets_dir(mc_install_path).join("indexes");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut found: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"))
        .filter_map(|e| {
            e.path()
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
        })
        .collect();
    found.sort_by(|a, b| match (index_position(a), index_position(b)) {
        (None, _) => std::cmp::Ordering::Less,
        (_, None) => std::cmp::Ordering::Greater,
        (Some(x), Some(y)) => x.cmp(&y),
    });
    found
}

/// 原版 `load_asset_index` / 下载 stage 3 共用的 objects 过滤：键含
/// `minecraft/sounds/`、跳过 `music/` 与 `records/`；顺序按字母序倒着走。
pub fn filter_objects(objects: &serde_json::Map<String, Value>) -> Vec<(String, String)> {
    let mut keys: Vec<&String> = objects.keys().collect();
    keys.sort();
    keys.into_iter()
        .rev()
        .filter(|key| {
            if !key.contains("minecraft/sounds/") {
                return false;
            }
            // 原版注释：Skip music and records
            if key.contains("music/") || key.contains("records/") {
                return false;
            }
            true
        })
        .map(|key| {
            let hash = objects[key]
                .get("hash")
                .and_then(|h| h.as_str())
                .unwrap_or("");
            ((*key).clone(), hash.to_string())
        })
        .collect()
}

/// 原版 `sounds_directory` = exe 旁 `data/sounds`；重写版的采样目录解析链一致
/// （首选项自定义 > find_sounds_dir > exe 旁 Data/Sounds）。
pub fn sounds_dir(prefs: &nbs_io::prefs::Preferences) -> PathBuf {
    prefs
        .sounds_dir_path()
        .or_else(|| nbs_audio::find_sounds_dir(None))
        .unwrap_or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(|d| d.to_path_buf()))
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Data/Sounds")
        })
}

/// 原版 `mc_default_path`：GM 存档目录去尾段 + `.minecraft`。没有 game_save_id，
/// 直接取 `%LOCALAPPDATA%\.minecraft`（Windows）/ `~/.minecraft`。
fn default_mc_path() -> String {
    #[cfg(windows)]
    {
        if let Ok(base) = std::env::var("LOCALAPPDATA") {
            let base = base.trim_end_matches(['\\', '/']);
            return format!("{base}\\.minecraft");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return format!("{}/.minecraft", home.trim_end_matches('/'));
    }
    String::new()
}

/// HTTP 取回通道，宿主实现（原版 `http_get` / `http_get_file`，底层 WinINet）。
pub trait MinecraftFetcher: Send + Sync {
    fn fetch_text(&self, url: &str) -> Result<String, ()>;
    fn fetch_file(&self, url: &str, dst: &Path) -> Result<(), ()>;
}

/// 下载线程与 UI 共享的数据。锁在 [`DownloadHandle`] 上，Condvar 放锁外，
/// 等待时才能同时拿到两样。
pub struct DownloadShared {
    /// 1 拉版本清单；2 拉版本 JSON；3 拉索引 JSON；4 逐文件下载；0 空闲。
    pub stage: i32,
    /// 镜像原版 `sound_import_status`（0 空闲 / 1 获取中 / 2 完成）。
    pub status: i8,
    /// 版本清单里的 release id 列表 + 对应下载地址（一一对应）。
    pub indexes: Vec<String>,
    pub version_urls: Vec<String>,
    /// 版本清单就绪，等 UI pump 取走。
    pub indexes_ready: bool,
    /// (下载地址, 落盘路径) 文件清单。
    pub files: Vec<(String, PathBuf)>,
    /// 原版 `sound_import_download_files_index`：「下一个要发起的」下标。
    pub files_index: usize,
    pub count: u64,
    /// 覆盖确认已挂起，等 UI 回答（原版 message_yesnocancel 卡住整个循环）。
    pub replace_asked: bool,
    replace_decision: Option<bool>,
    /// 任一步失败（原版走 message + 复位，可点「获取音效」续传）。
    pub failed: bool,
    /// 线程已结束。
    pub finished: bool,
}

pub struct DownloadHandle {
    pub shared: Mutex<DownloadShared>,
    signal: Condvar,
}

fn lock<'a>(handle: &'a DownloadHandle) -> MutexGuard<'a, DownloadShared> {
    handle
        .shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 下载模式。
pub enum DownloadMode {
    /// 「获取」按钮：拉版本清单（原版 stage 1）。
    VersionList,
    /// 「获取音效」且没下到一半：从版本 JSON 开始全流程（原版 stage 2..4）。
    Import {
        version_url: String,
        sounds_root: PathBuf,
    },
    /// 失败续传：从 `files_index` 接着逐文件下载（原版 stage 4 续跑）。
    Resume,
}

/// 起后台线程；返回的 handle 存进 `SoundImport::pending`。
pub fn spawn_download(
    fetcher: Arc<dyn MinecraftFetcher>,
    mode: DownloadMode,
) -> Arc<DownloadHandle> {
    let handle = new_handle();
    spawn_on(handle.clone(), fetcher, mode);
    handle
}

/// 空的共享端（续传要复用旧 handle 上的文件清单）。
pub fn new_handle() -> Arc<DownloadHandle> {
    Arc::new(DownloadHandle {
        shared: Mutex::new(DownloadShared {
            stage: 0,
            status: 0,
            indexes: Vec::new(),
            version_urls: Vec::new(),
            indexes_ready: false,
            files: Vec::new(),
            files_index: 0,
            count: 0,
            replace_asked: false,
            replace_decision: None,
            failed: false,
            finished: false,
        }),
        signal: Condvar::new(),
    })
}

/// 在既有 handle 上起线程（Resume 用）。
pub fn spawn_on(
    handle: Arc<DownloadHandle>,
    fetcher: Arc<dyn MinecraftFetcher>,
    mode: DownloadMode,
) {
    std::thread::spawn(move || run_download(&handle, &fetcher, mode));
}

fn run_download(
    handle: &Arc<DownloadHandle>,
    fetcher: &Arc<dyn MinecraftFetcher>,
    mode: DownloadMode,
) {
    match mode {
        DownloadMode::VersionList => {
            // 原版 Get 按钮：`stage = 1; http_get(manifest); files_index = 0`。
            {
                let mut g = lock(handle);
                g.stage = 1;
                g.files_index = 0;
            }
            // 原版 stage 1 收尾：只收 type == "release" 的前 16 条
            // （`array_length > 15` 就 break）。
            let result = fetcher
                .fetch_text(MANIFEST_URL)
                .and_then(|body| serde_json::from_str::<Value>(&body).map_err(|_| ()));
            let mut g = lock(handle);
            match result {
                Ok(json) => {
                    if let Some(versions) = json.get("versions").and_then(|v| v.as_array()) {
                        for v in versions {
                            if v.get("type").and_then(|t| t.as_str()) == Some("release") {
                                if let (Some(id), Some(url)) = (
                                    v.get("id").and_then(|i| i.as_str()),
                                    v.get("url").and_then(|u| u.as_str()),
                                ) {
                                    g.indexes.push(id.to_string());
                                    g.version_urls.push(url.to_string());
                                }
                            }
                            if g.indexes.len() > 15 {
                                break;
                            }
                        }
                    }
                    g.indexes_ready = true;
                }
                Err(()) => g.failed = true,
            }
            g.stage = 0;
            g.finished = true;
        }
        DownloadMode::Import {
            version_url,
            sounds_root,
        } => {
            // 原版 stage 2 → 3：版本 JSON 拿 assetIndex.url，索引 JSON 建文件清单。
            {
                let mut g = lock(handle);
                g.stage = 2;
            }
            let objects = fetcher
                .fetch_text(&version_url)
                .and_then(|body| serde_json::from_str::<Value>(&body).map_err(|_| ()))
                .and_then(|json| {
                    json.get("assetIndex")
                        .and_then(|a| a.get("url"))
                        .and_then(|u| u.as_str())
                        .map(str::to_string)
                        .ok_or(())
                })
                .and_then(|url| fetcher.fetch_text(&url))
                .and_then(|body| serde_json::from_str::<Value>(&body).map_err(|_| ()));
            let mut g = lock(handle);
            let Some(json) = objects.ok() else {
                g.failed = true;
                g.stage = 0;
                g.finished = true;
                return;
            };
            g.stage = 3;
            let matching = json
                .get("objects")
                .and_then(|o| o.as_object())
                .map(filter_objects)
                .unwrap_or_default();
            g.count = matching.len() as u64;
            let mc_dir = sounds_root.join("minecraft");
            for (key, hash) in &matching {
                let rel = key.strip_prefix("minecraft/sounds/").unwrap_or(key);
                let url = format!(
                    "https://resources.download.minecraft.net/{}/{}",
                    &hash[..hash.len().min(2)],
                    hash
                );
                g.files.push((url, mc_dir.join(rel)));
            }
            // 覆盖确认：目录已存在才问（原版 message_yesnocancel，答「否/取消」整段作废）。
            // wait 会释放锁，UI 每帧照样能抄进度。
            if mc_dir.is_dir() {
                g.replace_asked = true;
                while g.replace_decision.is_none() {
                    g = handle
                        .signal
                        .wait(g)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
                if !g.replace_decision.take().unwrap() {
                    g.replace_asked = false;
                    g.stage = 0;
                    g.finished = true;
                    return;
                }
            }
            // 原版确认后：`directory_delete_lib(sounds_mc_subdir)`，随后 status = 1。
            let _ = std::fs::remove_dir_all(&mc_dir);
            g.status = 1;
            g.stage = 4;
            download_files(handle, fetcher, g);
        }
        DownloadMode::Resume => {
            let mut g = lock(handle);
            g.stage = 4;
            g.status = 1;
            download_files(handle, fetcher, g);
        }
    }
}

/// 原版 stage 4 的逐文件循环：先发起再 `files_index += 1`，失败回调里 `-=` 1，
/// 同步实现等价。全程不持锁等网络，UI 每帧都能抄到进度。
fn download_files<'a>(
    handle: &'a Arc<DownloadHandle>,
    fetcher: &Arc<dyn MinecraftFetcher>,
    mut g: MutexGuard<'a, DownloadShared>,
) {
    while g.files_index < g.files.len() {
        let (url, dst) = g.files[g.files_index].clone();
        g.files_index += 1;
        drop(g);
        let ok = fetcher.fetch_file(&url, &dst).is_ok();
        g = lock(handle);
        if !ok {
            g.files_index -= 1;
            g.failed = true;
            g.status = 0;
            g.stage = 0;
            g.finished = true;
            return;
        }
    }
    g.status = 2;
    g.files_index = 0;
    g.stage = 0;
    g.finished = true;
}

/// UI 侧状态（原版一组 `sound_import_*` 全局 + `mc_install_path`）。
pub struct SoundImport {
    /// 原版 `sound_import_download_toggle`。
    pub download_toggle: bool,
    /// 原版 `sound_import_status`（0 空闲 / 1 获取中 / 2 完成）。
    pub status: i8,
    /// 原版 `sound_import_asset_indexes`（下拉列表）。
    pub indexes: Vec<String>,
    pub select: usize,
    /// 原版 `sound_import_selected_asset_index`。
    pub selected: String,
    /// 原版 `sound_import_asset_index_count`。
    pub count: u64,
    /// 后台进度镜像（pump 每帧从共享抄）。
    pub files_index: usize,
    pub files_total: usize,
    /// 覆盖确认框已排过（防重复入队）。
    pub replace_asked: bool,
    /// 下载模式后台线程的共享端。
    pub pending: Option<Arc<DownloadHandle>>,
    /// HTTP 取回器，宿主注入；没有就点不动下载模式的按钮。
    pub fetcher: Option<Arc<dyn MinecraftFetcher>>,
    /// 原版 `mc_install_path`。
    pub mc_install_path: String,
    /// 原版 `mc_default_path`。
    pub mc_default_path: String,
    /// 版本清单下载地址（Import 模式用；pump 从共享抄来）。
    pub version_urls: Vec<String>,
    /// 拷贝/下载目标根目录（[`sounds_dir`] 算好传入；线程落盘与本地拷贝共用）。
    pub sounds_root: PathBuf,
    /// 本地覆盖确认待拷的清单（答「是」后由 [`SoundImport::copy_staged`] 落盘）。
    pub staged: Vec<(String, String)>,
}

impl Default for SoundImport {
    fn default() -> Self {
        let default_path = default_mc_path();
        Self {
            download_toggle: false,
            status: 0,
            indexes: Vec::new(),
            select: 0,
            selected: String::new(),
            count: 0,
            files_index: 0,
            files_total: 0,
            replace_asked: false,
            pending: None,
            fetcher: None,
            mc_install_path: default_path.clone(),
            mc_default_path: default_path,
            sounds_root: PathBuf::new(),
            version_urls: Vec::new(),
            staged: Vec::new(),
        }
    }
}

/// `load_asset_index` 的结果。
#[derive(Debug)]
pub enum LocateOutcome {
    None,
    /// 原版 message「未找到该索引所指向的文件！」（含解析失败——原版这里会崩）。
    MissingIndex,
    /// 目录已存在，需要覆盖确认；携带待拷清单。
    NeedsReplace(Vec<(String, String)>),
}

impl SoundImport {
    /// `update_asset_index_menu`：刷新列表与选中项；本地模式顺带
    /// `load_asset_index(false)` 数一遍。
    pub fn update_menu(&mut self) {
        if !self.download_toggle {
            self.indexes = find_asset_indexes(&self.mc_install_path);
        }
        if self.indexes.is_empty() {
            self.select = 0;
            self.selected.clear();
            self.count = 0;
        } else {
            if self.select > self.indexes.len() - 1 {
                self.select = 0;
            }
            self.selected = self.indexes[self.select].clone();
        }
        if !self.download_toggle {
            let _ = self.load_asset_index(false);
        }
    }

    /// `load_asset_index`：`copy = false` 只数数；`copy = true` 先过覆盖确认，
    /// 确认后的拷贝在 [`Self::copy_staged`]。
    pub fn load_asset_index(&mut self, copy: bool) -> LocateOutcome {
        if self.selected.is_empty() {
            return LocateOutcome::None;
        }
        let index_path = get_assets_dir(&self.mc_install_path)
            .join("indexes")
            .join(format!("{}.json", self.selected));
        let parsed = std::fs::read_to_string(&index_path)
            .ok()
            .and_then(|content| serde_json::from_str::<Value>(&content).ok());
        let Some(objects) = parsed
            .as_ref()
            .and_then(|json| json.get("objects"))
            .and_then(|o| o.as_object())
        else {
            return LocateOutcome::MissingIndex;
        };
        let matching = filter_objects(objects);
        if !copy {
            self.status = 0;
            self.count = matching.len() as u64;
            return LocateOutcome::None;
        }
        self.staged = matching;
        if sounds_mc_dir(&self.sounds_root).exists() {
            return LocateOutcome::NeedsReplace(std::mem::take(&mut self.staged));
        }
        LocateOutcome::None
    }

    /// 覆盖确认后的实际拷贝（原版 `load_asset_index` 后半段：删目录 → 逐文件复制）。
    pub fn copy_staged(&mut self, sounds_root: &Path) {
        let matching = std::mem::take(&mut self.staged);
        self.status = 1;
        let mc_dir = sounds_root.join("minecraft");
        let _ = std::fs::remove_dir_all(&mc_dir);
        let assets_dir = get_assets_dir(&self.mc_install_path);
        let mut copied = 0u64;
        for (key, hash) in &matching {
            if hash.len() < 2 {
                continue;
            }
            let src = assets_dir.join("objects").join(&hash[..2]).join(hash);
            let rel = key.strip_prefix("minecraft/sounds/").unwrap_or(key);
            let dst = mc_dir.join(rel);
            // 原版：src 不存在就 continue（连计数都跳过）。
            if !src.is_file() {
                continue;
            }
            let _ = std::fs::create_dir_all(dst.parent().unwrap_or(&mc_dir));
            let _ = std::fs::copy(&src, &dst);
            copied += 1;
        }
        self.status = 2;
        self.count = copied;
    }
}

/// 原版 `sounds_directory + "minecraft"`。
fn sounds_mc_dir(sounds_root: &Path) -> PathBuf {
    sounds_root.join("minecraft")
}

/// 下载模式的覆盖确认回答：唤醒后台线程继续或作废。模态框两条路都调。
pub fn answer_replace(si: &mut SoundImport, yes: bool) {
    si.replace_asked = false;
    let Some(handle) = &si.pending else {
        return;
    };
    let mut g = lock(handle);
    g.replace_decision = Some(yes);
    handle.signal.notify_all();
}

/// 每帧把后台线程的进度抄进 UI 状态；覆盖确认、失败提示在这里排模态框。
/// 窗口绘制时调用。
pub fn pump(state: &mut EditorState) {
    let si = &mut state.sound_import;
    let Some(handle) = si.pending.clone() else {
        return;
    };
    let mut g = lock(&handle);
    si.status = g.status;
    si.files_index = g.files_index;
    si.files_total = g.files.len();
    si.count = g.count;
    let failed = g.failed;
    let need_ask = g.replace_asked;
    let ready = g.indexes_ready;
    let (indexes, version_urls) = if ready {
        g.indexes_ready = false;
        (
            std::mem::take(&mut g.indexes),
            std::mem::take(&mut g.version_urls),
        )
    } else {
        (Vec::new(), Vec::new())
    };
    drop(g);

    if ready {
        si.version_urls = version_urls;
        // 下载模式的 update_asset_index_menu 后半段：清空/钳位/选中。
        si.indexes = indexes;
        if si.indexes.is_empty() {
            si.select = 0;
            si.selected.clear();
            si.count = 0;
        } else {
            if si.select > si.indexes.len() - 1 {
                si.select = 0;
            }
            si.selected = si.indexes[si.select].clone();
        }
    }
    if failed {
        // 标志当场清掉，否则续传成功后每一帧都会被旧失败态覆盖回去。
        lock(&handle).failed = false;
        // 原版：message 下载失败 + stage/status 复位（files_index 由线程留着续传）。
        si.status = 0;
        state.modals = ModalQueue::message(
            tr("Note Block Studio", "Note Block Studio"),
            tr(
                "The file could not be downloaded! Please try again.",
                "下载失败！请重试。",
            ),
        );
        return;
    }
    if need_ask && !si.replace_asked {
        si.replace_asked = true;
        state.modals = ModalQueue::question(
            tr("Warning", "警告"),
            tr(
                "An existing folder with imported Minecraft sounds has been found in your Sounds folder. Would you like to replace it?",
                "在您的Sounds文件夹中发现已经存在一个包含已导入Minecraft音效的文件夹，您想要覆盖它吗？",
            ),
            Command::SoundImportReplaceAnswered,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    /// 独立临时目录，避免并行用例互踩。
    fn scratch(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("nbs_sound_import_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 假取回器：文本按 URL 查表；文件落盘（可让前 N 次失败，测续传）。
    struct FakeFetcher {
        texts: HashMap<String, String>,
        fails_left: AtomicUsize,
    }

    impl FakeFetcher {
        fn new(texts: HashMap<String, String>) -> Self {
            Self {
                texts,
                fails_left: AtomicUsize::new(0),
            }
        }
    }

    impl MinecraftFetcher for FakeFetcher {
        fn fetch_text(&self, url: &str) -> Result<String, ()> {
            self.texts.get(url).cloned().ok_or(())
        }
        fn fetch_file(&self, _url: &str, dst: &Path) -> Result<(), ()> {
            if self.fails_left.load(Ordering::SeqCst) > 0 {
                self.fails_left.fetch_sub(1, Ordering::SeqCst);
                return Err(());
            }
            std::fs::create_dir_all(dst.parent().unwrap_or(dst)).map_err(|_| ())?;
            std::fs::write(dst, b"oggdata").map_err(|_| ())
        }
    }

    /// 轮询到条件成立为止；期间像 UI 一样每轮 pump 一次。
    fn wait_until(
        handle: &Arc<DownloadHandle>,
        state: &mut EditorState,
        pred: impl Fn(&DownloadShared) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            pump(state);
            let done = lock(handle);
            if pred(&done) {
                return;
            }
            drop(done);
            assert!(
                Instant::now() < deadline,
                "下载线程 10 秒内没有到达预期状态"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn object_entry(hash: &str) -> Value {
        serde_json::json!({ "hash": hash })
    }

    #[test]
    fn friendly_name_matches_the_original_table() {
        assert_eq!(friendly_name("1.8"), "1.8");
        assert_eq!(friendly_name("14w31a"), "1.8 Pre-Release 1");
        assert_eq!(friendly_name("1.14-af"), "3D Shareware v1.34");
        assert_eq!(friendly_name("_"), "26.1+");
        // 空串 → 空串（下拉未选中时的显示）
        assert_eq!(friendly_name(""), "");
        // 表外 → "26.1+ (id)" 兜底
        assert_eq!(friendly_name("99.9"), "26.1+ (99.9)");
    }

    #[test]
    fn thousands_grouping() {
        assert_eq!(format_thousands(0), "0");
        assert_eq!(format_thousands(999), "999");
        assert_eq!(format_thousands(1_234), "1,234");
        assert_eq!(format_thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn indexes_scan_sorts_unknown_first_then_by_table_order() {
        let base = scratch("scan");
        let indexes = base.join("assets").join("indexes");
        std::fs::create_dir_all(&indexes).unwrap();
        for name in ["1.8.json", "legacy.json", "custom.json", "notes.txt"] {
            std::fs::write(indexes.join(name), "{}").unwrap();
        }
        // 不在表里的排最前；表内的按表序（legacy 在 1.8 之前）。
        assert_eq!(
            find_asset_indexes(base.to_str().unwrap()),
            vec![
                "custom".to_string(),
                "legacy".to_string(),
                "1.8".to_string()
            ]
        );
        // 目录不存在 → 空表。
        assert!(find_asset_indexes("Z:/definitely/not/here").is_empty());
    }

    #[test]
    fn objects_filter_keeps_sounds_in_reverse_order() {
        let objects = serde_json::json!({
            "minecraft/sounds/block/note.ogg": object_entry("bb"),
            "minecraft/sounds/music/game/menu.ogg": object_entry("cc"),
            "minecraft/sounds/records/13.ogg": object_entry("dd"),
            "minecraft/textures/block.png": object_entry("ee"),
            "minecraft/sounds/step/grass1.ogg": object_entry("aa"),
            "minecraft/sounds/nohash.ogg": {}
        });
        let got = filter_objects(objects.as_object().unwrap());
        // 字母序倒着走：textures/records/music 被滤掉，剩 step → nohash → block。
        assert_eq!(
            got,
            vec![
                (
                    "minecraft/sounds/step/grass1.ogg".to_string(),
                    "aa".to_string()
                ),
                ("minecraft/sounds/nohash.ogg".to_string(), String::new()),
                (
                    "minecraft/sounds/block/note.ogg".to_string(),
                    "bb".to_string()
                ),
            ]
        );
    }

    /// 本地路完整流程：数数 →（无已有目录）拷贝 → 状态 2、计数、文件就位。
    #[test]
    fn local_flow_counts_then_copies() {
        let base = scratch("local_copy");
        let indexes = base.join("assets").join("indexes");
        let objects_dir = base.join("assets").join("objects").join("aa");
        std::fs::create_dir_all(&indexes).unwrap();
        std::fs::create_dir_all(&objects_dir).unwrap();
        std::fs::write(
            indexes.join("1.8.json"),
            serde_json::json!({
                "objects": {
                    "minecraft/sounds/step/a.ogg": object_entry("aabb"),
                    "minecraft/sounds/music/b.ogg": object_entry("ccdd")
                }
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(objects_dir.join("aabb"), b"hello").unwrap();

        let sounds_root = base.join("sounds");
        let mut si = SoundImport::default();
        si.mc_install_path = base.to_string_lossy().into_owned();
        si.sounds_root = sounds_root.clone();
        si.update_menu();
        assert_eq!(si.selected, "1.8");
        // music/ 被跳过，只数到 1。
        assert_eq!(si.count, 1);

        let outcome = si.load_asset_index(true);
        assert!(matches!(outcome, LocateOutcome::None));
        si.copy_staged(&sounds_root);
        assert_eq!(si.status, 2);
        assert_eq!(si.count, 1);
        assert_eq!(
            std::fs::read(sounds_root.join("minecraft").join("step").join("a.ogg")).unwrap(),
            b"hello"
        );
    }

    /// 已有目录 → 覆盖确认 → 确认后旧目录整个换掉。
    #[test]
    fn local_flow_asks_replace_then_replaces() {
        let base = scratch("local_replace");
        let indexes = base.join("assets").join("indexes");
        let objects_dir = base.join("assets").join("objects").join("aa");
        std::fs::create_dir_all(&indexes).unwrap();
        std::fs::create_dir_all(&objects_dir).unwrap();
        std::fs::write(
            indexes.join("1.8.json"),
            serde_json::json!({ "objects": { "minecraft/sounds/a.ogg": object_entry("aabb") } })
                .to_string(),
        )
        .unwrap();
        std::fs::write(objects_dir.join("aabb"), b"new").unwrap();

        let sounds_root = base.join("sounds");
        std::fs::create_dir_all(sounds_root.join("minecraft")).unwrap();
        std::fs::write(sounds_root.join("minecraft").join("old.ogg"), b"old").unwrap();

        let mut si = SoundImport::default();
        si.mc_install_path = base.to_string_lossy().into_owned();
        si.sounds_root = sounds_root.clone();
        si.update_menu();
        match si.load_asset_index(true) {
            LocateOutcome::NeedsReplace(list) => {
                si.staged = list;
            }
            other => panic!("应要求覆盖确认，实际 {other:?}"),
        }
        si.copy_staged(&sounds_root);
        assert_eq!(si.status, 2);
        assert!(!sounds_root.join("minecraft").join("old.ogg").exists());
        assert_eq!(
            std::fs::read(sounds_root.join("minecraft").join("a.ogg")).unwrap(),
            b"new"
        );
    }

    #[test]
    fn missing_index_file_is_reported() {
        let base = scratch("local_missing");
        std::fs::create_dir_all(base.join("assets").join("indexes")).unwrap();
        let mut si = SoundImport::default();
        si.mc_install_path = base.to_string_lossy().into_owned();
        si.selected = "ghost".to_string();
        assert!(matches!(
            si.load_asset_index(false),
            LocateOutcome::MissingIndex
        ));
    }

    /// stage 1：只收 type == "release" 的前 16 条。
    #[test]
    fn version_list_collects_first_16_releases() {
        let mut versions = Vec::new();
        versions.push(serde_json::json!({"type":"snapshot","id":"snap","url":"u-snap"}));
        for i in 0..17 {
            versions.push(
                serde_json::json!({"type":"release","id":format!("1.{i}"),"url":format!("u{i}")}),
            );
        }
        let mut texts = HashMap::new();
        texts.insert(
            MANIFEST_URL.to_string(),
            serde_json::json!({ "versions": versions }).to_string(),
        );
        let fetcher: Arc<dyn MinecraftFetcher> = Arc::new(FakeFetcher::new(texts));
        let handle = spawn_download(fetcher, DownloadMode::VersionList);
        let mut state = EditorState::for_test();
        state.prefs.language = 1;
        // pump 每帧抄的是 state.sound_import.pending 指着的共享端。
        state.sound_import.pending = Some(handle.clone());
        wait_until(&handle, &mut state, |g| g.finished);
        assert!(!lock(&handle).failed);
        assert_eq!(state.sound_import.indexes.len(), 16);
        assert_eq!(state.sound_import.version_urls.len(), 16);
        assert_eq!(state.sound_import.indexes[0], "1.0");
        assert_eq!(state.sound_import.version_urls[0], "u0");
        assert_eq!(state.sound_import.selected, "1.0");
    }

    /// 下载路完整流程：版本 JSON → 索引 → 建清单 → 无已有目录不问 → 全部落盘。
    #[test]
    fn download_flow_imports_all_files() {
        let base = scratch("dl_ok");
        let version_url = "http://fake/version/1.8.json".to_string();
        let index_url = "http://fake/index/1.8.json".to_string();
        let mut texts = HashMap::new();
        texts.insert(
            version_url.clone(),
            serde_json::json!({ "assetIndex": { "url": index_url } }).to_string(),
        );
        texts.insert(
            index_url.clone(),
            serde_json::json!({
                "objects": {
                    "minecraft/sounds/step/a.ogg": object_entry("aa"),
                    "minecraft/sounds/deep/step/b.ogg": object_entry("bb"),
                    "minecraft/sounds/music/c.ogg": object_entry("cc")
                }
            })
            .to_string(),
        );
        let fetcher: Arc<dyn MinecraftFetcher> = Arc::new(FakeFetcher::new(texts));
        let sounds_root = base.join("sounds");
        let handle = spawn_download(
            fetcher,
            DownloadMode::Import {
                version_url,
                sounds_root: sounds_root.clone(),
            },
        );
        let mut state = EditorState::for_test();
        state.prefs.language = 1;
        state.sound_import.pending = Some(handle.clone());
        wait_until(&handle, &mut state, |g| g.finished);
        let si = &state.sound_import;
        // music/ 不进清单。
        assert_eq!(si.files_total, 2);
        assert_eq!(si.status, 2);
        assert_eq!(si.files_index, 0);
        assert!(sounds_root
            .join("minecraft")
            .join("step")
            .join("a.ogg")
            .exists());
        assert!(sounds_root
            .join("minecraft")
            .join("deep")
            .join("step")
            .join("b.ogg")
            .exists());
    }

    /// 已有目录 → 覆盖确认框 → 答「否」整段作废，旧文件原样。
    #[test]
    fn download_flow_replace_decline_aborts() {
        let base = scratch("dl_decline");
        let version_url = "http://fake/version/x.json".to_string();
        let index_url = "http://fake/index/x.json".to_string();
        let mut texts = HashMap::new();
        texts.insert(
            version_url.clone(),
            serde_json::json!({ "assetIndex": { "url": index_url } }).to_string(),
        );
        texts.insert(
            index_url.clone(),
            serde_json::json!({
                "objects": { "minecraft/sounds/a.ogg": object_entry("aa") }
            })
            .to_string(),
        );
        let fetcher: Arc<dyn MinecraftFetcher> = Arc::new(FakeFetcher::new(texts));
        let sounds_root = base.join("sounds");
        std::fs::create_dir_all(sounds_root.join("minecraft")).unwrap();
        std::fs::write(sounds_root.join("minecraft").join("old.ogg"), b"old").unwrap();
        let handle = spawn_download(
            fetcher,
            DownloadMode::Import {
                version_url,
                sounds_root: sounds_root.clone(),
            },
        );
        let mut state = EditorState::for_test();
        state.prefs.language = 1;
        state.sound_import.pending = Some(handle.clone());
        // 等到覆盖确认框排出来（线程挂起等回答）。
        wait_until(&handle, &mut state, |g| g.replace_asked);
        assert!(state.sound_import.replace_asked);
        assert_eq!(state.modals.items.len(), 1);
        answer_replace(&mut state.sound_import, false);
        wait_until(&handle, &mut state, |g| g.finished);
        // 从没进下载阶段：status 0、旧文件还在。
        assert_eq!(state.sound_import.status, 0);
        assert_eq!(
            std::fs::read(sounds_root.join("minecraft").join("old.ogg")).unwrap(),
            b"old"
        );
    }

    /// 第一个文件失败 → 失败提示 + files_index 留在原地 → 续传补完。
    #[test]
    fn download_flow_failure_then_resume() {
        let base = scratch("dl_resume");
        let version_url = "http://fake/version/r.json".to_string();
        let index_url = "http://fake/index/r.json".to_string();
        let mut texts = HashMap::new();
        texts.insert(
            version_url.clone(),
            serde_json::json!({ "assetIndex": { "url": index_url } }).to_string(),
        );
        texts.insert(
            index_url.clone(),
            serde_json::json!({
                "objects": {
                    "minecraft/sounds/a.ogg": object_entry("aa"),
                    "minecraft/sounds/b.ogg": object_entry("bb")
                }
            })
            .to_string(),
        );
        let fetcher = Arc::new(FakeFetcher::new(texts));
        fetcher.fails_left.store(1, Ordering::SeqCst);
        let fetcher_dyn: Arc<dyn MinecraftFetcher> = fetcher.clone();
        let sounds_root = base.join("sounds");
        let handle = spawn_download(
            fetcher_dyn,
            DownloadMode::Import {
                version_url,
                sounds_root: sounds_root.clone(),
            },
        );
        let mut state = EditorState::for_test();
        state.prefs.language = 1;
        state.sound_import.pending = Some(handle.clone());
        wait_until(&handle, &mut state, |g| g.finished);
        assert!(!state.sound_import.replace_asked);
        assert_eq!(state.sound_import.status, 0);
        // 失败提示已排。
        assert_eq!(state.modals.items.len(), 1);
        // files_index 退回到失败那个文件（=0），续传从这里接着来。
        assert_eq!(lock(&handle).files_index, 0);
        // 续传：同一个 handle（文件清单还在）、同一个取回器（失败额度已用完）。
        // 派发层点「获取音效」续传时先复位标志再起线程，这里照做——否则首次失败
        // 留下的 finished=true 会让下面的等待提前返回。
        {
            let mut g = lock(&handle);
            g.failed = false;
            g.finished = false;
            g.stage = 4;
            g.status = 1;
        }
        spawn_on(handle.clone(), fetcher.clone(), DownloadMode::Resume);
        wait_until(&handle, &mut state, |g| g.finished);
        let si = &state.sound_import;
        assert_eq!(si.status, 2);
        assert_eq!(si.files_index, 0);
        assert!(sounds_root.join("minecraft").join("a.ogg").exists());
        assert!(sounds_root.join("minecraft").join("b.ogg").exists());
    }
}

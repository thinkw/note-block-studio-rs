//! Windows 原生文件对话框（`IFileOpenDialog` / `IFileSaveDialog`）。
//!
//! 原版是 GameMaker 直接调系统选框；重写版原先自绘了一套 egui 选框，这里换回真正的
//! 系统组件。标题和文件类型逐条照抄原版各调用点传给
//! `get_save_filename_ext` / `get_open_filename_ext` 的参数（见 `FileDialog::picker_spec`）。
//!
//! COM 起不来、或 CoCreateInstance 失败时返回 `PickResult::Unavailable`，
//! 编辑器回落到自绘的那套选框——功能不断，只是样子回到旧的。

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::ptr::null_mut;

use nbs_editor::state::{FileDialog, FilePicker, PickResult};
use winapi::ctypes::c_void;
use winapi::shared::minwindef::{DWORD, UINT};
use winapi::shared::windef::HWND;
use winapi::shared::winerror::{RPC_E_CHANGED_MODE, S_FALSE, S_OK};
use winapi::shared::wtypesbase::CLSCTX_INPROC_SERVER;
use winapi::um::combaseapi::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize};
use winapi::um::objbase::COINIT_APARTMENTTHREADED;
use winapi::um::shobjidl::{
    IFileDialog, IFileOpenDialog, IFileSaveDialog, FOS_CREATEPROMPT, FOS_FILEMUSTEXIST,
    FOS_FORCEFILESYSTEM, FOS_NOREADONLYRETURN, FOS_OVERWRITEPROMPT, FOS_PATHMUSTEXIST,
};
use winapi::um::shobjidl_core::{
    CLSID_FileOpenDialog, CLSID_FileSaveDialog, IShellItem, SHCreateItemFromParsingName,
    SIGDN_FILESYSPATH,
};
use winapi::um::shtypes::COMDLG_FILTERSPEC;
use winapi::um::winuser::{GetActiveWindow, GetForegroundWindow, GetWindowThreadProcessId};

/// 用户按「取消」时 `IFileDialog::Show` 的返回值，即 `HRESULT_FROM_WIN32(ERROR_CANCELLED)`。
const HR_CANCELLED: i32 = 0x8007_04C7u32 as i32;

fn wide<S: AsRef<OsStr>>(s: S) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

/// 主窗口句柄，让系统选框挂在它上面（原版就是模态叠在主窗口上）。
/// 点菜单时主窗口必然是活动的；万一拿不到就返回 NULL，对话框照样弹，只是不绑属主。
unsafe fn owner() -> HWND {
    let active = GetActiveWindow();
    if !active.is_null() {
        return active;
    }
    let fg = GetForegroundWindow();
    if fg.is_null() {
        return null_mut();
    }
    let mut pid: DWORD = 0;
    GetWindowThreadProcessId(fg, &mut pid);
    if pid == std::process::id() {
        fg
    } else {
        null_mut()
    }
}

/// 把目录路径包成 `IShellItem`，供 `SetFolder` 用。路径不存在或 COM 报错时返回 None。
unsafe fn shell_item(path: &OsStr) -> Option<*mut IShellItem> {
    let w = wide(path);
    let mut item: *mut c_void = null_mut();
    let iid = <IShellItem as winapi::Interface>::uuidof();
    let hr = SHCreateItemFromParsingName(w.as_ptr(), null_mut(), &iid, &mut item);
    if hr == S_OK && !item.is_null() {
        Some(item as *mut IShellItem)
    } else {
        None
    }
}

/// `SIGDN_FILESYSPATH` 取绝对路径；`GetDisplayName` 的缓冲是 COM 分配的，必须 `CoTaskMemFree`。
unsafe fn path_of(item: *mut IShellItem) -> Option<PathBuf> {
    if item.is_null() {
        return None;
    }
    let mut text: *mut u16 = null_mut();
    let ok = (*item).GetDisplayName(SIGDN_FILESYSPATH, &mut text) == S_OK && !text.is_null();
    if !ok {
        (*item).Release();
        return None;
    }
    let mut len = 0usize;
    while *text.add(len) != 0 {
        len += 1;
    }
    let s = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
    CoTaskMemFree(text as *mut c_void);
    (*item).Release();
    Some(PathBuf::from(s))
}

/// 配好标题、文件类型、起始目录、默认文件名。每个 Set* 都是同步调用，
/// 所以宽字符串只要活到本函数结束即可。
unsafe fn configure(dlg: &FileDialog, fd: *mut IFileDialog) {
    let spec = dlg.picker_spec();

    // 原版只有两种对话框：`get_open_filename_ext` 和 `get_save_filename_ext`，
    // 连「选文件夹」都是后者（按下保存再取那个占位名所在的目录），所以这里没有目录模式。
    let mut options = FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST;
    if dlg.save {
        options |= FOS_OVERWRITEPROMPT | FOS_CREATEPROMPT | FOS_NOREADONLYRETURN;
    } else {
        options |= FOS_FILEMUSTEXIST;
    }
    (*fd).SetOptions(options);

    let names: Vec<Vec<u16>> = spec.filters.iter().map(|f| wide(&f.name)).collect();
    let specs: Vec<Vec<u16>> = spec.filters.iter().map(|f| wide(&f.patterns)).collect();
    let entries: Vec<COMDLG_FILTERSPEC> = names
        .iter()
        .zip(specs.iter())
        .map(|(n, s)| COMDLG_FILTERSPEC {
            pszName: n.as_ptr(),
            pszSpec: s.as_ptr(),
        })
        .collect();
    if !entries.is_empty() {
        (*fd).SetFileTypes(entries.len() as UINT, entries.as_ptr());
    }

    let title = wide(&spec.title);
    (*fd).SetTitle(title.as_ptr());

    // 起始目录只在原版调用处真的传了目录时才设；传 `""` 的那几个（MIDI/Schematic 导入导出、
    // 数据包导出）本来就交给系统自己记上次的位置，我们不能替它决定。
    if dlg.start_dir && dlg.dir.is_dir() {
        if let Some(item) = shell_item(dlg.dir.as_os_str()) {
            (*fd).SetFolder(item);
            (*item).Release();
        }
    }
    if !dlg.name.is_empty() {
        let name = wide(&dlg.name);
        (*fd).SetFileName(name.as_ptr());
        // 用户没打扩展名时由系统补上，和 GameMaker 一致。多个类型时没有「那一个」扩展名，就不补。
        if let Some(only) = spec
            .filters
            .first()
            .map(|f| f.patterns.trim_start_matches("*."))
        {
            if !only.contains(';') && !only.is_empty() && spec.filters.len() == 1 {
                let ext = wide(only);
                (*fd).SetDefaultExtension(ext.as_ptr());
            }
        }
    }
}

pub struct WinFileDialogPicker;

impl FilePicker for WinFileDialogPicker {
    fn pick(&self, request: &FileDialog) -> PickResult {
        unsafe { pick_now(request) }
    }
}

unsafe fn pick_now(request: &FileDialog) -> PickResult {
    // 主线程可能已经被别人初始化过：S_FALSE = 已经初始化过（这次算我们持有的，要收尾），
    // RPC_E_CHANGED_MODE = 已被初始化成另一种模型（MTA），不归我们收尾但仍可用。
    let hr_init = CoInitializeEx(null_mut(), COINIT_APARTMENTTHREADED);
    if hr_init != S_OK && hr_init != S_FALSE && hr_init != RPC_E_CHANGED_MODE {
        return PickResult::Unavailable;
    }
    let must_uninit = hr_init == S_OK || hr_init == S_FALSE;

    let mut obj: *mut c_void = null_mut();
    let clsid = if request.save {
        &CLSID_FileSaveDialog
    } else {
        &CLSID_FileOpenDialog
    };
    let iid = if request.save {
        <IFileSaveDialog as winapi::Interface>::uuidof()
    } else {
        <IFileOpenDialog as winapi::Interface>::uuidof()
    };
    let hr = CoCreateInstance(clsid, null_mut(), CLSCTX_INPROC_SERVER, &iid, &mut obj);
    if hr != S_OK || obj.is_null() {
        if must_uninit {
            CoUninitialize();
        }
        return PickResult::Unavailable;
    }

    // IFileOpenDialog / IFileSaveDialog 的 vtable 都以 IFileDialog 打头（winapi 用 Deref 表达
    // 这层继承），所以取结果统一走 IFileDialog::GetResult，不必再 QueryInterface。
    let fd = obj as *mut IFileDialog;
    configure(request, fd);
    let hr_show = (*fd).Show(owner());
    let result = if hr_show == S_OK {
        let mut item: *mut IShellItem = null_mut();
        if (*fd).GetResult(&mut item) == S_OK {
            match path_of(item) {
                Some(p) => PickResult::Picked(p),
                None => PickResult::Cancelled,
            }
        } else {
            PickResult::Cancelled
        }
    } else if hr_show == HR_CANCELLED {
        PickResult::Cancelled
    } else {
        // 对话框自己出错（外壳扩展崩了等）。报 Unavailable，让编辑器换回自绘选框，
        // 用户这条导出还是走得下去。
        PickResult::Unavailable
    };
    (*fd).Release();
    if must_uninit {
        CoUninitialize();
    }
    result
}

/// 只跑配置、不弹窗：把 `configure` 原样执行一遍，再问对话框「你现在在哪个目录」。
/// 给 `examples/dialog_probe.rs` 用（那个 example 直接 include 本文件）——起始目录到底
/// 设没设进 Win32 组件，这样不用点鼠标也能实测。主程序里没人调它。
#[allow(dead_code)]
pub(crate) unsafe fn probe(request: &FileDialog) -> Result<String, String> {
    let hr_init = CoInitializeEx(null_mut(), COINIT_APARTMENTTHREADED);
    if hr_init != S_OK && hr_init != S_FALSE && hr_init != RPC_E_CHANGED_MODE {
        return Err(format!("CoInitializeEx 返回 {hr_init:#010x}"));
    }
    let must_uninit = hr_init == S_OK || hr_init == S_FALSE;
    let mut obj: *mut c_void = null_mut();
    let clsid = if request.save {
        &CLSID_FileSaveDialog
    } else {
        &CLSID_FileOpenDialog
    };
    let iid = if request.save {
        <IFileSaveDialog as winapi::Interface>::uuidof()
    } else {
        <IFileOpenDialog as winapi::Interface>::uuidof()
    };
    let hr = CoCreateInstance(clsid, null_mut(), CLSCTX_INPROC_SERVER, &iid, &mut obj);
    if hr != S_OK || obj.is_null() {
        if must_uninit {
            CoUninitialize();
        }
        return Err(format!("CoCreateInstance 返回 {hr:#010x}"));
    }
    let fd = obj as *mut IFileDialog;
    configure(request, fd);
    let mut item: *mut IShellItem = null_mut();
    let got = (*fd).GetFolder(&mut item);
    let where_am_i = match (got, path_of(item)) {
        (h, Some(p)) if h == S_OK => p.display().to_string(),
        (h, _) => format!("GetFolder 返回 {h:#010x}"),
    };
    (*fd).Release();
    if must_uninit {
        CoUninitialize();
    }
    Ok(where_am_i)
}

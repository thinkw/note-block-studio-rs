//! Windows 10/11 浮窗客户区的 DWM 合成背景。窗口本身仍按普通装饰窗口创建；
//! 透明时只给客户区加无模糊的 transparent gradient，标题栏、任务栏和边框不受影响。

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::ptr;

use winapi::shared::windef::HWND;
use winapi::um::libloaderapi::{FreeLibrary, GetProcAddress, LoadLibraryW};
use winapi::um::winuser::{
    FindWindowW, GetWindowLongPtrW, GWL_EXSTYLE, HWND_NOTOPMOST, HWND_TOPMOST, IsWindow,
    SetWindowLongPtrW, SetWindowPos, WS_EX_LAYERED, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
};

const WCA_ACCENT_POLICY: i32 = 19;
const ACCENT_DISABLED: i32 = 0;
const ACCENT_ENABLE_TRANSPARENTGRADIENT: i32 = 2;

fn accent_state(transparent: bool) -> i32 {
    if transparent {
        ACCENT_ENABLE_TRANSPARENTGRADIENT
    } else {
        ACCENT_DISABLED
    }
}

fn accent_flags(state: i32) -> i32 {
    if state == ACCENT_ENABLE_TRANSPARENTGRADIENT {
        2
    } else {
        0
    }
}

#[repr(C)]
struct AccentPolicy {
    state: i32,
    flags: i32,
    gradient_color: u32,
    animation_id: i32,
}

#[repr(C)]
struct WindowCompositionAttributeData {
    attribute: i32,
    data: *mut c_void,
    size_of_data: usize,
}

fn wide(value: &str) -> Vec<u16> {
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

pub fn find_by_title(title: &str) -> Option<isize> {
    let title = wide(title);
    let window = unsafe { FindWindowW(ptr::null(), title.as_ptr()) };
    (!window.is_null()).then_some(window as isize)
}

pub fn is_window(window: isize) -> bool {
    unsafe { IsWindow(window as HWND) != 0 }
}

/// 新窗口可靠地露出一次：短暂置顶再立刻取消，不改变用户设置的常态层级。
pub fn bring_to_front_once(window: isize) -> bool {
    let window = window as HWND;
    let flags = 0x0001 | 0x0002 | 0x0010 | 0x0040;
    unsafe {
        SetWindowPos(window, HWND_TOPMOST, 0, 0, 0, 0, flags);
        SetWindowPos(window, HWND_NOTOPMOST, 0, 0, 0, 0, flags);
    }
    true
}

fn passthrough_exstyle(exstyle: isize, enabled: bool) -> isize {
    let bits = (WS_EX_TRANSPARENT | WS_EX_LAYERED) as isize;
    if enabled {
        exstyle | bits
    } else {
        exstyle & !bits
    }
}

/// Windows 的命中穿透需要 WS_EX_TRANSPARENT + WS_EX_LAYERED；客户区背景仍由
/// DWM accent 负责，关闭穿透时把两位一起移除。
pub fn set_mouse_passthrough(window: isize, enabled: bool) -> bool {
    let window = window as HWND;
    let exstyle = unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) };
    if exstyle == 0 {
        return false;
    }
    let was_topmost = exstyle & WS_EX_TOPMOST as isize != 0;
    let next = passthrough_exstyle(exstyle, enabled);
    if next != exstyle {
        unsafe {
            SetWindowLongPtrW(window, GWL_EXSTYLE, next);
        }
    }
    unsafe {
        SetWindowPos(
            window,
            ptr::null_mut(),
            0,
            0,
            0,
            0,
            0x0001 | 0x0002 | 0x0004 | 0x0010 | 0x0020,
        );
        if was_topmost {
            // 改 layered 样式时 Windows 可能重置置顶 z-order；重新断言一次。
            SetWindowPos(
                window,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                0x0001 | 0x0002 | 0x0010 | 0x0040,
            );
        }
    }
    true
}

fn apply_accent(window: HWND, state: i32) -> bool {
    let mut accent = AccentPolicy {
        state,
        flags: accent_flags(state),
        gradient_color: 0,
        animation_id: 0,
    };
    let data = WindowCompositionAttributeData {
        attribute: WCA_ACCENT_POLICY,
        data: (&mut accent as *mut AccentPolicy).cast::<c_void>(),
        size_of_data: std::mem::size_of::<AccentPolicy>(),
    };
    let module_name = wide("user32.dll");
    let module = unsafe { LoadLibraryW(module_name.as_ptr()) };
    if module.is_null() {
        return false;
    }
    let proc = unsafe { GetProcAddress(module, c"SetWindowCompositionAttribute".as_ptr()) };
    let ok = if proc.is_null() {
        false
    } else {
        let apply: unsafe extern "system" fn(
            HWND,
            *const WindowCompositionAttributeData,
        ) -> i32 = unsafe { std::mem::transmute(proc) };
        unsafe { apply(window, &data) != 0 }
    };
    unsafe {
        FreeLibrary(module);
    }
    ok
}

pub fn set_transparent(window: isize, transparent: bool) -> bool {
    let window = window as HWND;
    let ok = apply_accent(window, accent_state(transparent));
    if ok {
        unsafe {
            // 0x0001 = SWP_NOSIZE；漏了它会把 cx/cy=0 当成新尺寸，浮窗压成 0×0。
            SetWindowPos(
                window,
                ptr::null_mut(),
                0,
                0,
                0,
                0,
                0x0001 | 0x0002 | 0x0004 | 0x0010 | 0x0020,
            );
        }
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparency_uses_gradient_not_acrylic() {
        assert_eq!(accent_state(true), 2, "客户区只用无模糊的 transparent gradient");
        assert_eq!(accent_flags(2), 2, "transparent gradient 必须带 flags 2");
        assert_eq!(accent_state(false), ACCENT_DISABLED);
        assert_eq!(accent_flags(ACCENT_DISABLED), 0);
        let base = 0x0004_0110;
        let bits = (WS_EX_TRANSPARENT | WS_EX_LAYERED) as isize;
        assert_eq!(passthrough_exstyle(base, true), base | bits);
        assert_eq!(passthrough_exstyle(passthrough_exstyle(base, true), false), base);
    }
}

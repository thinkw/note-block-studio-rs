//! Windows 上「运行中换窗口图标」的实现。
//!
//! 原版换图标走的是 GameMaker 的 `window_set_icon_raw` / `window_reset_icon_raw`
//! （`control_draw.gml:27-31`），标题栏和任务栏一起换。egui 那条路够不到任务栏：
//! `ViewportCommand::Icon` 到 winit 的 `set_window_icon` 只设 `ICON_SMALL`，
//! 而 eframe 里唯一会同时设 `ICON_BIG`/`ICON_SMALL` 的 `AppTitleIconSetter`
//! 只在启动那一次生效。所以开关切换之后，这里再补一次 `WM_SETICON`。

/// 把 `icon` 设成当前活动窗口的大/小图标。
///
/// 返回 `false` 表示窗口句柄还没就绪（`GetActiveWindow` 要等窗口建好、且归属本线程才非空），
/// 调用方下一帧重试 —— 与 eframe 的 `AppTitleIconSetter` 是同一套重试语义。
/// `made` 存本模块上一次交出去的 `HICON`，换完就销毁，不然每切一次漏两个 GDI 句柄。
#[cfg(windows)]
pub fn apply(icon: &egui::IconData, made: &mut [Option<usize>; 2]) -> bool {
    use winapi::um::winuser;

    // SAFETY: 只读查询，窗口尚未创建时返回空指针。
    let hwnd = unsafe { winuser::GetActiveWindow() };
    if hwnd.is_null() {
        return false;
    }

    let Some(src) = image::RgbaImage::from_raw(icon.width, icon.height, icon.rgba.clone()) else {
        return true; // 像素数对不上是数据本身的问题，重试也不会变好
    };

    // 顺序有讲究：先大后小。eframe 的注释说明只设 ICON_BIG 的话，
    // Windows 会自己去缩放小图标，效果很差。
    let mut handles = [0usize; 2];
    for (i, metric) in [winuser::SM_CXICON, winuser::SM_CXSMICON]
        .iter()
        .enumerate()
    {
        // SAFETY: 只读系统度量。
        let size = unsafe { winuser::GetSystemMetrics(*metric) }.max(1);
        // 原版两张源图都是 256x256，自己按目标尺寸 Lanczos 缩一遍再编码成 PNG，
        // 交给 CreateIconFromResourceEx —— 它能直接认 PNG，这样透明通道才正常。
        let scaled = image::imageops::resize(
            &src,
            size as u32,
            size as u32,
            image::imageops::FilterType::Lanczos3,
        );
        let mut png: Vec<u8> = Vec::new();
        if scaled
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .is_err()
        {
            return true;
        }
        // SAFETY: png 在调用期间保持存活，声明尺寸与编码进去的图一致。
        let hicon = unsafe {
            winuser::CreateIconFromResourceEx(
                png.as_mut_ptr(),
                png.len() as u32,
                1, // 1 = 图标，不是光标
                0x0003_0000,
                size,
                size,
                winuser::LR_DEFAULTCOLOR,
            )
        };
        if hicon.is_null() {
            return true;
        }
        handles[i] = hicon as usize;
        // SAFETY: 传进去的是刚由本函数创建、尚未被任何地方释放的句柄。
        unsafe {
            winuser::SendMessageW(
                hwnd,
                winuser::WM_SETICON,
                if i == 0 {
                    winuser::ICON_BIG as usize
                } else {
                    winuser::ICON_SMALL as usize
                },
                hicon as isize,
            )
        };
    }

    for (slot, handle) in made.iter_mut().zip(handles) {
        if let Some(old) = slot.replace(handle) {
            // SAFETY: old 是本模块上一轮 CreateIconFromResourceEx 的返回值，
            // 已经交给窗口、此后不会再被别处使用。
            unsafe {
                winuser::DestroyIcon(old as winapi::shared::windef::HICON);
            }
        }
    }
    true
}

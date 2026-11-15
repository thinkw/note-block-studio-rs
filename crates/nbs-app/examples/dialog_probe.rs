//! 原生文件对话框这一层的实测入口。
//!
//! ```text
//! cargo run -p nbs-app --example dialog_probe             # 不弹窗：核对标题、类型、起始目录
//! cargo run -p nbs-app --example dialog_probe -- show     # 真的弹一次，打印选中的路径
//! cargo run -p nbs-app --example dialog_probe -- folder   # 换成首选项「修改」那条请求
//! cargo run -p nbs-app --example dialog_probe -- nostart   # 换成原版传 `""`（不给起始目录）的那条
//! ```
//! 直接 `include` 主程序里那个模块（`nbs-app` 只有 bin 目标，没有 lib），
//! 保证探测走的和程序运行时是同一段代码。

#[cfg(windows)]
#[path = "../src/native_dialog.rs"]
mod native_dialog;

fn main() {
    #[cfg(windows)]
    {
        use nbs_editor::state::{DialogAction, FileDialog, FilePicker, PickResult};
        use std::path::PathBuf;

        let show = std::env::args().nth(1).as_deref() == Some("show");
        // 第二个开关测首选项那条「修改」：原版是过滤器为空串的保存框，不是目录选择器。
        let folder = std::env::args().nth(1).as_deref() == Some("folder");
        // 第三个开关测原版传 `""` 的那几个（从 Schematic 导入）：不给起始目录，
        // 位置该由系统组件自己记着，我们不许把当前工作目录塞进去。
        let nostart = std::env::args().nth(1).as_deref() == Some("nostart");
        let dir = PathBuf::from(r"D:\software\NBS\Songs");
        let dlg = if folder {
            FileDialog::for_action(DialogAction::PickFolder(nbs_editor::state::PrefsFolder::Song), None, Some(&dir))
        } else if nostart {
            FileDialog::for_action(DialogAction::ImportSchematic, None, None)
        } else {
            FileDialog::for_action(DialogAction::OpenSong, None, Some(&dir))
        };
        let spec = dlg.picker_spec();
        println!("标题: {}", spec.title);
        println!("保存式对话框: {}", dlg.save);
        println!("文件名栏: {:?}", dlg.name);
        println!("起始目录: {} (显式给定: {})", dlg.dir.display(), dlg.start_dir);
        for f in &spec.filters {
            println!("类型: {:?} -> {:?}", f.name, f.patterns);
        }
        if spec.filters.is_empty() {
            println!("类型: （原版过滤器是空串，不列任何类型）");
        }
        println!("本进程工作目录: {}", std::env::current_dir().unwrap().display());
        match unsafe { native_dialog::probe(&dlg) } {
            Ok(where_am_i) => println!("对话框实际所在: {where_am_i}"),
            Err(e) => println!("配置探测失败: {e}"),
        }
        if show {
            match native_dialog::WinFileDialogPicker.pick(&dlg) {
                PickResult::Picked(p) => println!("选中: {}", p.display()),
                PickResult::Cancelled => println!("取消"),
                PickResult::Unavailable => println!("系统组件不可用"),
            }
        }
    }
    #[cfg(not(windows))]
    println!("原生文件对话框只有 Windows 实现，这个探测程序在其它平台上没有内容可测。");
}

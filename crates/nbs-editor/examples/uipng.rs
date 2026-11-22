//! 无窗口渲染一帧 UI 并存成 PNG，用来和原版逐像素对照。
//!
//! ```text
//! cargo run -p nbs-editor --example uipng -- <song.nbs> <out.png> [宽] [高] [aqua|retro|dark|fluent]
//!
//! 环境变量 `NBS_UIPNG_WINDOW=<窗口名>[:页签]` 会先把那个对话框摆开再渲染，
//! 窗口名见下面 `match name`（preferences / instruments / datapack / …）。
//! `NBS_UIPNG_CROP=x0,y0,x1,y1[,倍率]` 只保存那块区域（逻辑像素）并可整数倍放大，
//! `NBS_UIPNG_DUMP=1` 打印绘制顺序（大块矩形、文字坐标），`NBS_UIPNG_PIX=x,y`
//! 打印写到某个像素上的每个图元，`NBS_UIPNG_MIDI=<路径>` 换 MIDI 导入窗口解析的文件，
//! `NBS_UIPNG_NOTE_DURATION=1` 打开 MIDI 导入窗口的「音符长度」（第四页签才会出现），
//! `NBS_UIPNG_MIDI_SCROLL=页签:行数` / `NBS_UIPNG_MIDI_MENU=ins|oct|pins|ppitch:行号`
//! 把 MIDI 导入窗口那张表滚到指定行 / 摆成某个下拉正打开的状态。
//! `NBS_UIPNG_POINTER=x,y` 每帧往该逻辑坐标发一次鼠标移动，用来渲染悬停态；
//! `NBS_UIPNG_FDARK=1` 打开 Fluent 的暗色模式；
//! `NBS_UIPNG_SCROLL=<tick>:<layer>` 把网格左上角滚到指定位置，验证可视区外的内容；
//! `NBS_UIPNG_KEY_PLAYED=<键号>:<秒>` 给某个琴键盖上「演奏后过了这么多秒」的时间戳，
//! 用来截图验证播放时琴键跟着弹下（染黄 + `floor(7*down)` 位移）。
//! `NBS_UIPNG_PREFS_MENU=theme|rate|output` 把首选项页里的某个下拉摆成打开状态
//! （前两个在「界面」页，`output` 在「播放」页）。
//! `NBS_UIPNG_WALLPAPER=<图片路径>` 先导入这张背景图片再渲染（原版「导入背景图片」那条路）。
//! `NBS_UIPNG_ZOOM=<倍>` 设界面缩放档（走 prefs，下一帧换算成 viewport.scale）；
//! `NBS_UIPNG_PANEL=<px>` 设图层栏宽度（#108：侧栏横轴不跟缩放，只能拖）；
//! `NBS_UIPNG_SELECT=1` 把第一个音符摘进浮动选区（宏条只在有选区时出现）。
//! `NBS_UIPNG_PIANO=<倍>` 设琴键栏独立缩放（`prefs.piano_scale`）；
//! `NBS_UIPNG_CHART=1` 打开「琴键五线谱」运行时开关（首选项出厂是关的）；
//! `NBS_UIPNG_PAN=1` 把帧驱动改成照 egui 自己报的「下次重绘延迟」走（鼠标只到位一次、
//! 之后不动，延迟为无穷大就断流），每帧打印起始槽 / 同屏键数 / 延迟，用来验证
//! 「鼠标停在琴键边缘不放，键盘是否还在继续平移」。
//! `NBS_UIPNG_DRAG=x0,y0,x1,y1` / `NBS_UIPNG_DBLCLICK=x,y` 补一段鼠标事件（拖动 /
//! 双击），用来验证图层栏右边界把手，跑完打印结果宽度。
//! `NBS_UIPNG_PPI=<倍>` 模拟系统的显示缩放（本机 Windows 是 150%），探针默认按 1.0 渲染，
//! 那样看不到亚像素落点带来的线宽跳变；`NBS_UIPNG_OFFSET=<px>` 把网格的亚刻位移
//! （`viewport.note_offset`，按帧跟随时每帧都在变）钉成给定值，用来逐帧对比竖线落点。
//! ```
//!
//! 走的是和 GUI 完全相同的一条路：`Context::run` → `tessellate` → 三角形。
//! 这里自带一个只处理三角形的小光栅器，因为本机远程桌面的截屏通道拿不到
//! glow(OpenGL) 窗口的内容。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::epaint::{ImageData, ImageDelta, Primitive, TextureId};
use egui::{Color32, Rect, Vec2};
use nbs_core::model::Note;
use nbs_editor::{EditorState, Theme, ThemeKind, WindowKind};

struct Tex {
    w: usize,
    h: usize,
    px: Vec<Color32>,
}

/// 把 egui 交给后端的贴图取回像素。字体图集只存覆盖率，白色 + alpha 就是 GPU 看到的内容。
fn to_tex(img: &ImageData) -> Tex {
    match img {
        ImageData::Color(ci) => Tex {
            w: ci.size[0],
            h: ci.size[1],
            px: ci.pixels.clone(),
        },
        ImageData::Font(f) => Tex {
            w: f.size[0],
            h: f.size[1],
            px: f
                .pixels
                .iter()
                .map(|a| Color32::from_white_alpha((*a * 255.0).clamp(0.0, 255.0) as u8))
                .collect(),
        },
    }
}

fn apply_delta(texs: &mut HashMap<TextureId, Tex>, id: TextureId, delta: &ImageDelta) {
    let new = to_tex(&delta.image);
    match delta.pos {
        None => {
            texs.insert(id, new);
        }
        Some([ox, oy]) => {
            let t = texs.entry(id).or_insert(Tex { w: new.w, h: new.h, px: vec![Color32::TRANSPARENT; new.w * new.h] });
            for y in 0..new.h {
                for x in 0..new.w {
                    let (dx, dy) = (ox + x, oy + y);
                    if dx < t.w && dy < t.h {
                        t.px[dy * t.w + dx] = new.px[y * new.w + x];
                    }
                }
            }
        }
    }
}

/// 采样：ppp 取 1 时字形 uv 与图集像素一一对应，最近邻足够用于对照。
fn sample(t: &Tex, uv: Vec2) -> Color32 {
    let x = (uv.x * t.w as f32) as usize;
    let y = (uv.y * t.h as f32) as usize;
    let x = x.min(t.w - 1);
    let y = y.min(t.h - 1);
    t.px[y * t.w + x]
}

fn blend(dst: &mut Color32, src: Color32) {
    let a = src.a() as f32 / 255.0;
    if a >= 1.0 {
        *dst = src;
        return;
    }
    if a <= 0.0 {
        return;
    }
    // `Color32` 的 rgb 通道存的就是预乘过 alpha 的值，所以「半透明盖在不透明上」
    // 是 `src + dst * (1 - a)`；再乘一次 a 会把所有半透明图元系统性画暗。
    let m = |d: u8, s: u8| (s as f32 + d as f32 * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
    *dst = Color32::from_rgb(m(dst.r(), src.r()), m(dst.g(), src.g()), m(dst.b(), src.b()));
}

/// 边函数：>0 表示 c 在 a→b 的左侧。
fn edge(a: Vec2, b: Vec2, c: Vec2) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

/// 重心插值一个通道。
fn lerp3(a: f32, b: f32, c: f32, w: [f32; 3]) -> f32 {
    a * w[0] + b * w[1] + c * w[2]
}

/// 某个采样点要写入的颜色：顶点色 × 贴图，直通 alpha（覆盖率由「命中几个采样点」体现）。
fn shade(v: &[egui::epaint::Vertex; 3], tex: Option<&Tex>, w: [f32; 3]) -> Color32 {
    let uv = Vec2::new(
        lerp3(v[0].uv.x, v[1].uv.x, v[2].uv.x, w),
        lerp3(v[0].uv.y, v[1].uv.y, v[2].uv.y, w),
    );
    let t = tex.map(|t| sample(t, uv)).unwrap_or(Color32::WHITE);
    let col = [0, 1, 2, 3].map(|k| {
        let g = |vv: Color32| match k {
            0 => vv.r(),
            1 => vv.g(),
            2 => vv.b(),
            _ => vv.a(),
        };
        lerp3(g(v[0].color) as f32, g(v[1].color) as f32, g(v[2].color) as f32, w)
    });
    // 四舍五入而不是截断：截断会让所有纯色填充系统性偏暗一格。
    let ch = |s: f32| (s.round().clamp(0.0, 255.0)) as u8;
    // 顶点色和贴图的 rgb 都已经是预乘 alpha 的（`Color32` 的存法），逐通道相乘
    // 得到的还是预乘值，所以这里必须用 `from_rgba_premultiplied` 原样存下，
    // 用 `from_rgba_unmultiplied` 会再乘一次 alpha。
    Color32::from_rgba_premultiplied(
        ch(t.r() as f32 * col[0] / 255.0),
        ch(t.g() as f32 * col[1] / 255.0),
        ch(t.b() as f32 * col[2] / 255.0),
        ch(t.a() as f32 * col[3] / 255.0),
    )
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize, def: &str| args.get(i).cloned().unwrap_or_else(|| def.to_string());
    let song = arg(0, "");
    let out = PathBuf::from(arg(1, "target/ui.png"));
    let w: f32 = arg(2, "1440").parse().map_err(|_| "宽不是数字")?;
    let h: f32 = arg(3, "900").parse().map_err(|_| "高不是数字")?;
    let kind = match arg(4, "fluent").as_str() {
        "aqua" => ThemeKind::Aqua,
        "retro" => ThemeKind::Retro,
        "dark" => ThemeKind::Dark,
        _ => ThemeKind::Fluent,
    };

    let mut state = EditorState::new();
    if !song.is_empty() && song != "-" {
        state.doc.replace_song(
            nbs_core::nbs::load(Path::new(&song)).map_err(|e| format!("载入 {song} 失败: {e}"))?,
        );
    }
    state.theme = Theme::new(kind);
    // `NBS_UIPNG_FDARK=1`：Fluent 的「暗色模式」（原版 `fdark`，和 state.rs 一样只对 theme 3 生效）。
    state.theme.fdark = kind == ThemeKind::Fluent && std::env::var("NBS_UIPNG_FDARK").is_ok();

    // `NBS_UIPNG_ZOOM=<倍>`：界面缩放档（走 prefs，`tick_transport` 每帧换算成 viewport.scale
    // 和 hires 贴图档）；`NBS_UIPNG_PANEL=<px>`：图层栏宽度（#108 的横轴脱离缩放）。
    if let Ok(z) = std::env::var("NBS_UIPNG_ZOOM") {
        state.prefs.window_scale = z.parse::<f32>().ok();
    }
    if let Ok(w) = std::env::var("NBS_UIPNG_PANEL") {
        state.prefs.layer_panel_width = w.parse::<f32>().unwrap_or(264.0);
    }
    // `NBS_UIPNG_PIANO=<倍>`：琴键栏独立缩放（不跟界面缩放，见 #85/#80）。
    if let Ok(s) = std::env::var("NBS_UIPNG_PIANO") {
        state.prefs.piano_scale = s.parse::<f32>().unwrap_or(1.0);
    }
    // `NBS_UIPNG_CHART=1`：打开「琴键五线谱」运行时开关（首选项出厂是关的）。
    if std::env::var("NBS_UIPNG_CHART").is_ok() {
        state.flags.show_notechart = true;
    }
    // `NBS_UIPNG_PAN=1`：见文件头，逐帧打印琴键平移相关的三个量。
    let pan_probe = std::env::var("NBS_UIPNG_PAN").is_ok();
    // `NBS_UIPNG_MONO=1`：关掉立体声（原版 `realstereo = 0`），声像图标消失、按钮整簇左移 10px。
    if std::env::var("NBS_UIPNG_MONO").is_ok() {
        state.prefs.mono = true;
    }

    // `NBS_UIPNG_SELECT=1`：把曲子里第一个音符按原版语义摘进浮动选区。
    // 宏条（图层栏底部那 15 个按钮）的显示条件是「有选区 + 显示琴键」（原版同语义），
    // 而 prefs 里琴键可能是关的，所以这里一并打开。
    if std::env::var("NBS_UIPNG_SELECT").is_ok() {
        state.prefs.show_piano = true;
        state.flags.show_piano = true;
        // 行上的 7 个按钮和宏条都要求「没有弹窗挡着」（原版同一个 `idle` 门），
        // 而载入歌曲可能按首选项自动弹出歌曲信息窗，这里一并关掉。
        state.window = None;
        let first = state.doc.song.iter_notes().next();
        if let Some((t, l, n)) = first {
            state.doc.song.remove(t, l);
            state.doc.selection = nbs_core::Selection {
                x: t as i64,
                y: l as i64,
                width: 1,
                height: 1,
                notes: vec![nbs_core::SelectionNote {
                    dx: 0,
                    dy: 0,
                    note: n,
                }],
            };
        }
    }

    // `NBS_UIPNG_SCROLL=<tick>:<layer>`：网格左上角（原版 `starta`/`startb`）。
    if let Ok(spec) = std::env::var("NBS_UIPNG_SCROLL") {
        let mut it = spec.split(':');
        state.viewport.start_tick = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        state.viewport.start_layer = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    }

    // `NBS_UIPNG_KEY_PLAYED=<键号>:<过了多少秒>`：给某个琴键盖「演奏后过了这么多秒」的
    // 时间戳。渲染循环每帧的 `input.time` 是自己给的（0.05s 递增），所以这里只解析，
    // 真正盖章要等到帧里知道 time 之后再做（见下面的循环）。
    let key_played = std::env::var("NBS_UIPNG_KEY_PLAYED").ok().and_then(|s| {
        let mut it = s.split(':');
        let key: usize = it.next()?.parse().ok()?;
        let elapsed: f64 = it.next().unwrap_or("0").parse().ok()?;
        Some((key, elapsed))
    });

    // NBS_UIPNG_WINDOW=preferences:2 —— 直接把某个对话框摆开渲染，用来逐像素对照原版窗口。
    // 冒号后面是页签序号（只有带页签的窗口用它）。
    if let Ok(spec) = std::env::var("NBS_UIPNG_WINDOW") {
        let mut it = spec.split(':');
        let name = it.next().unwrap_or("");
        let tab = it.next().and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
        let k = match name {
            "preferences" => WindowKind::Preferences,
            "instruments" => WindowKind::Instruments,
            "songinfo" => WindowKind::SongInfo,
            "songinfoedit" => WindowKind::SongInfoEdit,
            "properties" => WindowKind::Properties,
            "statistics" => WindowKind::Statistics,
            "shortcuts" => WindowKind::Shortcuts,
            "about" => WindowKind::About,
            "changelist" => WindowKind::Changelist,
            "update" => WindowKind::Update,
            "accent" => WindowKind::SetAccent,
            "set-velocity" => WindowKind::SetVelocity,
            "set-panning" => WindowKind::SetPanning,
            "set-pitch" => WindowKind::SetPitch,
            "tremolo" => WindowKind::Tremolo,
            "stereo" => WindowKind::Stereo,
            "clip-editor" => WindowKind::ClipEditor,
            "arpeggio" => WindowKind::Arpeggio,
            "portamento" => WindowKind::Portamento,
            "stagger" => WindowKind::Stagger,
            "audio-settings" => WindowKind::AudioSettings,
            "audio-export" => WindowKind::AudioExport,
            "midi-import" => WindowKind::MidiImport,
            "datapack" => WindowKind::DatapackExport,
            "schematic" => WindowKind::SchematicExport,
            "minecraft-compat" => WindowKind::MinecraftCompat,
            // 事件乐器编辑窗：草稿值按原版落格默认摆好（tempo=10 t/s → 150；消音器层 2 → 3）。
            "edit-tempo" => {
                state.event_edit.tempo = "150".into();
                WindowKind::EditTempoChanger(8, 0)
            }
            "edit-stopper" => {
                state.event_edit.start = "3".into();
                state.event_edit.until = "3".into();
                WindowKind::EditSoundStopper(8, 0)
            }
            other => return Err(format!("未知窗口 {other}")),
        };
        // 冒号后面的页签序号要落到对应窗口的那个字段上。
        match &k {
            WindowKind::MinecraftCompat => state.mc_tab = tab,
            WindowKind::DatapackExport => state.datapack_tab = tab,
            WindowKind::SchematicExport => state.schematic_tab = tab,
            WindowKind::MidiImport => state.midi_tab = tab as usize,
            _ => state.prefs_tab = tab,
        }
        // 原版没有空白的 MIDI 导入窗口：`open_midi()` 先解析成功才开。这里同样先解析
        // （`NBS_UIPNG_MIDI=<路径>`，默认取仓库里那首偶像曲），窗口才有内容可画。
        if k == WindowKind::MidiImport {
            let path = PathBuf::from(
                std::env::var("NBS_UIPNG_MIDI")
                    .as_deref()
                    .unwrap_or("assets/Songs/Entrusting this World to Idols ~ Idolatrize World.mid"),
            );
            let midi = nbs_io::midi::parse_midi_path(&path)
                .map_err(|e| format!("解析 {} 失败: {e}", path.display()))?;
            state.midi_options.reload_tables(&midi, &state.midi_gm);
            state.midi_file = path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            state.midi_data = Some(midi);
            // 原版第四页只在「音符长度」打开时存在，用它来单独渲染那一页。
            state.midi_options.note_duration |=
                std::env::var("NBS_UIPNG_NOTE_DURATION").is_ok();
            // 无头渲染喂不进鼠标，滚动位置和「点开某个下拉」用环境变量直接摆好。
            // 格式：`NBS_UIPNG_MIDI_SCROLL=页签:行数`、`NBS_UIPNG_MIDI_MENU=ins|oct|pins|ppitch:行号`。
            if let Ok(spec) = std::env::var("NBS_UIPNG_MIDI_SCROLL") {
                let mut it = spec.split(':');
                let page = it.next().unwrap_or("0").parse::<usize>().unwrap_or(0);
                let val = it.next().unwrap_or("0").parse::<f32>().unwrap_or(0.0);
                if let Some(slot) = state.midi_scroll.get_mut(page) {
                    *slot = val;
                }
            }
            if let Ok(spec) = std::env::var("NBS_UIPNG_MIDI_MENU") {
                let mut it = spec.split(':');
                let kind = it.next().unwrap_or("");
                let row = it.next().unwrap_or("0").parse::<usize>().unwrap_or(0);
                use nbs_editor::state::MidiMenu as M;
                state.midi_menu = Some(match kind {
                    "oct" => M::ChannelOct(row),
                    "pins" => M::PercIns(row),
                    "ppitch" => M::PercPitch(row),
                    _ => M::ChannelIns(row),
                });
            }
        }
        // `NBS_UIPNG_PREFS_MENU=theme|rate|output`：把首选项页里的某个下拉摆成打开状态。
        if matches!(k, WindowKind::Preferences) {
            match std::env::var("NBS_UIPNG_PREFS_MENU").as_deref() {
                Ok("rate") => state.prefs_rate_menu = true,
                Ok("theme") => state.prefs_theme_menu = true,
                Ok("output") => {
                    // 真打开菜单那一帧会刷设备表，摆状态也得刷。设备表是后台线程扫回来的
                    // （#94 治本：枚举一次 1.3 秒，不许卡在界面上），截图要等它落地，
                    // 否则画面里只剩「跟随系统默认」一项，看不出真实下拉长什么样。
                    for _ in 0..80 {
                        state.refresh_outputs();
                        if !state.output_list.is_empty() {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    state.prefs_output_menu = true;
                }
                _ => {}
            }
        }
        // 宏窗口照原版在绘制里自闸门：没有选区一帧就关窗。
        // 空歌时先落两个音符再全选，截图才有东西可画；载了歌就不动别人的谱面。
        let macro_window = matches!(
            k,
            WindowKind::Arpeggio
                | WindowKind::Portamento
                | WindowKind::Stagger
                | WindowKind::Tremolo
                | WindowKind::Stereo
                | WindowKind::SetVelocity
                | WindowKind::SetPanning
                | WindowKind::SetPitch
        );
        if macro_window && state.doc.selection.is_empty() {
            if state.doc.song.total_blocks == 0 {
                state.doc.place_note(0, 0, Note::new(0, 45));
                state.doc.place_note(1, 2, Note::new(0, 47));
            }
            state.doc.select_all();
        }
        state.window = Some(k);
    }

    let ctx = egui::Context::default();
    // `NBS_UIPNG_PPI=<倍>`：模拟系统的显示缩放。本机 Windows 是 150%（`AppliedDPI=120`），
    // 探针默认按 1.0 渲染，那样网格线的亚像素落点问题只会表现成「宽 1px」，看不出现实里
    // 1 逻辑像素 = 1.5 物理像素带来的逐帧粗细跳变。
    let ppi: f32 = std::env::var("NBS_UIPNG_PPI")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);
    // 只走下面 `ViewportInfo.native_pixels_per_point` 这一条路：egui 的缩放是
    // `zoom_factor × native`，`set_pixels_per_point` 改的是 zoom_factor，两者叠加会算成
    // 1.5×1.5=2.25（真实程序只报 native，zoom_factor 是 1.0）。
    state.theme.apply(&ctx);
    if !nbs_editor::fonts::install(&ctx) {
        eprintln!("提示: 本机找不到中文字体，截图里的中文会是空框");
    }
    // `NBS_UIPNG_WALLPAPER=<图片路径>`：走一遍和菜单「导入背景图片」完全相同的烘焙
    // （预缩放 → 降采样 → 虚化 → 上传贴图），用来看编辑区四周是否糊成渐变、暗化层盖了多少。
    if let Ok(path) = std::env::var("NBS_UIPNG_WALLPAPER") {
        state.wallpaper = Some(nbs_editor::background::bake(&ctx, Path::new(&path))?);
    }

    let mut texs: HashMap<TextureId, Tex> = HashMap::new();
    let mut prims = Vec::new();
    let dump = std::env::var("NBS_UIPNG_DUMP").is_ok();
    let mut ppp = 1.0f32;
    // `NBS_UIPNG_POINTER=x,y`：每帧往这个逻辑坐标发一次 `PointerMoved`，
    // 用来渲染「悬停某一行」的状态——菜单的选中底色、子菜单展开都靠悬停驱动。
    let pointer = std::env::var("NBS_UIPNG_POINTER").ok().and_then(|s| {
        let mut it = s.split(',');
        Some(egui::pos2(
            it.next()?.parse().ok()?,
            it.next()?.parse().ok()?,
        ))
    });
    // `NBS_UIPNG_DRAG=x0,y0,x1,y1` / `NBS_UIPNG_DBLCLICK=x,y`：在第 2..6 帧里喂一段
    // 按住左键的拖动 / 两次快速点击。#108 的图层栏右边界把手只有这两条路径能改宽度，
    // 光靠静态一帧截不出来，所以在这里把鼠标事件补上，跑完打印结果宽度。
    let drag = std::env::var("NBS_UIPNG_DRAG").ok().and_then(|s| {
        let v: Vec<f32> = s.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        (v.len() == 4).then_some((egui::pos2(v[0], v[1]), egui::pos2(v[2], v[3])))
    });
    let dblclick = std::env::var("NBS_UIPNG_DBLCLICK").ok().and_then(|s| {
        let mut it = s.split(',');
        Some(egui::pos2(
            it.next()?.parse().ok()?,
            it.next()?.parse().ok()?,
        ))
    });
    let btn = egui::PointerButton::Primary;
    let no_mod = egui::Modifiers::NONE;
    // 第一帧只为了建立布局（滚动条、面板尺寸），后面几帧才是稳定画面。
    // 必须把虚拟时间推过 egui 的 animation_time（0.1s）：Area 对刚打开的窗口做淡入，
    // 整个窗口子树会被乘上一个不到 1 的不透明度（area.rs 的 multiply_opacity），
    // 只渲染两帧 16ms 的话窗口底色会变成半透明，看起来就像「底色没画」。
    const FRAMES: u32 = 8;
    // `NBS_UIPNG_PAN=1` 的驱动方式：鼠标只在第 0 帧到位，之后一动不动，
    // 帧也不再固定 0.05s 推进，而是照 egui 报的「下次重绘延迟」走——
    // 它报 Duration::MAX 就断流，这一帧之后真实程序里不会再有帧。
    let mut vtime = 0.05f64;
    // 平移一步要 0.1 秒，8 帧只够看一步，所以这条路径把帧数拉长。
    let frames = if pan_probe { 24 } else { FRAMES };
    for frame in 0..frames {
        let mut events: Vec<egui::Event> = pointer
            .filter(|_| !pan_probe || frame == 0)
            .map(|p| vec![egui::Event::PointerMoved(p)])
            .unwrap_or_default();
        if let Some((a, b)) = drag {
            events.push(egui::Event::PointerMoved(a));
            let t = ((frame as f32 - 2.0) / 3.0).clamp(0.0, 1.0);
            let p = egui::pos2(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
            if frame == 2 {
                events.push(egui::Event::PointerButton {
                    pos: a,
                    button: btn,
                    pressed: true,
                    modifiers: no_mod,
                });
            }
            if (2..6).contains(&frame) {
                events.push(egui::Event::PointerMoved(p));
            }
            if frame == 5 {
                events.push(egui::Event::PointerButton {
                    pos: p,
                    button: btn,
                    pressed: false,
                    modifiers: no_mod,
                });
            }
        }
        if let Some(p) = dblclick {
            // 两次按下间隔 0.1s（< egui 的 0.3s 双击窗口），第二次会被判成 double_clicked。
            if matches!(frame, 2 | 4) {
                events.push(egui::Event::PointerMoved(p));
                events.push(egui::Event::PointerButton {
                    pos: p,
                    button: btn,
                    pressed: true,
                    modifiers: no_mod,
                });
            } else if matches!(frame, 3 | 5) {
                events.push(egui::Event::PointerButton {
                    pos: p,
                    button: btn,
                    pressed: false,
                    modifiers: no_mod,
                });
            }
        }
        let steady = 0.05 * (frame + 1) as f64;
        let secs = if pan_probe { vtime } else { steady };
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(w, h))),
            time: Some(secs),
            events,
            ..Default::default()
        };
        // egui 0.31 起「系统缩放」挪到了各视口自己的输入里（`ViewportInfo`），
        // `apply_ui_scale` 读的正是 `native_pixels_per_point`，根视口要填上。
        let vp_input = input.viewports.entry(egui::ViewportId::ROOT).or_default();
        vp_input.native_pixels_per_point = Some(ppi);
        // `NBS_UIPNG_OFFSET=<px>`：把网格的亚刻位移钉成给定值。按帧跟随时它就是每帧都在
        // 变的那个小数（`viewport.rs` 由 marker 算出），静态钉住等效于取播放中的一帧。
        if let Ok(o) = std::env::var("NBS_UIPNG_OFFSET") {
            if let Ok(v) = o.parse::<f32>() {
                state.viewport.note_offset = v;
            }
        }
        // 时间戳按本帧的虚拟时间倒推，绘制侧 `played_key(本帧 time - 时间戳)` 才落在窗口里。
        if let Some((k, e)) = key_played {
            if let Some(slot) = state.key_played.get_mut(k) {
                *slot = input.time.unwrap_or(0.0) - e;
            }
        }
        let out = ctx.run(input, |c| nbs_editor::show(c, &mut state));
        if pan_probe {
            let delay = out
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .map(|v| v.repaint_delay)
                .unwrap_or(std::time::Duration::MAX);
            eprintln!(
                "帧 {frame}: t={vtime:.3} piano_start={} keysshow={} delay={:.3}s",
                state.piano_start,
                state.piano_keys_shown,
                delay.as_secs_f64().min(99.0)
            );
            if delay == std::time::Duration::MAX {
                eprintln!("帧 {frame} 之后 egui 不再要求重绘：真实程序到这里就停帧了");
                break;
            }
            vtime += delay.as_secs_f64().clamp(0.016, 1.0);
        }
        ppp = out.pixels_per_point;
        if frame + 1 == frames {
            // 打印实际生效的缩放，免得把「egui 按 150% 栅格化」和「画布按 150% 换算」两件事搞混。
            eprintln!(
                "缩放: pixels_per_point={:.3} native={:?} viewport.scale={:.4} → 一格 {:.2} 物理像素",
                ppp,
                ctx.native_pixels_per_point(),
                state.viewport.scale,
                32.0 * state.viewport.scale * ppp
            );
        }
        for (id, delta) in &out.textures_delta.set {
            apply_delta(&mut texs, *id, delta);
        }
        for id in &out.textures_delta.free {
            texs.remove(id);
        }
        let shapes = out.shapes;
        if dump && frame + 1 == frames {
            eprintln!(
                "visuals: window_fill {:?} panel_fill {:?}",
                ctx.style().visuals.window_fill,
                ctx.style().visuals.panel_fill,
            );
            // shapes 的序号就是最终绘制顺序：按序列出所有大块矩形，查窗口底色有没有画。
            fn walk(sh: &egui::Shape, path: &str, n: &mut usize, clip: Rect) {
                match sh {
                    egui::Shape::Rect(r) if r.rect.width() * r.rect.height() > 8_000.0 => eprintln!(
                        "矩形 #{path} 填充 {:?} 描边 {:?} 圆角 {:?} 范围 ({:.0},{:.0})-({:.0},{:.0}) 裁剪 ({:.0},{:.0})-({:.0},{:.0})",
                        r.fill, r.stroke.color, r.corner_radius,
                        r.rect.min.x, r.rect.min.y, r.rect.max.x, r.rect.max.y,
                        clip.min.x, clip.min.y, clip.max.x, clip.max.y,
                    ),
                    egui::Shape::Vec(v) => {
                        for s in v {
                            *n += 1;
                            walk(s, &format!("{path}/{n}"), n, clip);
                        }
                    }
                    _ => {}
                }
            }
            for (k, cs) in shapes.iter().enumerate() {
                let mut n = 0usize;
                walk(&cs.shape, &format!("#{k}"), &mut n, cs.clip_rect);
            }
            for (n, cs) in shapes.iter().enumerate() {
                if let egui::Shape::Text(tx) = &cs.shape {
                    let s: String = tx.galley.text().chars().collect();
                    if !s.is_empty() {
                        eprintln!("文字 #{n} @({:.0},{:.0}) {s:?}", tx.pos.x, tx.pos.y);
                    }
                }
            }
        }
        prims = ctx.tessellate(shapes, ppp);
    }

    if std::env::var("NBS_UIPNG_DUMP").is_ok() {
        for (id, t) in &texs {
            let lit = t.px.iter().filter(|c| c.a() > 0).count();
            eprintln!(
                "贴图 {id:?}: {}×{} 左上角 {:?} 非透明像素 {lit}",
                t.w,
                t.h,
                t.px[0],
            );
        }
        let white = texs
            .get(&TextureId::default())
            .map(|t| t.px[0])
            .unwrap_or(Color32::BLACK);
        eprintln!("纯色图元取样到的「白」= {white:?}（必须是 alpha 255，否则所有纯色填充都会画成透明）");
        for (n, cp) in prims.iter().enumerate() {
            let Primitive::Mesh(mesh) = &cp.primitive else { continue };
            let b = mesh.calc_bounds();
            if b.width() * b.height() < 8_000.0 {
                continue;
            }
            let colors: Vec<Color32> = {
                let mut s: Vec<Color32> = mesh.vertices.iter().map(|v| v.color).collect();
                s.sort_by_key(|c| c.to_array());
                s.dedup();
                s
            };
            eprintln!(
                "图元 #{n} {}×{} 范围 ({:.0},{:.0})-({:.0},{:.0}) 裁剪 ({:.0},{:.0})-({:.0},{:.0}) 顶点色 {colors:?} 贴图 {:?}",
                b.width(), b.height(), b.min.x, b.min.y, b.max.x, b.max.y,
                cp.clip_rect.min.x, cp.clip_rect.min.y, cp.clip_rect.max.x, cp.clip_rect.max.y,
                mesh.texture_id,
            );
        }
    }

    if drag.is_some() || dblclick.is_some() {
        eprintln!("图层栏宽度 = {:.1}", state.prefs.layer_panel_width);
    }

    let (mut pw, mut ph) = ((w * ppp) as usize, (h * ppp) as usize);
    let bg = state.theme.window_bg;
    // 真实 GPU（eframe 默认开 MSAA）是逐采样点写入、每像素再平均的。
    // 若改成「每像素算一个覆盖率、再顺序混合」，同一个网格内部相邻三角形共享的那条
    // 对角边就会被两层部分覆盖叠加：窗口圆角的扇形三角上三层覆盖率 0.19+0.5+0.31=1，
    // 混合出来却只剩 0.72，于是整条对角线上出现原版没有的暗缝。
    // 所以这里也按采样点来：每像素 SS×SS 个采样点，全部图元写完再平均。
    const SS: usize = 4;
    const NS: usize = SS * SS;
    let mut sfb = vec![bg; pw * ph * NS];
    let mut tris = 0usize;

    // NBS_UIPNG_PIX="x,y"：把逐个写到该像素的图元全打出来，用来查「谁盖住了谁」。
    let trace = std::env::var("NBS_UIPNG_PIX").ok().and_then(|s| {
        let mut it = s.split(',');
        match (it.next()?.parse::<usize>().ok(), it.next()?.parse::<usize>().ok()) {
            (Some(x), Some(y)) => Some((x, y)),
            _ => None,
        }
    });

    for (tn, cp) in prims.iter().enumerate() {
        let Primitive::Mesh(mesh) = &cp.primitive else { continue };
        let clip = cp.clip_rect * ppp;
        let x0 = (clip.min.x.max(0.0)) as usize;
        let y0 = (clip.min.y.max(0.0)) as usize;
        let x1 = (clip.max.x.min(pw as f32 - 1.0)) as usize;
        let y1 = (clip.max.y.min(ph as f32 - 1.0)) as usize;
        let tex = texs.get(&mesh.texture_id);
        for i in (0..mesh.indices.len()).step_by(3) {
            let v: [egui::epaint::Vertex; 3] =
                [0, 1, 2].map(|k| mesh.vertices[mesh.indices[i + k] as usize]);
            let p: [Vec2; 3] = v.map(|x| Vec2::new(x.pos.x * ppp, x.pos.y * ppp));
            tris += 1;
            let area = edge(p[0], p[1], p[2]);
            if area.abs() < 1e-4 {
                continue;
            }
            let minx = p.iter().map(|q| q.x).fold(f32::MAX, f32::min).max(x0 as f32) as usize;
            let maxx = p.iter().map(|q| q.x).fold(f32::MIN, f32::max).min(x1 as f32) as usize;
            let miny = p.iter().map(|q| q.y).fold(f32::MAX, f32::min).max(y0 as f32) as usize;
            let maxy = p.iter().map(|q| q.y).fold(f32::MIN, f32::max).min(y1 as f32) as usize;
            // 内部像素走「全采样点命中」快速路径，压着边的像素逐采样点判定。
            let neg = area < 0.0;
            let side = [
                (p[1] - p[2]).length(),
                (p[2] - p[0]).length(),
                (p[0] - p[1]).length(),
            ];
            for y in miny..=maxy {
                for x in minx..=maxx {
                    let base = (y * pw + x) * NS;
                    let c = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let mut b = [
                        edge(p[1], p[2], c),
                        edge(p[2], p[0], c),
                        edge(p[0], p[1], c),
                    ];
                    if neg {
                        b = b.map(|e| -e);
                    }
                    // 像素中心到三条边的距离都超过 0.75px（角点最远 0.707px）时，
                    // 16 个采样点必然全在三角形内，直接复用中心点的重心坐标。
                    let whole = b.iter().enumerate().all(|(k, e)| *e >= 0.75 * side[k]);
                    let (hit, w) = if whole {
                        let sum = b[0] + b[1] + b[2];
                        (NS, b.map(|e| e / sum))
                    } else {
                        let mut n = 0usize;
                        let mut acc = [0.0f32; 3];
                        for sy in 0..SS {
                            for sx in 0..SS {
                                let q = Vec2::new(
                                    x as f32 + (sx as f32 + 0.5) / SS as f32,
                                    y as f32 + (sy as f32 + 0.5) / SS as f32,
                                );
                                let mut s = [
                                    edge(p[1], p[2], q),
                                    edge(p[2], p[0], q),
                                    edge(p[0], p[1], q),
                                ];
                                if neg {
                                    s = s.map(|e| -e);
                                }
                                if s.iter().all(|e| *e >= 0.0) {
                                    n += 1;
                                    let sum = s[0] + s[1] + s[2];
                                    let wsum = if sum > 0.0 { sum } else { 1.0 };
                                    for (a, e) in acc.iter_mut().zip(s) {
                                        *a += e / wsum;
                                    }
                                    blend(&mut sfb[base + sy * SS + sx], shade(&v, tex, s.map(|e| e / wsum)));
                                }
                            }
                        }
                        (n, acc.map(|e| e / n.max(1) as f32))
                    };
                    if hit == 0 {
                        continue;
                    }
                    let src = shade(&v, tex, w);
                    if trace == Some((x, y)) {
                        eprintln!(
                            "写 ({x},{y}) 图元#{tn} 三角形 {}/{} 顶点 ({:.1},{:.1})-({:.1},{:.1})-({:.1},{:.1}) area {area:.1} 命中采样点 {hit}/{NS} 平均权重 {w:?} 顶点色 {:?}{:?}{:?} src {src:?} 覆盖前 {:?}",
                            i / 3,
                            mesh.indices.len() / 3,
                            p[0].x, p[0].y, p[1].x, p[1].y, p[2].x, p[2].y,
                            v[0].color, v[1].color, v[2].color,
                            sfb[base],
                        );
                    }
                    if whole {
                        for s in 0..NS {
                            blend(&mut sfb[base + s], src);
                        }
                    }
                }
            }
        }
    }

    // MSAA resolve：每像素把 SS×SS 个采样点四舍五入平均（UI 底色不透明，逐通道平均即可）。
    let mut fb: Vec<Color32> = Vec::with_capacity(pw * ph);
    for n in 0..pw * ph {
        let base = n * NS;
        let mut acc = [0u32; 4];
        for s in 0..NS {
            for (a, c) in acc.iter_mut().zip(sfb[base + s].to_array()) {
                *a += c as u32;
            }
        }
        let avg = |a: u32| ((a * 2 + NS as u32) / (2 * NS as u32)) as u8;
        fb.push(Color32::from_rgba_unmultiplied(
            avg(acc[0]),
            avg(acc[1]),
            avg(acc[2]),
            avg(acc[3]),
        ));
    }

    // NBS_UIPNG_CROP="x0,y0,x1,y1[,整数倍率]"：只留下这块并放大，用来逐像素对照原版。
    if let Ok(spec) = std::env::var("NBS_UIPNG_CROP") {
        let n: Vec<usize> = spec.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        if n.len() >= 4 {
            let (x0, y0) = (n[0].min(pw), n[1].min(ph));
            let (x1, y1) = (n[2].clamp(x0 + 1, pw), n[3].clamp(y0 + 1, ph));
            let k = *n.get(4).unwrap_or(&1);
            let k = (1..=8).contains(&k).then_some(k).unwrap_or(1);
            let (cw, ch) = (x1 - x0, y1 - y0);
            let mut crop = vec![Color32::TRANSPARENT; cw * ch * k * k];
            for cy in 0..ch {
                for cx in 0..cw {
                    let c = fb[(y0 + cy) * pw + (x0 + cx)];
                    for sy in 0..k {
                        for sx in 0..k {
                            crop[(cy * k + sy) * cw * k + (cx * k + sx)] = c;
                        }
                    }
                }
            }
            pw = cw * k;
            ph = ch * k;
            fb = crop;
        }
    }

    let bytes: Vec<u8> = fb.iter().flat_map(|c| c.to_array()).collect();
    let img = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_vec(pw as u32, ph as u32, bytes)
        .ok_or_else(|| "帧缓冲尺寸与像素数不符".to_string())?;
    if let Some(d) = out.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    img.save(&out).map_err(|e| format!("写入 PNG 失败: {e}"))?;
    println!(
        "{}×{}，{} 个图元 / {} 个三角形 / {} 张贴图 → {}",
        pw,
        ph,
        prims.len(),
        tris,
        texs.len(),
        out.display()
    );
    Ok(())
}

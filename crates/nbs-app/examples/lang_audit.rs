//! T2e 收尾审计：把主界面、每个窗口、以及「只有做过操作才会出现」的文案都画一遍，
//! 抓出仍然画成中文（EN 模式）或画成英文（CN 模式）的漏网。
//!
//! 为什么不用 grep：文案漏网只看「实际画到屏幕上的字」。`tr(EN, 中文)` 的第二参数、
//! 注释、测试断言、CLI 输出里都有中文，grep 会把它们一起报出来；反过来多行写的
//! `tr(` 调用 grep 又会漏。这里直接取 egui 一帧的绘制结果，只看文字形状。
//!
//! 两类东西不算漏网，单独跳过（原版也不翻译它们）：
//! - 用户数据：歌曲名/作者/说明、层名、自定义音色名；
//! - 系统数据：Windows 报的输出设备名（「耳机 (Realtek(R) Audio)」这种）和
//!   `IO error:` 后面的系统报错（「系统找不到指定的路径。」），都随系统语言。
//!
//! 运行：
//!   cargo run -p nbs-app --example lang_audit          # EN 模式
//!   cargo run -p nbs-app --example lang_audit -- zh    # CN 模式反向查
use std::collections::HashSet;

use egui::{Context, Pos2, Rect, Shape, Vec2};
use nbs_core::model::Note;
use nbs_editor::state::{EditorState, WindowKind};

const W: f32 = 1440.0;
const H: f32 = 900.0;

fn walk(shape: &Shape, out: &mut Vec<String>) {
    match shape {
        Shape::Text(t) => out.push(t.galley.text().to_string()),
        Shape::Vec(list) => {
            for s in list {
                walk(s, out);
            }
        }
        _ => {}
    }
}

/// 画两帧（视口尺寸第二帧才稳），返回最后一帧画出的全部文字。
fn draw(state: &mut EditorState) -> Vec<String> {
    let ctx = Context::default();
    state.theme.apply(&ctx);
    ctx.set_pixels_per_point(1.0);
    let raw = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(W, H))),
        ..Default::default()
    };
    let mut texts = Vec::new();
    for frame in 0..2 {
        let out = ctx.run(raw.clone(), |ctx| nbs_editor::show(ctx, state));
        if frame == 1 {
            for clipped in &out.shapes {
                walk(&clipped.shape, &mut texts);
            }
        }
    }
    texts
}

fn has(hay: &str, want: bool) -> bool {
    hay.chars().any(|c| {
        let zh = ('\u{4e00}'..='\u{9fff}').contains(&c);
        zh == want
    })
}

/// 原版中文界面里同样保持英文的术语/快捷键/格式名（不是漏网）。
/// 逐条在 v3.12.0-rc.1 里核过的：
/// - `control_draw.gml:2459` 歌曲页 `bpm + " BPM"` 两条语言共用；
/// - `draw_window_schematic_export.gml:158` 中文分支仍写 `.schematic (pre 1.11)`；
/// - `dat_preview.gml:12` 存档包名占位 `<enter name>` 没有中文分支；
/// - `save_song.gml:54-55` 中文提示里也保留 `NBS v`。
/// 其余（Ctrl、WAV、Litematica、主题名）同理：是名称/按键，不是句子。
const EN_TERMS: &[&str] = &[
    "ctrl",
    "alt",
    "shift",
    "bpm",
    "t/s",
    "tick",
    "nbs",
    "nbt",
    "nbp",
    "midi",
    "wav",
    "mp3",
    "aiff",
    "flac",
    "ogg",
    "png",
    "lfo",
    "ok",
    "pre",
    "schematic",
    "litematica",
    "structure",
    "datapack",
    "fluent",
    "classic",
    "dark",
    "modern",
    "win10",
    "<enter name>",
];

/// 把已知术语去掉后，还剩多长的一段连续英文单词：够长才像「该翻译却没翻译」的句子。
/// 用最长单词而不是字母总数，`Ctrl+C/X/V` 这种按键组合（单个字母）才不会误报。
fn stray_word(t: &str) -> usize {
    let mut s = t.to_lowercase();
    for term in EN_TERMS {
        s = s.replace(term, " ");
    }
    s.chars()
        .fold((0, 0), |(run, best), c| {
            if c.is_ascii_alphabetic() {
                (run + 1, best.max(run + 1))
            } else {
                (0, best)
            }
        })
        .1
}

/// 原版在中文界面里也保持英文的东西，CN 模式不算漏网：内置音色名、宏名。
/// 依据是 `control_draw.gml:1848` 的中文宏菜单——只有淡入/淡出/替换音/设定音量/
/// 设定声道/设定音高/重置所有属性写了中文，Tremolo、Stereo、Chorus 等照旧英文。
fn kept_english(zh_mode: bool) -> HashSet<String> {
    let mut set: HashSet<String> = HashSet::new();
    if !zh_mode {
        return set;
    }
    for id in 0..nbs_core::model::FIRST_CUSTOM_INDEX {
        set.insert(nbs_core::instruments::instrument_name(id));
    }
    for m in nbs_core::Macro::all() {
        set.insert(m.name().to_string());
        set.insert(format!("Macro: {}", m.name()));
    }
    set
}

/// 报告和界面无关的一组串（读档报错、历史标签等）：返回漏掉的条数。
fn check(
    tag: &str,
    texts: &[String],
    zh_mode: bool,
    skip: &HashSet<String>,
    en_ok: &HashSet<String>,
) -> usize {
    let mut bad: Vec<&String> = texts
        .iter()
        // EN 模式找含汉字的；CN 模式反过来找「整条不含汉字、且去掉术语后还有一串英文」的。
        .filter(|t| {
            !skip.contains(*t)
                && !en_ok.contains(*t)
                && if zh_mode {
                    !has(t, true) && stray_word(t) >= 3
                } else {
                    has(t, true) && t.chars().count() > 3
                }
        })
        .collect();
    bad.sort();
    bad.dedup();
    if bad.is_empty() {
        println!("{tag:<34} 通过（{} 条文字）", texts.len());
        0
    } else {
        println!("{:<34} 漏网 {} / {} 条：", tag, bad.len(), texts.len());
        for t in &bad {
            println!("    {}", t.replace('\n', " ⏎ "));
        }
        bad.len()
    }
}

/// 用户数据 + 系统数据：这些不属于界面文案，跳过。
fn skip_set(state: &EditorState) -> HashSet<String> {
    let meta = &state.doc.song.meta;
    let mut set: HashSet<String> = [
        meta.name.clone(),
        meta.author.clone(),
        meta.original_author.clone(),
        meta.description.clone(),
        meta.midi_file.clone(),
        state.audio.device_info(),
    ]
    .into_iter()
    .collect();
    for l in &state.doc.song.layers {
        set.insert(l.name.clone());
    }
    for c in &state.doc.song.custom_instruments {
        set.insert(c.name.clone());
        set.insert(c.filename.clone());
    }
    set
}

fn state_with(lang: u8, window: Option<WindowKind>) -> EditorState {
    // 语言先钉好再构造：演示曲的说明是构造时按当前语言定型的。
    nbs_editor::set_language(lang);
    let mut state = EditorState::new();
    // show()/dispatch 每帧按 state.prefs.language 同步全局，这里也钉成待测语言。
    state.prefs.language = lang;
    state.flags.show_layers = true;
    state.flags.show_piano = true;
    state.prefs.show_layers = true;
    state.prefs.show_piano = true;
    // 原版带「无选区不开」闸门的那一族窗口（批量设置、颤音/立体声、琶音/滑音/错开）
    // 在空歌里 select_all 直接早退，窗口一画就自关，扫不到文案。先落两个音符。
    state.doc.place_note(0, 0, Note::new(0, 45));
    state.doc.place_note(1, 2, Note::new(0, 47));
    state.doc.select_all();
    state.window = window;
    state
}

/// 做过真实操作的状态：历史标签、宏名、缩放/炫彩提示只有这样才能画出来。
fn acted(lang: u8) -> EditorState {
    let mut state = state_with(lang, None);
    state.doc.place_note(2, 0, Note::new(0, 45));
    state.doc.select_rect(0, 0, 8, 3, false, false, true);
    state.doc.run_macro(&nbs_core::Macro::FadeIn);
    state.doc.select_rect(0, 0, 8, 3, false, false, true);
    state.doc.transpose(12);
    state.doc.undo();
    state
}

/// 只有触发一次才会出现的文案：状态栏提示、读档报错、历史标签。
fn extra_texts(state: &mut EditorState, ctx: &Context) -> Vec<String> {
    let mut out = Vec::new();
    let mut say = |state: &mut EditorState| {
        out.push(
            state
                .message
                .as_ref()
                .map(|(t, _)| t.clone())
                .unwrap_or_default(),
        );
    };
    state.step_window_scale(ctx, true);
    say(state);
    state.reset_window_scale(ctx);
    say(state);
    state.toggle_rainbow();
    say(state);
    state.toggle_rainbow();
    say(state);
    state.rainbow = false;
    // 读档失败会原样进状态栏，这里连报错文案一起验。
    // 只验我们自己写的那两句（版本不支持、读到一半断）；`IO error:` 后面那段是
    // Windows 按系统语言给的（「系统找不到指定的路径。」），不归我们翻译。
    for err in [
        nbs_core::nbs::load_bytes(&[0, 9, 0, 0, 0]).unwrap_err(),
        nbs_core::nbs::load_bytes(&[0, 1, 20, 5]).unwrap_err(),
    ] {
        out.push(format!("{err}"));
    }
    // 历史标签：撤销/重做按钮上就是这两条。
    out.extend(
        [
            state.doc.history.undo_label(),
            state.doc.history.redo_label(),
        ]
        .into_iter()
        .flatten()
        .map(|s| s.to_string()),
    );
    out
}

/// 白名单不能把真正的漏网也一起放过：这两组断言分别验「放过」和「仍然抓」。
fn selftest() {
    for ok in [
        "Ctrl+Shift+1",
        "10 t/s = 150 BPM",
        "Schematic",
        "NBS v6",
        "0 ticks",
    ] {
        assert!(stray_word(ok) < 3, "{ok} 应该算术语，不该报成漏网");
        assert!(!has(ok, true), "{ok} 不该含汉字");
    }
    for bad in ["Save the song", "Layer name", "Hold to move notes"] {
        assert!(stray_word(bad) >= 3, "{bad} 是整句英文，CN 模式必须抓到");
    }
}

fn main() {
    selftest();
    let zh_mode = std::env::args().nth(1).as_deref() == Some("zh");
    let lang = if zh_mode { 1 } else { 0 };
    println!(
        "模式：{}（检查方向：{}）\n",
        if zh_mode { "CN" } else { "EN" },
        if zh_mode {
            "画成英文的漏网"
        } else {
            "画成中文的漏网"
        }
    );

    let mut total = 0usize;
    let en_ok = kept_english(zh_mode);
    // (标签, 窗口, 首选项页签)
    let mut units: Vec<(String, Option<WindowKind>, u8)> = vec![("主界面".into(), None, 0)];
    let named: [(&str, WindowKind); 30] = [
        ("歌曲信息", WindowKind::SongInfo),
        ("歌曲属性", WindowKind::Properties),
        ("歌曲数据", WindowKind::Statistics),
        ("首选项", WindowKind::Preferences),
        ("乐器", WindowKind::Instruments),
        ("关于", WindowKind::About),
        ("设定音量", WindowKind::SetVelocity),
        ("设定声道", WindowKind::SetPanning),
        ("设定微调", WindowKind::SetPitch),
        ("颤音", WindowKind::Tremolo),
        ("立体声", WindowKind::Stereo),
        ("采样编辑器", WindowKind::ClipEditor),
        ("快捷键", WindowKind::Shortcuts),
        ("打开文件", WindowKind::FileOpen),
        ("保存文件", WindowKind::FileSave),
        ("层改名", WindowKind::LayerRename(0)),
        ("确认新建", WindowKind::ConfirmNew),
        ("导出音频", WindowKind::AudioExport),
        ("音频设置", WindowKind::AudioSettings),
        ("导出结构", WindowKind::DatapackExport),
        ("导入 MIDI", WindowKind::MidiImport),
        ("导出原理图", WindowKind::SchematicExport),
        ("导出 PNG", WindowKind::PngExport),
        ("琶音", WindowKind::Arpeggio),
        ("滑音", WindowKind::Portamento),
        ("错开", WindowKind::Stagger),
        ("自动存档", WindowKind::Autosaves),
        ("兼容性", WindowKind::MinecraftCompat),
        ("变速器编辑", WindowKind::EditTempoChanger(0, 0)),
        ("消音器编辑", WindowKind::EditSoundStopper(0, 0)),
    ];
    for (name, kind) in named {
        units.push((name.into(), Some(kind), 0));
    }
    // 首选项五个页签都要看一遍。
    for tab in 0..5u8 {
        units.push((
            format!("首选项页{}", tab + 1),
            Some(WindowKind::Preferences),
            tab,
        ));
    }

    for (tag, window, tab) in &units {
        let mut state = state_with(lang, window.clone());
        state.prefs_tab = *tab;
        let texts = draw(&mut state);
        total += check(tag, &texts, zh_mode, &skip_set(&state), &en_ok);
    }

    // 做过操作的界面：撤销/重做按钮带历史标签，宏条带宏名，状态栏带提示。
    let mut state = acted(lang);
    let mut texts = draw(&mut state);
    texts.extend(extra_texts(&mut state, &Context::default()));
    total += check(
        "操作后（含提示/历史/报错）",
        &texts,
        zh_mode,
        &skip_set(&state),
        &en_ok,
    );

    println!("\n合计漏网 {total} 条");
    if total > 0 {
        std::process::exit(1);
    }
}

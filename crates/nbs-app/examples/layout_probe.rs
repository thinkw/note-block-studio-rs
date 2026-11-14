//! 无窗口渲染几帧，把界面上每个贴图/矩形/文字的位置打出来，和原版的绝对像素坐标逐条核对。
//!
//! 原版界面全是硬编码像素（`control_draw.gml`），所以「按钮画在哪儿」是可以直接比对数字的事。
//! 这个探针在没有屏幕截图能力的环境里也能确认「真的画了东西、画在对的位置」。
//!
//! 运行：
//!   cargo run -p nbs-app --example layout_probe                 # 默认：图层栏开 + 钢琴开（Fluent）
//!   cargo run -p nbs-app --example layout_probe no_layers        # 收起图层栏：宏条改画在网格左下角（宽版）
//!   cargo run -p nbs-app --example layout_probe no_piano         # 收起钢琴：行区变高
//!   cargo run -p nbs-app --example layout_probe classic          # 经典主题：图标条/宏条改用 spr_iconbar 拉伸
//!   cargo run -p nbs-app --example layout_probe dump             # 默认帧的全部贴图清单

use egui::{Pos2, Rect, Shape, Vec2};

const W: f32 = 1440.0;
const H: f32 = 900.0;

#[derive(Clone, Copy, Debug)]
struct Item {
    kind: &'static str,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Item {
    fn new(kind: &'static str, r: Rect) -> Self {
        Self { kind, x: r.min.x, y: r.min.y, w: r.width(), h: r.height() }
    }
}

/// 收集一帧里所有形状的外框（贴图取顶点包围盒，矩形和文字取自身位置）。
fn walk(shape: &Shape, out: &mut Vec<Item>) {
    match shape {
        Shape::Mesh(mesh) => {
            let mut b = Rect::NOTHING;
            for v in &mesh.vertices {
                b = b.union(Rect::from_min_max(v.pos, v.pos));
            }
            if !b.is_negative() {
                out.push(Item::new("贴图", b));
            }
        }
        Shape::Rect(r) => out.push(Item::new("矩形", r.rect)),
        Shape::Text(t) => out.push(Item::new("文字", Rect::from_min_size(t.pos, Vec2::new(0.0, 0.0)))),
        Shape::Vec(list) => {
            for s in list {
                walk(s, out);
            }
        }
        _ => {}
    }
}

fn render(mode: &str) -> Vec<Item> {
    let mut state = nbs_editor::state::EditorState::new();
    let layers = mode != "no_layers";
    let piano = mode != "no_piano";
    state.flags.show_layers = layers;
    state.flags.show_piano = piano;
    state.prefs.show_layers = layers;
    state.prefs.show_piano = piano;
    // 宏条只在「有选区 + 开钢琴」时才画（原版 1496 `if (selected != 0 && show_piano)`），
    // 所以先把整首曲子选中，否则这一帧根本不会出现宏条。
    state.doc.select_all();
    if mode == "classic" {
        state.theme = nbs_editor::Theme::new(nbs_editor::ThemeKind::Aqua);
    }
    let ctx = egui::Context::default();
    state.theme.apply(&ctx);
    ctx.set_pixels_per_point(1.0);
    let raw = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(W, H))),
        ..Default::default()
    };
    // 视口尺寸是网格那一帧才写回 `state.viewport` 的，侧栏比网格先画，
    // 所以连跑两帧，取尺寸已经稳定的第二帧来核对。
    let mut items = Vec::new();
    for frame in 0..2 {
        let out = ctx.run(raw.clone(), |ctx| nbs_editor::show(ctx, &mut state));
        if frame == 1 {
            for clipped in &out.shapes {
                walk(&clipped.shape, &mut items);
            }
        }
    }
    items
}

/// 期望值：原版 `control_draw.gml` 在 1440×900、默认缩放下的锚点。
///
/// 顶栏 20 + 图标条带 34 ⇒ 图层栏头部带顶 54、行区顶 86。
/// 行数两边算法不同：
///   原版 `totalrows = floor((rh - rhval) / 32)`，rhval = 270（开钢琴）/ 130（收起）——经验值，
///     没把顶部 54 和状态栏 24 算全，收起钢琴时会多画一行，那一行被横向滚动条和状态栏压住；
///   本实现按真实可用区 `floor((可用高 - 时间轴 32 - 滚动条 32) / 32)`，永远不画半行。
/// 开钢琴时两式同为 19；收起钢琴时原版 24、本实现 23，这里按本实现核对，并把原版数字打出来备查。
struct Expect {
    rows_top: f32,
    rows: u32,
    gml_rows: u32,
    bar_y: f32,
    bar_w: f32,
    bar_h: f32,
    hbar_y: f32,
}

fn expect(mode: &str) -> Expect {
    const RH: f32 = 900.0;
    let piano = mode != "no_piano";
    let layers = mode != "no_layers";
    let reserved = 54.0 + 24.0 + if piano { 130.0 } else { 0.0 };
    let rows = (((RH - reserved - 64.0) / 32.0).floor() as u32).max(1);
    let gml_rows = ((RH - if piano { 270.0 } else { 130.0 }) / 32.0).floor() as u32;
    // 宏条底 = 最后一行的行框顶 + 32（开图层栏）/ + 52（收起）。
    let last_box_top = 85.0 + (rows as f32 - 1.0) * 32.0;
    Expect {
        rows_top: 85.0,
        rows,
        gml_rows,
        bar_y: if piano { last_box_top + if layers { 32.0 } else { 52.0 } } else { f32::NAN },
        bar_w: if layers { 255.0 } else { 380.0 },
        bar_h: if layers { 38.0 } else { 21.0 },
        hbar_y: 86.0 + rows as f32 * 32.0,
    }
}

fn report(mode: &str) {
    let items = render(mode);
    let e = expect(mode);
    println!("=== {mode} ===  形状 {} 个  行数 本实现 {} / 原版 {}", items.len(), e.rows, e.gml_rows);

    let near = |p: f32, q: f32| (p - q).abs() < 0.6;
    if mode == "no_layers" {
        println!("SKIP 图层行框          收起图层栏时原版不画任何行框（宏条仍在 x1 = 4）");
    } else {
        let rows: Vec<&Item> = items
            .iter()
            .filter(|i| near(i.x, 4.0) && (i.h - 34.0).abs() < 1.5 && i.y > 60.0 && i.y < 870.0)
            .collect();
        mark(rows.len() as u32 == e.rows, "图层行框数量", &format!("{}", rows.len()), &e.rows.to_string());
        if let (Some(f), Some(l)) = (rows.first(), rows.last()) {
            mark(
                near(f.y, e.rows_top) && near(l.y, e.rows_top + (e.rows as f32 - 1.0) * 32.0),
                "图层行框首/末 y",
                &format!("{:.0} / {:.0}", f.y, l.y),
                &format!("{:.0} / {:.0}", e.rows_top, e.rows_top + (e.rows as f32 - 1.0) * 32.0),
            );
        }
    }
    // 宏条底：经典主题是一张拉伸的 spr_iconbar（贴图），Fluent 是一个灰色圆角矩形。
    if e.bar_y.is_nan() {
        println!("SKIP 宏条            原版与本实现都在收起钢琴时不画宏条（`selected != 0 && show_piano`）");
    } else {
        let bar = items
            .iter()
            .find(|i| (i.y - e.bar_y).abs() < 2.0 && (i.w - e.bar_w).abs() < 6.0);
        match bar {
            Some(b) => mark(
                near(b.y, e.bar_y) && (b.h - e.bar_h).abs() < 1.5,
                "宏条底",
                &format!("y={:.1} {:.0}×{:.1}", b.y, b.w, b.h),
                &format!("y={:.1} {:.0}×{:.1}", e.bar_y, e.bar_w, e.bar_h),
            ),
            None => mark(false, "宏条底", "未找到", &format!("y={:.1}", e.bar_y)),
        }
        // 宏条按钮：原版 xx = x1 + 6、yy = y1 + 37（窄）/ + 55（宽），步长 25，第二行再下移 16。
        // 每个按钮会画两批顶点（底框 + 图标），所以按 x 去重后再数。
        let btn_y = e.bar_y + if mode == "no_layers" { 3.0 } else { 5.0 };
        let count_row = |y: f32| {
            let mut xs: Vec<f32> = items
                .iter()
                .filter(|i| (i.y - y).abs() < 1.0 && i.w < 20.0 && i.x > 4.0)
                .map(|i| i.x)
                .collect();
            xs.sort_by(|a, b| a.total_cmp(b));
            xs.dedup();
            xs.len()
        };
        mark(
            count_row(btn_y) == 10,
            "宏条第一行按钮数",
            &format!("{}", count_row(btn_y)),
            "10（原版 15 个分两行）",
        );
        mark(
            count_row(btn_y + 16.0) == 5,
            "宏条第二行按钮数",
            &format!("{}", count_row(btn_y + 16.0)),
            "5",
        );
    }
    // 横向滚动条：原版贴在行区之下。
    let hb = items.iter().find(|i| (i.y - e.hbar_y).abs() < 1.0 && i.h > 20.0 && i.w > 400.0);
    match hb {
        Some(i) => mark(
            true,
            "横向滚动条",
            &format!("y={:.0} {:.0}×{:.0}", i.y, i.w, i.h),
            &format!("y={:.0}", e.hbar_y),
        ),
        None => mark(false, "横向滚动条", "未找到", &format!("y={:.0}", e.hbar_y)),
    }
    // 状态栏文字：原版 `draw_text_dynamic(xx, rh - 18)`。
    match items.iter().filter(|i| i.kind == "文字" && i.y > 870.0).map(|i| i.y).reduce(f32::min) {
        Some(y) => mark(near(y, 882.0), "状态栏文字顶 y", &format!("{y:.1}"), "882.0 (rh-18)"),
        None => mark(false, "状态栏文字顶 y", "未找到", "882.0 (rh-18)"),
    }
    if mode == "dump" {
        println!("--- 全部形状（按 y、x 排序）---");
        let mut sorted = items.clone();
        sorted.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
        for i in &sorted {
            println!("{:<4} x={:>7.1} y={:>7.1}  {:.0}×{:.0}", i.kind, i.x, i.y, i.w, i.h);
        }
    }
}

fn mark(ok: bool, name: &str, got: &str, want: &str) {
    println!("{} {name:<14} 实测 {got:<28} 期望 {want}", if ok { "  OK" } else { "FAIL" });
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "default".into());
    report(&mode);
}

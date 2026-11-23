//! 播放特效底座。
//!
//! 一帧的分工：`timeline` 把曲子烘成「第几列在第几秒」，`FxStack` 按 z 序让每个
//! 启用中的特效往 [`stage::Frame`] 里塞图元，最后按 [`Surface`] 落笔。
//! 特效之间、特效与 egui 面板之间都不直接认识，所以加一种特效不用改 `app.rs`；
//! 内置特效与以后可能的第三方特效走的是同一个 trait。
//!
//! 两种表面：
//! - [`Surface::Overlay`]：叠在当前界面上。走 `LayerId::background()` 追加，
//!   和悬停五线谱（`piano::paint_notechart`）同一个套路 —— 压住网格与面板，
//!   但仍在弹窗之下。
//! - [`Surface::Stage`]：接管中央面板，网格这一帧不画。同一时刻只能有一个舞台，
//!   激活新的会自动换下旧的（这条规则在 [`FxStack::set_active`]，带一句回执）。
//!
//! 参数不写死在特效里：每个特效一份 `serde_json::Value`，随首选项持久化
//! （`Preferences::fx`），新键缺省由参数结构自己的 `#[serde(default)]` 补齐。

pub mod blockrow;
pub mod keyboard;
pub mod stage;
pub mod synthesia;
pub mod tiles;
pub mod timeline;

use egui::{Color32, Context, Id, LayerId, Order, Painter, Rect, TextureHandle, Ui};
use nbs_core::model::Song;
use std::sync::Arc;

use keyboard::{KeyboardLayout, BLACK_LIFT};
use stage::Frame;
use timeline::FxTimeline;
use tiles::{fallback_color, instrument_color, BlockTiles, InstrumentBlocks, NotePalette};

/// 特效画在哪一层。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// 叠在当前界面上。
    Overlay,
    /// 独占中央面板（网格这一帧不画）。
    Stage,
}

/// 特效的名字、位置和叠放规则。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectInfo {
    /// 持久化用的稳定 id：改名不能改它，否则用户的首选项会对不上。
    pub id: &'static str,
    /// 菜单里显示的中文名。
    pub name: &'static str,
    pub surface: Surface,
    /// 同一表面内的叠放顺序，小的先画（压在下面）。
    pub z: i8,
    /// 是否需要底部那条钢琴栏一起显示：轨道要按那条键盘摆放的特效都填 true，
    /// 舞台特效也一样（瀑布的胶囊就落在真键面上，画面里不会有两台键盘）。
    pub wants_piano: bool,
}

/// 一帧从界面拿到的输入，全是值：`FxState` 自己补 `timeline` / `blocks` / `tiles`，
/// 所以 `app.rs` 只要一次不可变借 `state`（凑齐这些数）就能再可变借 `state.fx` 落笔。
/// `area` / `keyboard` 两种表面给法不同：舞台 = 中央面板那块，叠加 = 整个窗口。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FxInput {
    /// 墙钟（秒）与帧步长：持续动效按它推进，和播放快慢无关。
    pub now: f64,
    pub dt: f32,
    /// 播放头（tick，浮点）与是否在播。
    pub position: f64,
    pub playing: bool,
    pub speed: f32,
    /// 像素倍率：琴键几何那些常量是屏幕像素（#80 起不随界面缩放），所以传 `piano_scale`。
    pub scale: f32,
    pub accent: Color32,
    /// 主界面编辑区那一块的底色（`theme.grid_bg`）：舞台特效想「和编辑区同色」就取这里。
    pub backdrop: Color32,
    /// 这一帧可画的区域：舞台给中央面板那块，叠加给整个窗口内容区。
    pub area: Rect,
    pub keyboard: Option<KeyboardLayout>,
}

/// 特效一帧能看到的东西。
pub struct FxContext<'a> {
    /// 贴图要用的 egui 上下文（舞台/叠加两条路径都能拿到）。
    pub tex: &'a Context,
    /// 本帧时刻与步长（秒）：粒子这类持续动效按它推进，与播放快慢无关。
    pub now: f64,
    pub dt: f32,
    /// 播放头（tick，浮点）与是否在播。
    pub position: f64,
    pub playing: bool,
    /// 倍速（1.0 = 原速）。
    pub speed: f32,
    /// 像素倍率（`piano_scale`，见 [`FxInput::scale`]）。
    pub scale: f32,
    /// 主题强调色（跟随主题与彩虹循环）。
    pub accent: Color32,
    /// 主界面编辑区的底色（`FxInput::backdrop`），舞台底色跟着主题走时用。
    pub backdrop: Color32,
    /// 可画区域。舞台 = 中央面板那块；叠加 = 整个窗口。
    pub area: Rect,
    /// 底部钢琴栏的摆放。钢琴栏关掉时是 `None`，舞台特效自己 `KeyboardLayout::fit`。
    pub keyboard: Option<KeyboardLayout>,
    pub timeline: &'a FxTimeline,
    /// 乐器 → 方块名（与导出同源）。
    pub blocks: &'a InstrumentBlocks,
    /// 方块贴图。素材没扒到时是 `None` 或空表，特效自动退回色块。
    pub tiles: Option<&'a BlockTiles>,
    /// 主界面画音符用的那行精灵的平均色：特效要「和编辑区同色」就取这里，
    /// 空表（素材没入库）时自己退回按乐器名哈希。
    pub palette: &'a NotePalette,
}

/// 一个播放特效。实现者只产图元，不碰 `Painter`。
pub trait PlaybackEffect {
    fn info(&self) -> EffectInfo;

    /// 产出这一帧的图元。`out` 已被底座清空并按 z 序排好前面的特效。
    fn draw(&mut self, cx: &FxContext, out: &mut Frame);

    /// 参数快照（进首选项）。
    fn params(&self) -> serde_json::Value {
        serde_json::Value::Null
    }

    /// 从首选项恢复参数；非法字段由 `#[serde(default)]` 兜住。
    fn set_params(&mut self, _value: &serde_json::Value) {}

    /// 调参控件。返回 `true` 表示参数改了，底座据此把首选项标脏。
    fn settings_ui(&mut self, _ui: &mut Ui) -> bool {
        false
    }

    /// 一句需要用户知道的降级说明（素材没扒到之类），没有就 `None`。
    fn note(&self) -> Option<String> {
        None
    }

    /// 舞台接管时，底部琴键栏那条面板的底色该用什么。
    /// 面板比可见琴键高出一截（黑键上探的 7px），这段本来什么都不画，露出来的是
    /// 窗口底色 —— 浅色界面里看不出来，深色舞台往上一压就是一条白边。跟舞台
    /// 底色同源才接得上。`grid_bg` 是主题编辑区的底色，没有特别要求的照旧用它。
    fn backdrop(&self, grid_bg: Color32) -> Color32 {
        grid_bg
    }
}

/// 当前启用中的特效集合，按 z 序合成。
pub struct FxStack {
    effects: Vec<Box<dyn PlaybackEffect>>,
    active: Vec<String>,
    frame: Frame,
    /// 叠到界面上时的整体透明度。
    opacity: f32,
    /// 上一帧是否因为图元太多被截断（用来说一句提示）。
    overflowed: bool,
}

impl Default for FxStack {
    fn default() -> Self {
        Self::new()
    }
}

impl FxStack {
    /// 注册内置特效。第三方特效以后从这里接进来（同一个 `Vec`）。
    pub fn new() -> Self {
        let effects: Vec<Box<dyn PlaybackEffect>> = vec![
            Box::<synthesia::Synthesia>::default(),
            Box::<blockrow::BlockRow>::default(),
        ];
        Self {
            effects,
            active: Vec::new(),
            frame: Frame::default(),
            opacity: 1.0,
            overflowed: false,
        }
    }

    /// 所有特效的 id（菜单用，按 z 排序，稳定显示）。
    pub fn ids(&self) -> Vec<&'static str> {
        let mut infos: Vec<(i8, &'static str)> = self
            .effects
            .iter()
            .map(|e| (e.info().z, e.info().id))
            .collect();
        infos.sort_by_key(|(z, id)| (*z, *id));
        infos.into_iter().map(|(_, id)| id).collect()
    }

    pub fn info(&self, id: &str) -> Option<EffectInfo> {
        self.effects.iter().find(|e| e.info().id == id).map(|e| e.info())
    }

    pub fn is_active(&self, id: &str) -> bool {
        self.active.iter().any(|a| a == id)
    }

    pub fn active(&self) -> &[String] {
        &self.active
    }

    /// 启用中的特效有没有画在某个表面上的。`app.rs` 靠它决定「这一帧谁在画」：
    /// 舞台那条把网格顶掉，叠加那条反过来要求网格照画 —— 两条都省掉空转。
    pub fn surface_active(&self, surface: Surface) -> bool {
        self.active
            .iter()
            .any(|id| self.info(id).is_some_and(|i| i.surface == surface))
    }

    pub fn set_opacity(&mut self, opacity: f32) {
        self.opacity = opacity.clamp(0.05, 1.0);
    }

    pub fn opacity(&self) -> f32 {
        self.opacity
    }

    /// 启用/停用某个特效。返回给用户看的一句回执（舞台互斥换下了谁、或不认识这个 id）。
    pub fn set_active(&mut self, id: &str, on: bool) -> Option<String> {
        let Some(surface) = self.info(id).map(|i| i.surface) else {
            return Some(format!("不认识特效「{id}」"));
        };
        if !on {
            let before = self.active.len();
            self.active.retain(|a| a != id);
            return (self.active.len() != before).then(|| format!("已关闭「{}」", self.name(id)));
        }
        if self.is_active(id) {
            return None;
        }
        // 舞台一次只能有一个：接管中央面板的路径只画一个舞台，
        // 两个同时开会后画的盖住前一个，所以直接换人而不是排队。
        let replaced: Option<String> = if surface == Surface::Stage {
            let other = self
                .active
                .iter()
                .filter(|a| self.info(a).map(|i| i.surface) == Some(Surface::Stage))
                .find(|a| a.as_str() != id)
                .cloned();
            if let Some(old) = &other {
                self.active.retain(|a| a != old);
            }
            other
        } else {
            None
        };
        self.active.push(id.to_owned());
        self.sort_active();
        let mut msg = format!("已开启「{}」", self.name(id));
        if let Some(old) = replaced {
            msg.push_str(&format!("，同时换下舞台特效「{}」", self.name(&old)));
        }
        Some(msg)
    }

    pub fn toggle(&mut self, id: &str) -> Option<String> {
        let on = !self.is_active(id);
        self.set_active(id, on)
    }

    fn name(&self, id: &str) -> String {
        self.info(id).map(|i| i.name.to_string()).unwrap_or_else(|| id.to_string())
    }

    /// 启用中的舞台特效（至多一个）。
    pub fn stage_id(&self) -> Option<String> {
        self.active
            .iter()
            .find(|a| self.info(a).map(|i| i.surface) == Some(Surface::Stage))
            .cloned()
    }

    /// 琴键面板在舞台接管时该填的底色：取启用中那个舞台特效的口径（[`PlaybackEffect::backdrop`]）。
    /// 没有舞台在画时也返回主题底色 —— 面板填不填由 `app.rs` 另行判断。
    pub fn stage_backdrop(&self, grid_bg: Color32) -> Color32 {
        self.stage_id()
            .and_then(|id| {
                self.effects
                    .iter()
                    .find(|e| e.info().id == id)
                    .map(|e| e.backdrop(grid_bg))
            })
            .unwrap_or(grid_bg)
    }

    /// 有没有特效要求底部钢琴栏跟着显示。
    pub fn wants_piano(&self) -> bool {
        self.active
            .iter()
            .any(|a| self.info(a).is_some_and(|i| i.wants_piano))
    }

    /// 是否需要在状态栏/特效窗里提一句「图元太多被截断」。
    pub fn took_too_many(&self) -> bool {
        self.overflowed
    }

    fn sort_active(&mut self) {
        // 排序的比较闭包借的是 `self.active`，在里面再调 `self.info()` 就成了
        // 「同时可变借 + 不可变借」。先把 z 抄成局部表再排。
        let table: Vec<(&'static str, i8)> = self
            .effects
            .iter()
            .map(|e| {
                let info = e.info();
                (info.id, info.z)
            })
            .collect();
        self.active.sort_by_key(|id| {
            (
                table
                    .iter()
                    .find(|(eid, _)| *eid == id)
                    .map_or(0, |(_, z)| *z),
                id.clone(),
            )
        });
    }

    /// 把参数灌进各特效，并把当前参数写回首选项。返回是否发生了写入。
    /// 只适合开机用：它是「先读后写」，调参之后再走一遍会把刚滑出来的值覆盖回旧快照，
    /// 那条路请走 [`Self::store`]。
    pub fn sync_prefs(&mut self, prefs: &mut nbs_io::prefs::Preferences) -> bool {
        let mut changed = false;
        for effect in &mut self.effects {
            let id = effect.info().id;
            if let Some(value) = prefs.fx.get(id) {
                effect.set_params(value);
            }
            let snapshot = effect.params();
            if prefs.fx.get(id) != Some(&snapshot) {
                prefs.fx.insert(id.to_owned(), snapshot);
                changed = true;
            }
        }
        let on: Vec<String> = prefs.fx_active.clone();
        self.opacity = prefs.fx_opacity;
        if on != self.active {
            if !on.is_empty() {
                // 首选项里记的启用状态只在「认得这些 id」时采用，舞台互斥规则照走一遍。
                let known: Vec<String> = on
                    .into_iter()
                    .filter(|id| self.info(id).is_some())
                    .collect();
                for id in known {
                    self.set_active(&id, true);
                }
            }
            changed = true;
        }
        prefs.fx_active = self.active.clone();
        changed
    }

    /// 启用状态写回首选项（不经过参数结构）。
    pub fn write_active(&self, prefs: &mut nbs_io::prefs::Preferences) {
        prefs.fx_active = self.active.clone();
        prefs.fx_opacity = self.opacity;
    }

    /// 只写不读：各特效的参数快照、启用状态、透明度灌回首选项。
    /// 调参面板改完特效自己的字段要靠这条落地 —— `sync_prefs` 会先把旧快照读回来，
    /// 把刚滑出来的值覆盖掉。
    pub fn store(&self, prefs: &mut nbs_io::prefs::Preferences) {
        for effect in &self.effects {
            let id = effect.info().id;
            let snapshot = effect.params();
            if prefs.fx.get(id) != Some(&snapshot) {
                prefs.fx.insert(id.to_owned(), snapshot);
            }
        }
        self.write_active(prefs);
    }

    /// 某个特效的调参面板。返回是否改动。
    pub fn settings_ui(&mut self, ui: &mut Ui, id: &str) -> bool {
        if let Some(effect) = self.effects.iter_mut().find(|e| e.info().id == id) {
            effect.settings_ui(ui)
        } else {
            false
        }
    }

    /// 各特效的降级说明（素材缺失之类）。
    pub fn notes(&self) -> Vec<String> {
        self.effects
            .iter()
            .filter(|e| self.is_active(e.info().id))
            .filter_map(|e| e.note())
            .collect()
    }

    /// 合成 `surface` 这一层的一帧图元。
    pub fn compose(&mut self, cx: &FxContext, surface: Surface) {
        self.frame.clear();
        let ids = self.active.clone();
        for id in &ids {
            let Some(index) = self.effects.iter().position(|e| e.info().id == *id) else {
                continue;
            };
            if self.effects[index].info().surface != surface {
                continue;
            }
            let frame = &mut self.frame;
            self.effects[index].draw(cx, frame);
        }
        self.overflowed = self.frame.truncate();
    }

    /// 舞台表面：在中央面板那块区域里画，并先铺一层底色（舞台是独占的）。
    pub fn paint_stage(&mut self, cx: &FxContext, p: &Painter) {
        self.compose(cx, Surface::Stage);
        p.rect_filled(cx.area, 0.0, STAGE_BACKDROP);
        stage::paint(p, &self.frame, 1.0);
    }

    /// 叠加表面：追加到背景层，压住面板、仍在弹窗之下。
    pub fn paint_overlay(&mut self, cx: &FxContext, ctx: &Context) {
        self.compose(cx, Surface::Overlay);
        if self.frame.is_empty() {
            return;
        }
        let p = ctx.layer_painter(LayerId::new(Order::Background, Id::new("nbs_fx_overlay")));
        stage::paint(&p, &self.frame, self.opacity);
    }

    pub fn frame(&self) -> &Frame {
        &self.frame
    }
}

/// 舞台底色：不用纯黑，免得强调色在上面刺眼。
pub const STAGE_BACKDROP: Color32 = Color32::from_rgb(12, 12, 16);

/// 底色算不算「浅」：舞台特效要在浅底上把参考线从白 alpha 换成黑 alpha，
/// 否则整条线看不见。按 0.299/0.587/0.114 加权（`Color32` 存的是预乘色，
/// 底色都是不透明的，这里直接拿通道比就行），和主题是不是 dark 无关 —— 用户可以把
/// 浅色主题的舞台切成黑底，那时该用白线。
pub fn is_light(color: Color32) -> bool {
    (color.r() as u32 * 299 + color.g() as u32 * 587 + color.b() as u32 * 114) >= 128 * 1000 / 2
}

/// 只压 alpha：`from_rgba_premultiplied` 就是「按这 4 个数存」，
/// RGB 不动才不会淡出的同时把颜色也压暗。
pub fn fade(color: Color32, k: f32) -> Color32 {
    Color32::from_rgba_premultiplied(
        color.r(),
        color.g(),
        color.b(),
        (color.a() as f32 * k.clamp(0.0, 1.0)) as u8,
    )
}

/// 一枚音符在画面上该用什么表示。方块排和瀑布的「乐器方块」样式共用这一条，
/// 所以同一件乐器在两条特效里必须同图同色。
pub struct NoteBlock {
    /// 有贴图就走贴图；`None` 且 `missing` 为真 = 该有贴图但素材没扒到，下面用兜底色。
    pub texture: Option<Arc<TextureHandle>>,
    /// 没贴图时用的色：配过方块但缺素材 → 按方块名兜底色；压根没配方块（自定义
    /// 乐器出厂全是同一块石头）→ 按乐器名哈希色，否则一整排同色就看不出「响的是哪件」。
    pub color: Color32,
    /// 该有贴图却没读到（用来提一句「记得扒贴图」）。
    pub missing: bool,
    /// 导出表里没给这件乐器配方块（用来指一句去哪儿配）。
    pub unconfigured: bool,
}

/// 按导出方块表 + 贴图目录查一枚音符的方块长相。`use_tiles` 关掉时只给色。
pub fn note_block(cx: &FxContext, instrument: u8, use_tiles: bool) -> NoteBlock {
    let has_block = cx.blocks.has_block(instrument);
    let texture = if use_tiles && has_block {
        cx.tiles
            .and_then(|tiles| tiles.texture_for(cx.tex, cx.blocks, instrument))
    } else {
        None
    };
    let missing = texture.is_none() && use_tiles && has_block;
    let color = if has_block {
        fallback_color(cx.blocks.name(instrument))
    } else {
        instrument_color(cx.timeline.instrument_name(instrument), instrument)
    };
    NoteBlock {
        texture,
        color,
        missing,
        unconfigured: !has_block,
    }
}

/// 状态里攒着的那份时间轴：曲子一改就重烘。
pub struct FxState {
    pub timeline: FxTimeline,
    /// `song.rev` + 「是否播放锁定层」，烘焙的缓存键。
    /// 换歌（开档/新建/导入）也走 rev：`Document::replace_song` 会递增它，
    /// 否则新曲子和上一首都是 0，这份缓存会一直留着上一首的音符表。
    pub baked_for: (u64, bool),
    pub blocks: InstrumentBlocks,
    /// 烘焙用的乐器方块表快照（导出选项一改就重建 `blocks`）。
    pub blocks_key: Vec<u8>,
    pub stack: FxStack,
    pub tiles: Option<BlockTiles>,
    /// 主界面那行方块精灵的平均色（瀑布胶囊的底色，和编辑区同源）。
    pub palette: NotePalette,
    /// 素材目录只认一次：没扒素材时 `palette` 一直是空的，不然后每帧重扫一遍目录。
    palette_done: bool,
}

impl Default for FxState {
    fn default() -> Self {
        Self {
            timeline: FxTimeline::new(),
            baked_for: (u64::MAX, false),
            blocks: InstrumentBlocks::default(),
            blocks_key: Vec::new(),
            stack: FxStack::new(),
            tiles: None,
            palette: NotePalette::default(),
            palette_done: false,
        }
    }
}

impl FxState {
    /// 有启用中的舞台特效时，中央面板让给它。
    pub fn stage_active(&self) -> bool {
        self.stack.stage_id().is_some()
    }

    /// 有没有需要底部钢琴栏跟着显示的特效。舞台特效也算：瀑布的轨道直接按
    /// 面板那条键盘的摆放算，画面里不会出现两条键盘。
    pub fn wants_piano(&self) -> bool {
        self.stack.wants_piano()
    }

    /// 琴键面板在舞台接管时该填的底色（取启用中舞台特效的口径，见
    /// [`FxStack::stage_backdrop`]）。
    pub fn stage_backdrop(&self, grid_bg: Color32) -> Color32 {
        self.stack.stage_backdrop(grid_bg)
    }

    /// 有没有画在叠加层上的特效。`app.rs` 靠它决定要不要为叠加那一路凑一份输入
    /// （凑一份要读时钟、算键盘摆放，一条都没开时不该花钱）。
    pub fn overlay_active(&self) -> bool {
        self.stack.surface_active(Surface::Overlay)
    }

    /// 换歌曲标签时作废烘焙缓存（刀 0）：下一次 [`Self::refresh`] 按新曲子重烘。
    /// 哨兵值与「一条特效都没开」那一帧打回去的相同，不会画半帧上一首的音符。
    pub fn invalidate_bake(&mut self) {
        self.baked_for = (u64::MAX, false);
    }

    /// 每帧开头调一次：该重烘的烘掉，素材只认一次目录。
    ///
    /// 一条特效都没开时整段跳过 —— 那时画的是主界面网格，把整首曲子的事件展开、
    /// 排序、累加秒表纯属白干。这是「渲染瀑布时主界面不画」的另一半：
    /// 反过来渲染主界面时特效这边也不许烘表。跳过那一帧把缓存键打回哨兵值，
    /// 下一条特效开启的那帧就地重烘，不会画半帧上一首曲子。
    pub fn refresh(&mut self, song: &Song, include_locked: bool, opts: &nbs_io::schematic::SchematicOptions) {
        if self.stack.active().is_empty() {
            self.baked_for = (u64::MAX, false);
            return;
        }
        if self.baked_for != (song.rev, include_locked) {
            self.timeline = FxTimeline::bake(song, include_locked);
            self.baked_for = (song.rev, include_locked);
        }
        if self.blocks_key != opts.ins_block {
            self.blocks = InstrumentBlocks::from_options(opts);
            self.blocks_key = opts.ins_block.clone();
        }
        if self.tiles.is_none() {
            self.tiles = Some(BlockTiles::open());
        }
        // 精灵平均色只读一次目录：没扒素材时表会一直是空的，不能每帧重扫。
        if !self.palette_done {
            self.palette = NotePalette::open();
            self.palette_done = true;
        }
    }

    /// 舞台那一路：`area` 是中央面板那块，`p` 是从那块面板拿到的画笔。
    pub fn paint_stage(&mut self, ctx: &Context, input: &FxInput, p: &Painter) {
        let cx = fx_context(
            ctx,
            (
                &self.timeline,
                &self.blocks,
                self.tiles.as_ref(),
                &self.palette,
            ),
            input,
        );
        self.stack.paint_stage(&cx, p);
    }

    /// 叠加那一路：所有面板画完、弹窗之前调。没有启用中的叠加特效时等于空判断。
    pub fn paint_overlay(&mut self, ctx: &Context, input: &FxInput) {
        if !self.stack.surface_active(Surface::Overlay) {
            return;
        }
        let cx = fx_context(
            ctx,
            (
                &self.timeline,
                &self.blocks,
                self.tiles.as_ref(),
                &self.palette,
            ),
            input,
        );
        self.stack.paint_overlay(&cx, ctx);
    }
}

/// 凑一帧特效的视野。做成自由函数而不是 `FxState` 的方法：方法会把整个 `self`
/// 借走，调用方就没法在同一句里可变借 `self.stack` 落笔。
fn fx_context<'a>(
    tex: &'a Context,
    data: (
        &'a FxTimeline,
        &'a InstrumentBlocks,
        Option<&'a BlockTiles>,
        &'a NotePalette,
    ),
    input: &'a FxInput,
) -> FxContext<'a> {
    let (timeline, blocks, tiles, palette) = data;
    FxContext {
        tex,
        now: input.now,
        dt: input.dt,
        position: input.position,
        playing: input.playing,
        speed: input.speed,
        scale: input.scale,
        accent: input.accent,
        backdrop: input.backdrop,
        area: input.area,
        keyboard: input.keyboard,
        timeline,
        blocks,
        tiles,
        palette,
    }
}

/// 底部钢琴栏那块矩形 → 舞台/叠加共用的键盘摆放。
/// 钢琴栏关掉时返回 `None`，特效自己 `KeyboardLayout::fit`。
pub fn panel_keyboard(rect: Option<Rect>, scale: f32, first: usize, slots: usize) -> Option<KeyboardLayout> {
    let rect = rect?;
    Some(KeyboardLayout {
        origin: egui::pos2(
            rect.min.x + ((rect.width() - slots as f32 * keyboard::KEY_W * scale) / 2.0).floor(),
            rect.min.y + BLACK_LIFT * scale,
        ),
        scale,
        first,
        slots,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dummy {
        info: EffectInfo,
    }

    impl PlaybackEffect for Dummy {
        fn info(&self) -> EffectInfo {
            self.info
        }
        fn draw(&mut self, _cx: &FxContext, out: &mut Frame) {
            out.rect(
                Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(4.0, 4.0)),
                Color32::RED,
                0.0,
            );
        }
    }

    fn stack_with(infos: &[EffectInfo]) -> FxStack {
        let mut stack = FxStack::new();
        stack.effects = infos
            .iter()
            .map(|i| {
                Box::new(Dummy { info: *i }) as Box<dyn PlaybackEffect>
            })
            .collect();
        stack
    }

    const OVERLAY: EffectInfo = EffectInfo {
        id: "overlay_a",
        name: "叠加A",
        surface: Surface::Overlay,
        z: 1,
        wants_piano: false,
    };
    const OVERLAY_LATE: EffectInfo = EffectInfo {
        id: "overlay_b",
        name: "叠加B",
        surface: Surface::Overlay,
        z: 5,
        wants_piano: true,
    };
    const STAGE_ONE: EffectInfo = EffectInfo {
        id: "stage_one",
        name: "舞台1",
        surface: Surface::Stage,
        z: 0,
        wants_piano: false,
    };
    const STAGE_TWO: EffectInfo = EffectInfo {
        id: "stage_two",
        name: "舞台2",
        surface: Surface::Stage,
        z: 1,
        wants_piano: false,
    };

    #[test]
    fn built_ins_are_registered() {
        let stack = FxStack::new();
        let ids = stack.ids();
        assert!(ids.contains(&"synthesia"), "{ids:?}");
        assert!(ids.contains(&"block_row"), "{ids:?}");
        // 出厂默认都不开：用户没要特效时，播放路径不该多出任何绘制。
        assert!(stack.active().is_empty());
        assert_eq!(stack.stage_id(), None);
    }

    #[test]
    fn stage_switches_instead_of_stacking() {
        let mut stack = stack_with(&[STAGE_ONE, STAGE_TWO, OVERLAY]);
        assert!(stack.set_active("stage_one", true).unwrap().contains("已开启"));
        let msg = stack.set_active("stage_two", true).unwrap();
        assert!(msg.contains("换下") && msg.contains("舞台1"), "{msg}");
        assert_eq!(stack.stage_id().as_deref(), Some("stage_two"));
        assert!(!stack.is_active("stage_one"), "旧舞台要被换掉");
        // 关掉当前舞台后，别把之前那个自动请回来。
        stack.set_active("stage_two", false);
        assert_eq!(stack.stage_id(), None);
    }

    #[test]
    fn overlays_coexist_and_sort_by_z() {
        let mut stack = stack_with(&[OVERLAY_LATE, OVERLAY]);
        stack.set_active("overlay_b", true);
        stack.set_active("overlay_a", true);
        assert_eq!(stack.active(), &["overlay_a".to_owned(), "overlay_b".to_owned()]);
        assert_eq!(
            stack.ids(),
            vec!["overlay_a", "overlay_b"],
            "菜单顺序也要按 z，和绘制顺序一致"
        );
        assert!(stack.wants_piano(), "叠加B 要求钢琴栏");
        // 叠加特效不进舞台。
        assert_eq!(stack.stage_id(), None);
    }

    #[test]
    fn compose_only_runs_the_matching_surface() {
        let mut stack = stack_with(&[OVERLAY, STAGE_ONE]);
        stack.set_active("overlay_a", true);
        stack.set_active("stage_one", true);
        let ctx = Context::default();
        let timeline = FxTimeline::new();
        let blocks = InstrumentBlocks::default();
        let palette = NotePalette::default();
        let cx = FxContext {
            tex: &ctx,
            now: 0.0,
            dt: 0.016,
            position: 0.0,
            playing: false,
            speed: 1.0,
            scale: 1.0,
            accent: Color32::LIGHT_GRAY,
            backdrop: STAGE_BACKDROP,
            area: Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0)),
            keyboard: None,
            timeline: &timeline,
            blocks: &blocks,
            tiles: None,
            palette: &palette,
        };
        stack.compose(&cx, Surface::Overlay);
        assert_eq!(stack.frame().len(), 1, "叠加层只跑叠加特效");
        stack.compose(&cx, Surface::Stage);
        assert_eq!(stack.frame().len(), 1);
        assert!(!stack.took_too_many());
    }

    #[test]
    fn unknown_id_is_reported_not_silently_ignored() {
        let mut stack = stack_with(&[OVERLAY]);
        let msg = stack.set_active("nope", true).unwrap();
        assert!(msg.contains("不认识"), "{msg}");
        assert!(stack.active().is_empty());
    }

    #[test]
    fn prefs_round_trip() {
        let mut prefs = nbs_io::prefs::Preferences::default();
        let mut stack = FxStack::new();
        assert!(stack.sync_prefs(&mut prefs), "首次同步要把出厂参数写进去");
        assert!(prefs.fx.contains_key("synthesia"));
        stack.set_active("synthesia", true);
        stack.write_active(&mut prefs);
        assert_eq!(prefs.fx_active, vec!["synthesia".to_owned()]);
        // 另一个 stack 从同一份首选项恢复，启用状态要回来。
        let mut restored = FxStack::new();
        restored.sync_prefs(&mut prefs);
        assert!(restored.is_active("synthesia"), "启用状态没落地");
        // 再同步一次不该继续标脏，否则首选项会被白白反复重写。
        assert!(!restored.sync_prefs(&mut prefs));
    }

    #[test]
    fn panel_keyboard_is_none_without_the_panel() {
        assert!(panel_keyboard(None, 1.0, 0, 52).is_none());
        let rect = Rect::from_min_size(egui::pos2(0.0, 500.0), egui::vec2(52.0 * 39.0, 128.0));
        let layout = panel_keyboard(Some(rect), 1.0, 0, 52).unwrap();
        assert_eq!(layout.slots, 52);
        assert_eq!(layout.origin.y, 500.0 + BLACK_LIFT);
        assert_eq!(layout.slot_white_key(0), Some(0));
    }

    /// 舞台接管时琴键面板的底色取自启用中的舞台特效：默认跟编辑区同色，
    /// 勾了「黑色背景」就一起黑。没有舞台在画（或被叠加特效换下）时回落编辑区底色。
    #[test]
    fn stage_backdrop_comes_from_the_active_stage_effect() {
        let grid_bg = Color32::from_rgb(249, 249, 249);
        let mut stack = FxStack::new();
        assert_eq!(stack.stage_backdrop(grid_bg), grid_bg, "没有舞台在画");
        stack.set_active("synthesia", true);
        assert_eq!(stack.stage_backdrop(grid_bg), grid_bg, "默认跟编辑区同色");
        stack.effects[0].set_params(&serde_json::json!({ "dark_background": true }));
        assert_eq!(stack.stage_backdrop(grid_bg), STAGE_BACKDROP, "面板要跟舞台一起黑");
        stack.set_active("synthesia", false);
        assert_eq!(
            stack.stage_backdrop(grid_bg),
            grid_bg,
            "舞台关掉后回落到编辑区底色"
        );
    }

    /// `store` 只写不读：调参面板改的是特效自己的字段，落盘必须拿新值。
    /// `sync_prefs` 是「先读后写」，同一条改动会被旧快照盖掉 —— 所以开机用它、调参用 `store`。
    #[test]
    fn store_writes_the_live_params_not_the_old_snapshot() {
        let mut prefs = nbs_io::prefs::Preferences::default();
        let mut stack = FxStack::new();
        stack.sync_prefs(&mut prefs);
        let shipped = prefs.fx["synthesia"].clone();
        stack.effects[0].set_params(&serde_json::json!({ "fall_seconds": 4.25 }));
        stack.store(&mut prefs);
        assert_eq!(prefs.fx["synthesia"]["fall_seconds"].as_f64(), Some(4.25));
        assert_ne!(prefs.fx["synthesia"], shipped, "出厂那份快照该被换掉");
        stack.effects[0].set_params(&serde_json::json!({ "fall_seconds": 1.5 }));
        stack.sync_prefs(&mut prefs);
        assert_eq!(
            stack.effects[0].params()["fall_seconds"].as_f64(),
            Some(4.25),
            "这条路会把刚滑出来的值读回旧快照"
        );
        // 启用状态与透明度同样走 `store`。
        stack.set_active("synthesia", true);
        stack.set_opacity(0.4);
        stack.store(&mut prefs);
        assert_eq!(prefs.fx_active, vec!["synthesia".to_owned()]);
        assert_eq!(prefs.fx_opacity, 0.4);
    }

    /// 接线用的接缝：`FxInput` 里那几个数得原样进 [`FxContext`]，底座只补自己的三份数据。
    #[test]
    fn fx_input_becomes_the_effect_context() {
        let ctx = Context::default();
        let fx = FxState::default();
        let layout = panel_keyboard(
            Some(Rect::from_min_size(
                egui::pos2(0.0, 500.0),
                egui::vec2(26.0 * keyboard::KEY_W, 128.0),
            )),
            1.0,
            14,
            26,
        )
        .unwrap();
        let input = FxInput {
            now: 12.5,
            dt: 0.05,
            position: 30.0,
            playing: true,
            speed: 1.5,
            scale: 1.25,
            accent: Color32::from_rgb(1, 2, 3),
            backdrop: STAGE_BACKDROP,
            area: Rect::from_min_size(egui::pos2(4.0, 6.0), egui::vec2(640.0, 480.0)),
            keyboard: Some(layout),
        };
        let cx = fx_context(
            &ctx,
            (&fx.timeline, &fx.blocks, fx.tiles.as_ref(), &fx.palette),
            &input,
        );
        assert_eq!(cx.now, input.now);
        assert_eq!(cx.dt, input.dt);
        assert_eq!(cx.position, input.position);
        assert!(cx.playing);
        assert_eq!(cx.speed, input.speed);
        assert_eq!(cx.scale, input.scale);
        assert_eq!(cx.accent, input.accent);
        assert_eq!(cx.area, input.area);
        assert_eq!(cx.keyboard, Some(layout), "琴键面板的对齐要传下去");
        assert_eq!(cx.timeline.note_count(), 0);
        assert!(cx.tiles.is_none(), "没 refresh 就没有贴图");
    }

    /// 两条绘制路径真跑一遍：出厂（一条没开）不产出任何图元，开了舞台/叠加才有。
    #[test]
    fn both_paint_paths_run() {
        let ctx = Context::default();
        let mut fx = FxState::default();
        let area = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let input = FxInput {
            now: 1.0,
            dt: 0.05,
            position: 3.0,
            playing: true,
            speed: 1.0,
            scale: 1.0,
            accent: Color32::LIGHT_GRAY,
            backdrop: STAGE_BACKDROP,
            area,
            keyboard: None,
        };
        let p = Painter::new(ctx.clone(), LayerId::background(), area);
        fx.paint_stage(&ctx, &input, &p);
        assert!(fx.stack.frame().is_empty(), "没人开特效时舞台路径不该画东西");
        fx.paint_overlay(&ctx, &input);
        assert!(fx.stack.frame().is_empty(), "同上，叠加路径也不该画东西");

        // 舞台：空曲子也该画出轨道和击中线，并让中央面板认它。
        fx.stack.set_active(synthesia::Synthesia::ID, true);
        assert!(fx.stage_active());
        fx.paint_stage(&ctx, &input, &p);
        assert!(
            (20..40).contains(&fx.stack.frame().len()),
            "空曲子的静态帧该只剩轨道线 + 击中线（800px 放 20 槽 = 22 个），实际 {}",
            fx.stack.frame().len()
        );
        assert!(
            fx.wants_piano(),
            "瀑布的键盘就是底部那条面板，得把它顶出来"
        );

        // 叠加：方块排要琴键栏；没曲子没贴图时是空帧，但不能炸。
        fx.stack.set_active(blockrow::BlockRow::ID, true);
        assert!(fx.wants_piano(), "方块排按琴键面板对齐");
        fx.paint_overlay(&ctx, &input);
        assert!(fx.stack.frame().is_empty(), "空曲子的方块排没有方块");
    }

    fn song_with(notes: &[(u32, u32, u8, u8)]) -> nbs_core::model::Song {
        let mut s = nbs_core::model::Song::new();
        for (tick, layer, instrument, key) in notes {
            while s.layers.len() <= *layer as usize {
                s.layers.push(nbs_core::model::Layer::default());
            }
            s.set(*tick, *layer, nbs_core::model::Note::new(*instrument, *key));
        }
        s.recalculate();
        s
    }

    /// 换歌必须重烘。解析出来的曲子 `rev` 恒为 0，和启动时那份演示曲撞在同一个缓存键上，
    /// 不递增就永远不重烘 —— 特效于是一直在画上一首的音符，看着就是
    /// 「特效不随出现的乐器变」（用户报的原话）。
    #[test]
    fn swapping_the_song_rebakes_the_timeline() {
        let mut doc = nbs_core::Document::new();
        let mut fx = FxState::default();
        // 关着特效时 `refresh` 直接不烘（省开销那条），这里要验的是换歌重烘，
        // 所以先把瀑布打开 —— 和真机上「开着特效换歌」是同一件事。
        fx.stack.set_active(synthesia::Synthesia::ID, true);
        let opts = nbs_io::schematic::SchematicOptions::default();
        doc.replace_song(song_with(&[(0, 0, 0, 60), (1, 0, 0, 62), (2, 0, 7, 64)]));
        fx.refresh(&doc.song, false, &opts);
        assert_eq!(fx.timeline.note_count(), 3);
        doc.replace_song(song_with(&[(0, 0, 7, 48)]));
        fx.refresh(&doc.song, false, &opts);
        assert_eq!(fx.timeline.note_count(), 1, "换歌之后特效还在用上一首的音符表");
    }

    /// 两边都不渲染才叫省开销：特效全关时 `refresh` 一帧都不该烘，画面上也不该
    /// 有人去算图元；反过来开着瀑布时叠加表面（方块排）不干活。
    #[test]
    fn nothing_is_baked_or_painted_while_every_effect_is_off() {
        let mut fx = FxState::default();
        let opts = nbs_io::schematic::SchematicOptions::default();
        fx.refresh(&song_with(&[(0, 0, 0, 60), (1, 0, 0, 62), (2, 0, 7, 64)]), false, &opts);
        assert_eq!(fx.timeline.note_count(), 0, "没开特效不该烘音符表");
        assert!(!fx.stage_active() && !fx.overlay_active());

        fx.stack.set_active(synthesia::Synthesia::ID, true);
        fx.refresh(&song_with(&[(0, 0, 0, 60), (1, 0, 0, 62), (2, 0, 7, 64)]), false, &opts);
        assert_eq!(fx.timeline.note_count(), 3, "打开特效的那一次就要补烘上");
        assert!(fx.stage_active(), "瀑布占舞台");
        assert!(!fx.overlay_active(), "瀑布不占叠加表面");
    }
}

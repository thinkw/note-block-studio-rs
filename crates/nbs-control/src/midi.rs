//! MIDI 导入的能力面：字段和 `nbs_io::midi::MidiImportOptions` /
//! `nbs_io::midi_enhanced::EnhancedMidiOptions` 逐一对齐。
//!
//! 为什么每个字段都是 `Option`：导入窗那套选项**本来就有一份"人不开窗口时的那份"**
//! （标量取自偏好里上次留下的勾，三张映射表按这个文件 + 当前 GM 表现算）。外部调用方
//! 省略某个字段时必须拿到那一份基线，而不是 `MidiImportOptions::default()`——出厂
//! `default()` 的通道映射全被写成乐器 0、鼓映射是空的，照它导出的 MIDI 会整首变钢琴且
//! 打击乐全哑，那既不是人的默认也不是任何人想要的结果。
//!
//! 这里的类型不带 nbs-io 的字段类型，是因为 `nbs-control` 刻意不依赖 `nbs-core`/`nbs-io`：
//! 协议要能单独测，转换由编辑器那一侧逐字段搬运。
//!
//! 下面这些**入参**结构体都带 `deny_unknown_fields`，和仓库其他地方「认不出的字段当没给」
//! 相反。理由是这里的失败形状：一个拼错的字段名等于「这一项没生效、其余照旧」，
//! 回执看起来完全正常——导入窗里少了两个勾这种事没人能从结果反推出来。当场拒掉，
//! 错误还便宜一点。回显用的字段是同一批结构体，序列化不受影响。

use serde::{Deserialize, Serialize};

/// 一条通道的音色/八度覆盖，对应界面上「乐器」那一页的两列。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ImportChannelMap {
    #[cfg_attr(
        feature = "schema",
        schemars(description = "MIDI 通道号 0..=15（界面显示 1..=16，这里从 0 起算）；9 = 打击乐通道，不走这条")
    )]
    pub channel: u8,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "落到的音色号：内置 0..=19，自定义从 20 起（见 get_instruments）；-1 = 忽略整条通道，该通道不出音符")
    )]
    pub instrument: i8,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "整条通道的八度位移，单位是八度（不是半音）；0 = 不移")
    )]
    #[serde(default)]
    pub octave: i8,
}

/// 一个鼓音号的映射覆盖，对应界面上「打击乐」那一页。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ImportDrumMap {
    #[cfg_attr(
        feature = "schema",
        schemars(description = "MIDI 音号 0..=127（这个文件的鼓音清单看 inspect_midi 的 drums）")
    )]
    pub note: u8,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "用哪个音色打这个音；-1 = 静音，导入时这个鼓音直接不出现在歌里")
    )]
    pub instrument: i8,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "最终落到 NBS 的键号 0..=87（GM 表出厂那条里已经含 +33，这里给的是加过之后的）")
    )]
    pub key: i8,
}

/// 传统导入的一整套可调项。省略 = 用编辑器的基线；给了哪个就覆盖哪个。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ImportMidiOptions {
    #[cfg_attr(
        feature = "schema",
        schemars(description = "去除开始时空部分：省略 = 用偏好里上次留下的勾")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove_silent: Option<bool>,
    #[cfg_attr(feature = "schema", schemars(description = "给每层命名"))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_layers: Option<bool>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "层名按乐器还是按通道号；只在 name_layers 为真时有意义")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_after_patch: Option<bool>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "速度取文件里的；false = 保持歌曲出厂速度")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tempo_from_file: Option<bool>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "把文件里的速度变化写成 Tempo Changer 音符（会多插一层 TempoChgr 并注册自定义音色）；游戏内和多数 NBS 工具不认")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tempo_changer: Option<bool>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "每条通道最多占几层，1..=20，20 = 不限；超出上限的音符被丢掉而不是挤在一起")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_height: Option<u8>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "保持八度范围：把音符按八度折进 Minecraft 原生音域 33..=57")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_octave_range: Option<bool>,
    #[cfg_attr(feature = "schema", schemars(description = "导入音符音量：false = 全部按 100"))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_velocity: Option<bool>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "精准度，只收 0 / 1 / 3 / 7，对应界面上的 1 倍 / 2 倍 / 4 倍 / 8 倍间距；倍数越大 tick 越密，同一份文件 8 倍比 1 倍长 8 倍")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precision: Option<u8>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "音符长度：把长音摊成一串尾音。开了才有 tail_parts 的意义")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_duration: Option<bool>,
    #[cfg_attr(feature = "schema", schemars(description = "尾音渐变：尾音音量按 start% → end% 线性变化"))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_duration_fade: Option<bool>,
    #[cfg_attr(feature = "schema", schemars(description = "第一个尾音相对原音符音量的百分比 0..=100"))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_duration_fade_start: Option<u8>,
    #[cfg_attr(feature = "schema", schemars(description = "最后一个尾音的百分比 0..=100"))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_duration_fade_end: Option<u8>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "通道映射覆盖；空 = 按当前 GM 音色表为这个文件现算（也就是人打开导入窗时看到的那一页）")
    )]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<ImportChannelMap>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "鼓音映射覆盖；空 = 按当前 GM 鼓表现算。音号在这个文件里不存在会被拒绝，不是被忽略")
    )]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drums: Vec<ImportDrumMap>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "哪些声部参与尾音，给的是 inspect_midi 里 parts 的下标；省略 = 文件解析出的初始集合（打击乐声部不参与），给了就以它为准——空数组 = 全部关掉尾音")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail_parts: Option<Vec<u32>>,
}

/// 增强导入的音域处理方式，对应界面那四个单选。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum ImportRangeMode {
    /// 只做手动移调，不替换也不折叠。
    Keep,
    /// 手动移调之后按八度把音符折回来。
    Fold,
    #[default]
    /// 先按音色替换，再推荐整八度位移。
    Octave,
    /// 先按音色替换，再推荐半音位移。
    Semitone,
}

/// 一条通道的三个槽位（旋律 + 两个上和声），对应增强导入页的三列音色。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ImportSlot {
    #[cfg_attr(
        feature = "schema",
        schemars(description = "这个槽用的音色 0..=19（内置）；null = 这个槽静音")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instrument: Option<u8>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "这个槽独立的音量百分比 0..=100，省略 = 100；这是 NBS 层音量，不是采样力度")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<u8>,
}

/// 一条通道的增强设置：三个槽位 + 手动移调。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ImportEnhancedChannel {
    #[cfg_attr(feature = "schema", schemars(description = "MIDI 通道号 0..=15"))]
    pub channel: u8,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "手动移调，单位半音，-72..=72；省略 = 保留这份文件算出来的推荐位移。给了就以它为准——槽位或 range_mode 触发的重算不会盖掉它")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transpose: Option<i8>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "最多 3 个槽，给了几个用几个，剩下的按静音补；空 = 保留这份文件的 GM 组合预设")
    )]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<ImportSlot>,
}

/// 一个音色的高/低替换目标，对应界面「越界替换」那张 20 行的表。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ImportSubstitution {
    #[cfg_attr(feature = "schema", schemars(description = "被替换的音色号 0..=19"))]
    pub instrument: u8,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "高音越界时换到的音色；null = 不换")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high: Option<u8>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "低音越界时换到的音色；null = 不换")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub low: Option<u8>,
}

/// 延音档位，对应界面那三个单选。
///
/// 这个字段**两种写法都收**：`"off"` / `"all"` / `"tracks"` 是新契约，`true` / `false`
/// 是已经发布出去的老契约（`true` = `all`、`false` = `off`）。回显只出字符串——
/// 一份回声同时认两种写法，等于让调用方每次先判类型。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportSustain {
    /// 不动尾音，沿用传统导入的「每 tick 一块」。
    #[default]
    Off,
    /// 所有轨道的长音都按包络重敲。
    All,
    /// 只重敲 `sustain_tracks` 列出的轨道。
    Tracks,
}

// 手写而不是 `#[serde(untagged)]`：untagged 失败时只吐一句「哪个变体都不匹配」，
// 把允许值全藏了。理由和 `nbs-mcp` 的 `Pitch` 一样——这条面立的是「拼错要大声」。
impl<'de> Deserialize<'de> for ImportSustain {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;
        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            serde_json::Value::Bool(on) => Ok(if on { Self::All } else { Self::Off }),
            serde_json::Value::String(text) => match text.as_str() {
                "off" => Ok(Self::Off),
                "all" => Ok(Self::All),
                "tracks" => Ok(Self::Tracks),
                other => Err(D::Error::custom(format!(
                    "sustain 只认 \"off\" / \"all\" / \"tracks\"（老写法 true / false 等于 all / off），收到 {other:?}"
                ))),
            },
            other => Err(D::Error::custom(format!(
                "sustain 要么是 \"off\" / \"all\" / \"tracks\"，要么是 true / false，实际是 {other}"
            ))),
        }
    }
}

/// 手写而不是 derive：两种写法各自要带一句说明，derive 给不出这个 `anyOf` 形状。
#[cfg(feature = "schema")]
impl schemars::JsonSchema for ImportSustain {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ImportSustain".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "延音：长音按该乐器实测余音重敲成多颗，而不是每 tick 一个块",
            "anyOf": [
                {
                    "type": "string",
                    "enum": ["off", "all", "tracks"],
                    "description": "off = 关；all = 所有轨道都重敲；tracks = 只重敲 sustain_tracks 列出的轨道"
                },
                {
                    "type": "boolean",
                    "description": "老写法，仍然支持：true = all、false = off。tracks 只能走字符串那种"
                }
            ]
        })
    }
}

/// 重敲排多密，对应界面那个单选。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum ImportSustainDensity {
    /// 间隔 = 该乐器采样实测余音换算出的 tick 数。
    #[default]
    Measured,
    /// 每 tick 一块（和 Miau 的步长对齐）。
    EveryTick,
}

/// 增强导入那一整套。省略 = 传统路径（`enabled = false`）加上按这个文件算出的预设。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ImportEnhancedOptions {
    #[cfg_attr(
        feature = "schema",
        schemars(description = "是否走增强导入；省略 = false，即界面里的「传统导入」档位")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "音域处理方式：keep / fold / octave / semitone，省略 = octave")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_mode: Option<ImportRangeMode>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "智能替换：允许越界时换成别的音色。关掉就只剩移调/折叠")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smart_substitute: Option<bool>,
    #[cfg_attr(feature = "schema", schemars(description = "强制按八度折叠"))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_fold: Option<bool>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "延音：把长音按该乐器实测余音重敲成多颗，而不是每 tick 一个块。off / all / tracks，老写法 true / false 仍收")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sustain: Option<ImportSustain>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "sustain = tracks 时的白名单，存 MIDI 轨下标（和回执 tracks[].index 同一套编号）。空列表配 tracks 会被拒，因为那等于一颗都不重敲")
    )]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sustain_tracks: Vec<u32>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "延音衰减：把重敲沿包络渐弱；false = 保持同一电平。只在 sustain 为 all / tracks 时有意义")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sustain_fade: Option<bool>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "重敲排多密：measured = 按实测余音换算间隔（默认，也是老行为）；every_tick = 每 tick 一块（同 Miau）。只在 sustain 为 all / tracks 时有意义")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sustain_density: Option<ImportSustainDensity>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "拆声部：把一条 MIDI 通道拆成两层（右手/左手或高/低声部）")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub separate_voices: Option<bool>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "拆声部时忽略轨/通道元数据，退回按音高聚类")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_ignore_metadata: Option<bool>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "去重：同 tick 同音色同音高只留第一个")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dedupe_notes: Option<bool>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "逐通道覆盖；空 = 用这个文件的预设（GM 组合 + 推荐位移）")
    )]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<ImportEnhancedChannel>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "越界替换表覆盖，只动列出的音色；给出的一行整体替换出厂那一行，所以 high/low 省略就是那一侧不换。空 = 用出厂表")
    )]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub substitutes: Vec<ImportSubstitution>,
}

// --------------------------------------------------------------- 回执形状
//
// 下面这些只在回路上出现，所以不带 JSON Schema：工具面给入参形状，产物形状由
// `inspect_midi` / `import_midi` 的说明文字负责。

/// 一条轨：名字 + 事件数。界面「轨道」那一页给的就是这两样。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MidiTrackInfo {
    pub index: u32,
    pub name: String,
    pub events: usize,
}

/// 一条通道按**生效选项**会被怎么处理。`silent` 为真的通道不会有任何音符进歌。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MidiChannelPlan {
    pub channel: u8,
    /// 文件里最后一次的 program change；`>= 128` 表示这条通道没写过音色。
    pub patch: u16,
    /// GM 名字，越界时是空串。
    pub patch_name: String,
    /// 落到的音色，-1 = 忽略该通道。
    pub instrument: i8,
    pub octave: i8,
    /// 这条通道在文件里的音符数。
    pub notes: usize,
    pub silent: bool,
}

/// 一个鼓音按生效选项会被怎么处理。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MidiDrumPlan {
    pub note: u8,
    /// GM 鼓名，表里没有时是空串。
    pub name: String,
    pub instrument: i8,
    /// 最终键号 0..=87；`instrument = -1` 时这个鼓音整体静音，key 只做展示。
    pub key: i8,
    pub notes: usize,
}

/// 一个声部（`track` × `channel` × `patch`），尾音勾选的下标就是这里的 `index`。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MidiPartPlan {
    pub index: u32,
    pub track: u32,
    pub channel: u8,
    pub patch: u16,
    /// 这次生效的尾音开关。
    pub tail: bool,
    pub notes: usize,
}

/// 增强导入那六项计数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MidiEnhancedReport {
    pub notes: usize,
    pub out_of_range: usize,
    pub substituted: usize,
    pub folded: usize,
    pub voiced_notes: usize,
    pub deduped: usize,
}

/// 「这个文件按这套选项导进去会得到什么」的一份完整核对单。
///
/// 它同时是 `inspect_midi` 的产物和 `import_midi` 的回执：前者算完就把临时歌曲丢掉、
/// 工程不动，后者把同一份产物落进工程。两边共用一个形状，AI 才能拿导入前的预测和
/// 导入后的回执逐字段比——预测和实得不一致，那就是我们该修的 bug。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MidiPlan {
    pub path: String,
    pub format: u16,
    /// 头部分频字段原值；`smpte_division` 为真时它不是「每四分音符 tick 数」。
    pub division: u16,
    pub smpte_division: bool,
    /// 见过的最大通道号（不是通道个数），与原版 `midi_channels` 同义。
    pub channels_seen: u8,
    pub tracks: Vec<MidiTrackInfo>,
    /// 按时值算「长到值得重敲」的那些轨（界面延音那一页的候选清单，同一个函数）。
    /// 它只是**参考**：白名单可以列任意轨号，没进这份清单的轨照样能列，只是它的音大概率短到重敲不出第二颗。
    pub sustain_candidates: Vec<u32>,
    /// 旋律通道的处理，**不含通道 9**——打击乐整段在 `drums` 里，两边不重复数。
    pub channels: Vec<MidiChannelPlan>,
    /// 通道 9：这个文件里出现过的鼓音，按首次出现顺序。
    pub drums: Vec<MidiDrumPlan>,
    pub parts: Vec<MidiPartPlan>,
    /// 文件里的速度变化条数；只有 `tempo_changer` 开着才会写进歌。
    pub tempo_changes: usize,
    /// 生效选项用完后算出来的歌曲形状。
    pub notes: usize,
    pub layers: usize,
    pub layer_names: Vec<String>,
    pub length_ticks: u32,
    pub tempo: f32,
    pub bpm: f32,
    /// 落在 Minecraft 原生音域 33..=57 之外的音符数（这些键号在 MC 里要换八度才响）。
    pub out_of_range: usize,
    /// 按通道/鼓映射判成静音的**源音符**条数。不含 `max_height` 截断，也不看增强导入的
    /// 音色替换——那两个要看得准只能真导一遍，回执里的 `notes` 才是落地的数。
    pub silenced_notes: usize,
    pub enhanced: Option<MidiEnhancedReport>,
    /// 这次真正生效的传统选项（已把省略字段按基线补全）——回显，不是再次输入。
    pub applied: ImportMidiOptions,
    /// 这次真正生效的增强选项；`applied_enhanced.enabled` 为 false 时走的就是传统路径。
    pub applied_enhanced: ImportEnhancedOptions,
}


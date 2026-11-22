//! 音频转 MIDI 的能力面：`TranscribeOptions` 与 `nbs_io::transcription::TranscriptionOptions`
//! 逐字段对齐，取值域按本机 worker 的校验（`muscriptor_worker.py`）抄一份，
//! 好让「device 写了 gpu」这种错在起 python 子进程**之前**就说出来。
//!
//! 为什么这里全是 `Option`：转录窗上那套参数本来就有一份人正在用的值——窗口的输入框、
//! 模型路径、逗号分隔的乐器组键。外部调用方省略哪一项，编辑器就照窗上那一份走；
//! 写成 `non-Option` 就等于强迫每次调用把所有字段填全，漏一个就把窗上的设置覆盖掉。
//!
//! 字段名**拼错会被当场拒**（`deny_unknown_fields`），理由和 `midi.rs` 顶部那段一样：
//! 这里一个拼错的字段等于「那一项没生效、其余照旧」，回执看着完全正常。
//!
//! 许可不在这份结构里，也不提供任何「AI 代填」的入口：MuScriptor 的权重是 CC BY-NC 4.0、
//! 环境靠 uv 现装，这两件事只有人自己在那扇窗口里勾过「确认」才算数。

use serde::{Deserialize, Serialize};

/// 一次转录（或模型预检）的参数。省略 = 用转录窗当前那一份。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields, default)]
pub struct TranscribeOptions {
    #[cfg_attr(
        feature = "schema",
        schemars(description = "模型名：small / medium / large；给了 model_path 时它只用来挑架构")
    )]
    pub model: Option<String>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "本地 .safetensors 权重路径（不接受网址或包名）；省略 = 按 model 用已缓存的那份")
    )]
    pub model_path: Option<String>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "算在哪：auto / cpu / cuda。cuda 要环境安装时勾过 CUDA 构建")
    )]
    pub device: Option<String>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "数值精度：auto / float32 / float16（0.4.0 先按 FP32 构建再转，显存不够会直接报下界）")
    )]
    pub dtype: Option<String>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "只认这些乐器组键，逗号分隔的写法在窗上是文本框，这里给数组，如 [\"acoustic_piano\",\"voice\"]；省略 = 空 = 全部")
    )]
    pub instruments: Option<Vec<String>>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "采样解码（更慢、结果会变）；省略 = 窗上当前的勾")
    )]
    pub use_sampling: Option<bool>,
    #[cfg_attr(feature = "schema", schemars(description = "采样温度 0..=2，仅 use_sampling 有用"))]
    pub temperature: Option<f64>,
    #[cfg_attr(feature = "schema", schemars(description = "beam 宽度 1..=4 的整数，越大越慢"))]
    pub beam_size: Option<u32>,
    #[cfg_attr(feature = "schema", schemars(description = "类引导系数 0..=3"))]
    pub cfg_coef: Option<f64>,
    #[cfg_attr(
        feature = "schema",
        schemars(description = "严格 EOS 收尾，出厂 true；省略 = 窗上当前的勾")
    )]
    pub strict_eos: Option<bool>,
}

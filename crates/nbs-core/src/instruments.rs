//! 内置乐器表（对应原版 `first_custom_index = instrument_list` 的长度）。
//! 原版装机版（NBS 3.12，支持 MC 26.1）有 20 种：0..15 是经典十六种，
//! 16..19 是铜块一族（`Data/Sounds/trumpet*.ogg`，音效事件 `block.note_block.trumpet*`，
//! 依据 https://noteblock.studio/nbs 的 v6 乐器表）。
//! 方块配色在这里定义，与渲染层解耦。

use crate::model::FIRST_CUSTOM_INDEX;

#[derive(Clone, Copy, Debug)]
pub struct Instrument {
    pub name: &'static str,
    /// 方块主色（RGB）。
    pub color: (u8, u8, u8),
}

pub const INSTRUMENTS: [Instrument; 20] = [
    Instrument {
        name: "Harp",
        color: (96, 190, 130),
    },
    Instrument {
        name: "Double Bass",
        color: (92, 140, 190),
    },
    Instrument {
        name: "Bass Drum",
        color: (190, 96, 96),
    },
    Instrument {
        name: "Snare Drum",
        color: (206, 150, 78),
    },
    Instrument {
        name: "Click",
        color: (150, 150, 160),
    },
    Instrument {
        name: "Guitar",
        color: (176, 128, 190),
    },
    Instrument {
        name: "Flute",
        color: (126, 196, 196),
    },
    Instrument {
        name: "Bell",
        color: (220, 196, 96),
    },
    Instrument {
        name: "Chime",
        color: (140, 206, 216),
    },
    Instrument {
        name: "Xylophone",
        color: (206, 126, 126),
    },
    Instrument {
        name: "Iron Xylophone",
        color: (160, 164, 172),
    },
    Instrument {
        name: "Cow Bell",
        color: (196, 176, 110),
    },
    Instrument {
        name: "Didgeridoo",
        color: (150, 116, 86),
    },
    Instrument {
        name: "Bit",
        color: (120, 170, 220),
    },
    Instrument {
        name: "Banjo",
        color: (216, 146, 96),
    },
    Instrument {
        name: "Pling",
        color: (176, 196, 116),
    },
    // 铜块一族的配色是装机版自带 `instrumenttextures.zip` 里对应方块贴图
    // （copper_block / exposed_copper / weathered_copper / oxidized_copper，16×16 RGB）
    // 的逐像素平均值。
    Instrument {
        name: "Trumpet",
        color: (160, 101, 95),
    },
    Instrument {
        name: "Exposed Trumpet",
        color: (147, 110, 107),
    },
    Instrument {
        name: "Weathered Trumpet",
        color: (107, 132, 114),
    },
    Instrument {
        name: "Oxidized Trumpet",
        color: (96, 127, 123),
    },
];

pub fn instrument(index: u8) -> Option<&'static Instrument> {
    INSTRUMENTS.get(index as usize)
}

pub fn instrument_name(index: u8) -> String {
    match instrument(index) {
        Some(i) => i.name.to_string(),
        None => format!(
            "{} {}",
            crate::tr("Custom instrument", "自定义音色"),
            index - FIRST_CUSTOM_INDEX + 1,
        ),
    }
}

/// 方块配色：内置乐器用固定色，自定义乐器按索引循环取色。
pub fn instrument_color(index: u8) -> (u8, u8, u8) {
    match instrument(index) {
        Some(i) => i.color,
        None => {
            let palette = [
                (200, 120, 190),
                (120, 180, 210),
                (210, 170, 110),
                (150, 200, 140),
                (200, 130, 130),
                (140, 160, 210),
            ];
            palette[(index as usize - FIRST_CUSTOM_INDEX as usize) % palette.len()]
        }
    }
}

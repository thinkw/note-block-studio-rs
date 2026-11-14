//! 内置乐器表（对应原版 `first_custom_index = 16`）。
//! 方块颜色/形状索引在这里定义，与渲染层解耦，方便后续换成原版精灵贴图。

#[derive(Clone, Copy, Debug)]
pub struct Instrument {
    pub name: &'static str,
    /// 方块主色（RGB）。
    pub color: (u8, u8, u8),
    /// 0=方块 1=圆 2=菱形 3=三角 4=星形（use_shapes 时使用）。
    pub shape: u8,
    pub short: &'static str,
}

pub const INSTRUMENTS: [Instrument; 16] = [
    Instrument { name: "Harp",             color: ( 96, 190, 130), shape: 0, short: "Hp" },
    Instrument { name: "Double Bass",      color: ( 92, 140, 190), shape: 1, short: "DB" },
    Instrument { name: "Bass Drum",        color: (190,  96,  96), shape: 1, short: "BD" },
    Instrument { name: "Snare Drum",       color: (206, 150,  78), shape: 2, short: "SD" },
    Instrument { name: "Click",            color: (150, 150, 160), shape: 3, short: "Ck" },
    Instrument { name: "Guitar",           color: (176, 128, 190), shape: 0, short: "Gt" },
    Instrument { name: "Flute",            color: (126, 196, 196), shape: 2, short: "Fl" },
    Instrument { name: "Bell",             color: (220, 196,  96), shape: 1, short: "Bl" },
    Instrument { name: "Chime",            color: (140, 206, 216), shape: 3, short: "Ch" },
    Instrument { name: "Xylophone",        color: (206, 126, 126), shape: 0, short: "Xy" },
    Instrument { name: "Iron Xylophone",   color: (160, 164, 172), shape: 0, short: "IX" },
    Instrument { name: "Cow Bell",         color: (196, 176, 110), shape: 2, short: "CB" },
    Instrument { name: "Didgeridoo",       color: (150, 116,  86), shape: 1, short: "Dg" },
    Instrument { name: "Bit",              color: (120, 170, 220), shape: 3, short: "Bt" },
    Instrument { name: "Banjo",            color: (216, 146,  96), shape: 0, short: "Bj" },
    Instrument { name: "Pling",            color: (176, 196, 116), shape: 2, short: "Pl" },
];

pub fn instrument(index: u8) -> Option<&'static Instrument> {
    INSTRUMENTS.get(index as usize)
}

pub fn instrument_name(index: u8) -> String {
    match instrument(index) {
        Some(i) => i.name.to_string(),
        None => format!("自定义音色 {}", index - 16 + 1),
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
            palette[(index as usize - 16) % palette.len()]
        }
    }
}

pub fn instrument_shape(index: u8) -> u8 {
    instrument(index).map(|i| i.shape).unwrap_or((index % 4) as u8)
}

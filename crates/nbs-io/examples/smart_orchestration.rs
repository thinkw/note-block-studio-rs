//! 临时探针：智能编排最小案例——Pure Furies 长笛通道四档对比。不属于产品代码。
//! cargo run --release --example smart_orchestration -p nbs-io
//!
//! 音域模型（装机版 GML dat_pitch 原文 + f0 实测基域推出，只读验证）：
//!   游戏内听感（MIDI 口径）= S_ins + 12·log2(pitch)，S_ins = 采样原速的 MIDI 音高。
//!   编辑器同式 → 谱面 key 对非 45 基域乐器是"偏移记谱"，GM 表 octave 字段做补偿。
//!   原生档（谱面 33..57）可达听感域 = [S−12, S+12]；扩展档（谱面 9..81，_±1）= [S−36, S+36]。
//!   基域 S：bass/didgeridoo 42（f0 实测 21/21.1）、guitar/trumpet_weathered 54（33）、
//!   harp/bit/banjo/pling/trumpet 66（45）、chime 71（50）；flute 按 GM 表 oct=0 推 45 域 → S=66。
//!   （chime 是高音侧候选，本曲长笛无高音界外，未触发。）
//!   Tier-A（bass 等）：资源包变体与内置采样同源 → 扩展段音色与编辑器一致；
//!   Tier-B（flute 等 8 件）：不同源 → 音色差（音高仍精确）。
use nbs_core::model::{MC_KEY_MAX, MC_KEY_MIN, Song};
use nbs_io::midi::{import_midi_path, parse_midi_path, MidiImportOptions};
use nbs_io::midi_gm::MidiGmTable;

const FLUTE: u8 = 6;
const BASS: u8 = 1;
const S_FLUTE: i32 = 66;
const S_BASS: i32 = 42;
/// 谱面 key = 听感 m − S + 45。
const NATIVE: std::ops::RangeInclusive<i32> = MC_KEY_MIN as i32..=MC_KEY_MAX as i32;
const EXTEND: std::ops::RangeInclusive<i32> = 9..=81;

fn fold_key(k: i32) -> i32 {
    let mut k = k;
    while k < MC_KEY_MIN as i32 {
        k += 12;
    }
    while k > MC_KEY_MAX as i32 {
        k -= 12;
    }
    k
}

fn main() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/Songs/Whereabouts of the Heart ~ Pure Furies.mid");
    let midi = parse_midi_path(&path).expect("解析 MIDI");
    let opts = MidiImportOptions::for_midi(&midi, &MidiGmTable::default());
    let song: Song = import_midi_path(&path, &opts).expect("导入");

    let flute: Vec<(u32, u32, i32)> = song
        .iter_notes()
        .filter(|(_, _, n)| n.instrument == FLUTE)
        .map(|(x, y, n)| (x, y, n.key as i32))
        .collect();
    let n = flute.len();
    println!(
        "长笛通道：音符 {n} | 谱面 key {}..{} | 听感 m {}..{} | 原生段内 {} | 界上 {} | 界下 {}",
        flute.iter().map(|(_, _, k)| *k).min().unwrap_or(0),
        flute.iter().map(|(_, _, k)| *k).max().unwrap_or(0),
        flute.iter().map(|(_, _, k)| k + 21).min().unwrap_or(0),
        flute.iter().map(|(_, _, k)| k + 21).max().unwrap_or(0),
        flute.iter().filter(|(_, _, k)| NATIVE.contains(k)).count(),
        flute.iter().filter(|(_, _, k)| *k > MC_KEY_MAX as i32).count(),
        flute.iter().filter(|(_, _, k)| *k < MC_KEY_MIN as i32).count(),
    );

    // 四档：(乐器, 谱面key, 听感m, 档位说明)
    // A 原版折八度；B1 免包智能（native 同件→域内换乐器→折八度兜底）；
    // B2 用包智能（native 同件→同件扩展→Tier-A 乐器扩展兜底）；C 原样不折。
    let mut a_ok = 0usize;
    let mut a_shift = 0usize;
    let (mut b1_f, mut b1_bass, mut b1_fold) = (0usize, 0usize, 0usize);
    let (mut b2_f_nat, mut b2_f_ext, mut b2_bass_ext) = (0usize, 0usize, 0usize);
    let (mut c_nat, mut c_ext, mut c_drop) = (0usize, 0usize, 0usize);
    for (x, _, k) in flute.iter().copied() {
        let m = k + 21;
        // A
        if fold_key(k) + S_FLUTE - 45 == m {
            a_ok += 1;
        } else {
            a_shift += 1;
        }
        // B1：flute native → bass native（低音侧）→ 折八度兜底
        if NATIVE.contains(&k) {
            b1_f += 1;
        } else if (S_BASS - 12..=S_BASS + 12).contains(&m) {
            b1_bass += 1;
        } else {
            b1_fold += 1;
        }
        // B2：flute native → flute 扩展 → bass 扩展兜底
        if NATIVE.contains(&k) {
            b2_f_nat += 1;
        } else if EXTEND.contains(&k) {
            b2_f_ext += 1;
        } else {
            let kb = m - S_BASS + 45;
            if EXTEND.contains(&kb) {
                b2_bass_ext += 1;
            } else {
                b2_f_ext += 1; // 理论到不了：key 5..8 → kb 29..32
            }
        }
        // C
        if NATIVE.contains(&k) {
            c_nat += 1;
        } else if EXTEND.contains(&k) {
            c_ext += 1;
        } else {
            c_drop += 1;
        }
        let _ = x;
    }
    let p = |x: usize| format!("{:>5.1}%", 100.0 * x as f32 / n as f32);
    println!("\n档A 原版折八度 : 保真 {}{} | 丢 0 | 要包 否 | 音色差 0 | 音高错 {}{}（全为上移12半音）", a_ok, p(a_ok), a_shift, p(a_shift));
    println!(
        "档B1 免包智能  : 保真 {}{}（flute原生 {} + 换bass {}）| 丢 0 | 要包 否 | 音色差 0 | 音高错 {}{}（折八度兜底）| 换乐器 {}",
        b1_f + b1_bass, p(b1_f + b1_bass), b1_f, b1_bass, b1_fold, p(b1_fold), b1_bass,
    );
    println!(
        "档B2 用包智能  : 保真 {}{}（flute原生 {} + flute扩展 {} + bass扩展兜底 {}）| 丢 0 | 要包 是 | 音色差 {}（flute扩展段，Tier-B）| 0 折音",
        b2_f_nat + b2_f_ext + b2_bass_ext,
        p(b2_f_nat + b2_f_ext + b2_bass_ext),
        b2_f_nat, b2_f_ext, b2_bass_ext, b2_f_ext,
    );
    println!(
        "档C 原样不折   : 保真 {}{} | 丢 {}{} | 要包 是 | 音色差 {}（flute扩展段）| 0 折音",
        n, p(n), c_drop, p(c_drop), c_ext,
    );

    println!("\n乐句抽样（tick | 原谱面 | 档A | 档B1 | 档B2 | 档C）  F=flute B=bass");
    let mut shown = 0usize;
    for (x, _, k) in flute.iter().copied() {
        if shown >= 20 {
            break;
        }
        let m = k + 21;
        let b1 = if NATIVE.contains(&k) {
            format!("F{k}")
        } else if (S_BASS - 12..=S_BASS + 12).contains(&m) {
            format!("B{}", m - S_BASS + 45)
        } else {
            format!("F{}", fold_key(k))
        };
        let b2 = if NATIVE.contains(&k) {
            format!("F{k}")
        } else if EXTEND.contains(&k) {
            format!("F{k}⁺")
        } else {
            format!("B{}⁺", m - S_BASS + 45)
        };
        let c = if NATIVE.contains(&k) {
            format!("F{k}")
        } else if EXTEND.contains(&k) {
            format!("F{k}⁺")
        } else {
            format!("F{k}✕")
        };
        println!("tick {x:>4} | F{k:<3} | F{:<3} | {:<4} | {:<5} | {c}", fold_key(k), b1, b2);
        shown += 1;
    }
    println!("（⁺=扩展档事件 _1，✕=普通导出丢弃）");
}

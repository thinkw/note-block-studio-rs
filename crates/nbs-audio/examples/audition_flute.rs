//! 临时探针：长笛通道四档 + 游戏近似 试听对比（WAV）。不属于产品代码。
//! cargo run --release --example audition_flute -p nbs-audio
//!
//! 只改长笛（instrument 6）声部，其余声部逐位相同。窗口自动取界外长笛音最密的区段。
//!   C  原样不折      —— 界外音保留原谱面 key，编辑器按 ratio<0.5 直接拉伸内置采样
//!   A  原版折八度    —— 界外音折进 33..57（音高错八度；编辑器=游戏听感）
//!   B1 免包换贝斯    —— 界外低音交给 bass 原生域（听感不变的换乐器）
//!   B2 用包+bass兜底 —— 同 C，但谱面 <9 的音（数据包没有对应事件）改走 bass 救回
//!   G  游戏近似      —— 谱面 9..32 的界外长笛音改用**资源包的 flute_-1.ogg 实际文件**。
//!                       mixer 公式 keyshift = key + (ins.key − 78)，取 ins.key = 69 时
//!                       ratio = 0.5·2^((k−9)/12)，与 dat_pitch 低段逐位相同；
//!                       已由 audition_check 实测：同谱面 9 上资源包近似与内置只差 12.8 音分。
//! 听感口径（全部实测，见 audition_check）：MIDI = S_ins + (谱面key − 45)，
//!   S_flute = 78、S_bass = 42 —— 所以长笛谱面 k 的听感 = k + 33，同音搬到 bass 要写 k + 36。
//!   （长笛的 78 是这次量出来的；先前按 GM 表推成 66，差一个八度，本探针上一版就错在这儿。）
use nbs_core::model::{CustomInstrument, FIRST_CUSTOM_INDEX, MC_KEY_MAX, MC_KEY_MIN, Note, Song};
use nbs_core::nbs;
use nbs_audio::engine::{export_wav, ExportOptions};
use nbs_audio::samples::SoundBank;
use std::path::{Path, PathBuf};

const FLUTE: u8 = 6;
const BASS: u8 = 1;
/// 原始采样自身音高（谱面 45 时的听感 MIDI），audition_check 实测值。
const S_FLUTE: i32 = 78;
const S_BASS: i32 = 42;
/// 挂资源包 flute_-1.ogg 时自定义乐器的基准 key，取 69 才能复现 dat_pitch 低段的 ratio。
const PACK_BASE_KEY: u8 = 69;
const SPAN: u32 = 900;
const OUT_DIR: &str = "D:/develop/rust-project/scratch/试听对比-2026-11-17";
const PACK_FLUTE: &str =
    "D:/develop/rust-project/scratch/extranotes_probe/ext/assets/minecraft/sounds/block/note_block/flute_-1.ogg";

fn native(k: i32) -> bool {
    (MC_KEY_MIN as i32..=MC_KEY_MAX as i32).contains(&k)
}

fn fold_key(mut k: i32) -> i32 {
    while k < MC_KEY_MIN as i32 {
        k += 12;
    }
    while k > MC_KEY_MAX as i32 {
        k -= 12;
    }
    k
}

/// 长笛谱面 key 的听感 MIDI。
fn heard(k: i32) -> i32 {
    S_FLUTE + (k - 45)
}

/// 把听感 MIDI 换算成 bass 的谱面 key（bass 原生可达 = S_BASS ± 12）。
fn bass_key(m: i32) -> u8 {
    (m - S_BASS + 45) as u8
}

fn bass_reachable(m: i32) -> bool {
    (S_BASS - 12..=S_BASS + 12).contains(&m)
}

/// 资源包 flute_-1 是否可用；可用时它占用 FIRST_CUSTOM_INDEX 这一个槽位。
fn pack_available() -> bool {
    Path::new(PACK_FLUTE).exists()
}

/// 裁出 [start, start+SPAN) 的窗口副本，必要时挂上资源包自定义乐器。
fn window_song(song: &Song, start: u32, with_pack: bool) -> Song {
    let end = start + SPAN;
    let mut w = Song::new();
    w.layers = song.layers.clone();
    w.meta.tempo = song.meta.tempo;
    if with_pack {
        w.custom_instruments.push(CustomInstrument {
            name: "flute_-1(资源包)".into(),
            filename: PACK_FLUTE.replace('\\', "/"),
            key: PACK_BASE_KEY,
            press: false,
        });
    }
    for (x, y, n) in song.iter_notes() {
        if x >= start && x < end {
            w.set(x - start, y, n);
        }
    }
    w.recalculate();
    w
}

/// 对长笛音符套用变换；返回被改动的音符数。
fn rewrite_flute(song: &mut Song, f: impl Fn(i32) -> Option<(u8, u8)>) -> usize {
    let cells: Vec<(u32, u32, Note)> = song
        .iter_notes()
        .filter(|(_, _, n)| n.instrument == FLUTE)
        .map(|(x, y, n)| (x, y, n))
        .collect();
    let mut changed = 0;
    for (x, y, mut n) in cells {
        if let Some((ins, key)) = f(n.key as i32) {
            if ins != n.instrument || key != n.key {
                n.instrument = ins;
                n.key = key;
                song.set(x, y, n);
                changed += 1;
            }
        }
    }
    changed
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let song =
        nbs::load(&root.join("assets/Songs/Whereabouts of the Heart ~ Pure Furies.nbs"))
            .expect("读取 Pure Furies.nbs");
    let bank_missing = {
        let b = SoundBank::load_from(&root.join("assets/Sounds"));
        b.missing
    };
    if !bank_missing.is_empty() {
        println!("内置采样缺失: {bank_missing:?}");
    }
    let has_pack = pack_available();
    println!(
        "资源包采样: {}",
        if has_pack { PACK_FLUTE.to_string() } else { "未找到，G 档无法渲染".to_string() }
    );
    std::fs::create_dir_all(OUT_DIR).expect("创建输出目录");
    let out = Path::new(OUT_DIR);

    // 界外长笛音的分布，挑最密的 SPAN 窗口
    let flute: Vec<(u32, i32)> = song
        .iter_notes()
        .filter(|(_, _, n)| n.instrument == FLUTE)
        .map(|(x, _, n)| (x, n.key as i32))
        .collect();
    let offs: Vec<(u32, i32)> = flute.iter().filter(|(_, k)| !native(*k)).copied().collect();
    let deepest: Vec<u32> = offs.iter().filter(|(_, k)| *k < 9).map(|(x, _)| *x).collect();
    let mut best = (0u32, 0usize);
    for &start in &{
        let mut s: Vec<u32> = offs.iter().map(|(x, _)| x.saturating_sub(SPAN / 3)).collect();
        s.sort();
        s.dedup();
        s
    } {
        let c = offs.iter().filter(|(x, _)| *x >= start && *x < start + SPAN).count();
        if c > best.1 {
            best = (start, c);
        }
    }
    println!(
        "全曲长笛 {} 个，界外 {} 个（谱面 <9 的 {} 个，tick {}..{}）| 选中窗口 [{},{}) 含界外 {} 个",
        flute.len(),
        offs.len(),
        deepest.len(),
        deepest.iter().min().copied().unwrap_or(0),
        deepest.iter().max().copied().unwrap_or(0),
        best.0,
        best.0 + SPAN,
        best.1,
    );

    let opts = ExportOptions::default();
    let render = |name: &str, s: &Song, pack: bool| {
        let mut b = SoundBank::load_from(&root.join("assets/Sounds"));
        if pack {
            b.extend_for_song(s);
        }
        let path = out.join(name);
        match export_wav(s, &b, &path, &opts) {
            Ok(st) => println!(
                "{name:34} 音符 {:>4} | {:.2}s | 峰值 {:.3}",
                st.notes, st.duration_seconds, st.peak
            ),
            Err(e) => println!("{name:34} 渲染失败: {e}"),
        }
    };

    let start = best.0;
    render("00-C原样不折.wav", &window_song(&song, start, false), false);

    // A：原版做法，界外音折八度（音高错一个八度，但音色不变、不用包）
    let mut a = window_song(&song, start, false);
    let na = rewrite_flute(&mut a, |k| if native(k) { None } else { Some((FLUTE, fold_key(k) as u8)) });
    render("01-A原版折八度.wav", &a, false);

    // B1：免包，能在 bass 原生域内保持音高的就换 bass，剩下的仍折八度
    let (n_b1_bass, n_b1_fold) = (std::cell::Cell::new(0usize), std::cell::Cell::new(0usize));
    let mut b1 = window_song(&song, start, false);
    let nb1 = rewrite_flute(&mut b1, |k| {
        if native(k) {
            return None;
        }
        if bass_reachable(heard(k)) {
            n_b1_bass.set(n_b1_bass.get() + 1);
            Some((BASS, bass_key(heard(k))))
        } else {
            n_b1_fold.set(n_b1_fold.get() + 1);
            Some((FLUTE, fold_key(k) as u8))
        }
    });
    render("02-B1免包换贝斯.wav", &b1, false);
    println!("  B1 内部：换 bass {} 个（听感不变）、折八度 {} 个（高 12 半音）", n_b1_bass.get(), n_b1_fold.get());

    // B2：界外音原样保留（编辑器能播），只把数据包彻底没有事件的谱面 <9 交给 bass
    let mut b2 = window_song(&song, start, false);
    let nb2 = rewrite_flute(&mut b2, |k| {
        if k >= 9 {
            return None;
        }
        Some((BASS, bass_key(heard(k))))
    });
    render("03-B2用包bass兜底.wav", &b2, false);

    // G：9..32 换成资源包实际文件（游戏听到的就是这个）；<9 游戏根本没事件，仍用 bass 兜底
    if has_pack {
        let mut g = window_song(&song, start, true);
        let ng = rewrite_flute(&mut g, |k| {
            if native(k) {
                return None;
            }
            if k < 9 {
                Some((BASS, bass_key(heard(k))))
            } else {
                Some((FIRST_CUSTOM_INDEX, k as u8))
            }
        });
        render("04-G游戏近似.wav", &g, true);
        println!("长笛改动数：A {na} | B1 {nb1} | B2 {nb2} | G {ng}");
    } else {
        println!("长笛改动数：A {na} | B1 {nb1} | B2 {nb2} | G 未渲染（缺资源包文件）");
    }
    println!("输出目录：{OUT_DIR}");
}

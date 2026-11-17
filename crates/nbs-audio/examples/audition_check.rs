//! 临时探针：试听渲染的音高校准（单音实测）。不属于产品代码。
//! cargo run --release --example audition_check -p nbs-audio
//!
//! 三件事，全部用渲染出来的声音判定，不靠推导：
//! 1. 斜率：同一乐器谱面 +12 是否恰好 +1200 音分 —— 顺带证明测频方法本身可信。
//! 2. 「游戏近似」档的做法（把资源包 flute_-1.ogg 挂成自定义乐器、基准 key 取 69）
//!    是否与编辑器内置长笛落在同一个音上；基准 key 取 33（我第一次推的错误值）应差两个八度。
//! 3. 乐器锚点：谱面 45 时听感 MIDI 是多少（= 原始采样自身的音高）。
//!    这是换乐器救音时唯一的依据，长笛原先的 66 是从 GM 表推的，这里实测。
//!    每档同时报「半频幅值 / 峰值幅值」，防止最强分量其实是泛音。
use nbs_core::model::{CustomInstrument, FIRST_CUSTOM_INDEX, Note, Song};
use nbs_audio::engine::{export_wav, ExportOptions};
use nbs_audio::samples::SoundBank;
use std::path::{Path, PathBuf};

const FLUTE: u8 = 6;
const PACK_SLOT: u8 = FIRST_CUSTOM_INDEX;
const EXT_DIR: &str =
    "D:/develop/rust-project/scratch/extranotes_probe/ext/assets/minecraft/sounds/block/note_block";
const TMP: &str = "D:/develop/rust-project/scratch/试听对比-2026-11-17/_check";

/// 内置乐器在谱面 key 上的单音。
fn builtin_note(ins: u8, key: u8) -> Song {
    let mut s = Song::new();
    s.set(0, 0, Note::new(ins, key));
    s.recalculate();
    s
}

/// 资源包实际录音挂成自定义乐器（下标 PACK_SLOT）后的单音。
fn pack_note(file: &str, slot_key: u8, key: u8) -> Song {
    let mut s = Song::new();
    s.custom_instruments.push(CustomInstrument {
        name: file.trim_end_matches(".ogg").into(),
        filename: Path::new(EXT_DIR).join(file).to_string_lossy().replace('\\', "/"),
        key: slot_key,
        press: false,
    });
    s.set(0, 0, Note::new(PACK_SLOT, key));
    s.recalculate();
    s
}

/// 读 WAV（本仓库 write_wav_i16 的产物），返回单声道 f32 + 采样率。
fn read_wav(path: &Path) -> (Vec<f32>, u32) {
    let b = std::fs::read(path).expect("读 WAV");
    let rate = u32::from_le_bytes([b[24], b[25], b[26], b[27]]);
    let ch = u16::from_le_bytes([b[22], b[23]]) as usize;
    let mut off = 12;
    let (mut data, mut size) = (0usize, 0usize);
    while off + 8 <= b.len() {
        let id = &b[off..off + 4];
        let sz = u32::from_le_bytes([b[off + 4], b[off + 5], b[off + 6], b[off + 7]]) as usize;
        if id == b"data" {
            data = off + 8;
            size = sz;
            break;
        }
        off += 8 + sz + (sz & 1);
    }
    assert!(data > 0, "WAV 里没有 data 块");
    let s16 = |frame: &[u8], i: usize| i16::from_le_bytes([frame[i * 2], frame[i * 2 + 1]]) as f32 / 32768.0;
    let mut x = Vec::with_capacity(size / 2 / ch);
    for frame in b[data..(data + size).min(b.len())].chunks(ch * 2) {
        x.push(if ch > 1 { (s16(frame, 0) + s16(frame, 1)) * 0.5 } else { s16(frame, 0) });
    }
    (x, rate)
}

/// 单频点幅值（窗口内单 bin DFT，增量旋转）。
fn tone(x: &[f32], rate: u32, f: f64) -> f64 {
    let start = x.len().min(2000);
    let n = x.len().min(start + (1 << 14));
    let s = &x[start..n];
    if s.len() < 1000 || f <= 0.0 {
        return 0.0;
    }
    let w = 2.0 * std::f64::consts::PI * f / rate as f64;
    let (dc, ds) = (w.cos(), w.sin());
    let (mut c, mut sn) = (1.0f64, 0.0f64);
    let (mut sr, mut si) = (0.0f64, 0.0f64);
    for &v in s.iter() {
        sr += v as f64 * c;
        si += v as f64 * sn;
        let nc = c * dc - sn * ds;
        sn = c * ds + sn * dc;
        c = nc;
    }
    (sr * sr + si * si).sqrt() / s.len() as f64
}

/// 最强分量：对数栅格粗扫 + 峰附近细化。栅格步长小于窗频率分辨率，靠细化补。
fn f_peak(x: &[f32], rate: u32) -> f32 {
    let mut best = (0.0f64, 0.0f64);
    let mut f = 20.0f64;
    while f < 4000.0 {
        let m = tone(x, rate, f);
        if m > best.1 {
            best = (f, m);
        }
        f *= 1.002;
    }
    let (mut g, hi) = (best.0 / 1.006, (best.0 * 1.006).min(3999.0));
    while g <= hi {
        let m = tone(x, rate, g);
        if m > best.1 {
            best = (g, m);
        }
        g *= 1.00003;
    }
    best.0 as f32
}

/// A4 = 440 Hz 的 MIDI 音高。
fn midi(f: f32) -> f32 {
    69.0 + 12.0 * (f64::from(f) / 440.0).log2() as f32
}

fn cents(a: f32, b: f32) -> f32 {
    1200.0 * (f64::from(a) / f64::from(b)).log2() as f32
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::create_dir_all(TMP).expect("建临时目录");
    let opts = ExportOptions { sample_rate: 44100, channels: 1, include_locked: false };

    let cases: Vec<(String, Song, bool)> = vec![
        ("内置 flute @谱面9".into(), builtin_note(FLUTE, 9), false),
        ("内置 flute @谱面21".into(), builtin_note(FLUTE, 21), false),
        ("包 flute_-1 @谱面9 (基准 69)".into(), pack_note("flute_-1.ogg", 69, 9), true),
        ("包 flute_-1 @谱面21 (基准 69)".into(), pack_note("flute_-1.ogg", 69, 21), true),
        ("包 flute_-1 @谱面9 (基准 33)".into(), pack_note("flute_-1.ogg", 33, 9), true),
        ("内置 flute @谱面45".into(), builtin_note(FLUTE, 45), false),
        ("内置 bass @谱面45".into(), builtin_note(1, 45), false),
        ("内置 harp @谱面45".into(), builtin_note(0, 45), false),
        ("内置 guitar @谱面45".into(), builtin_note(5, 45), false),
        ("内置 didgeridoo @谱面45".into(), builtin_note(12, 45), false),
        ("内置 bass @谱面33（上一版 B1 用的档位）".into(), builtin_note(1, 33), false),
    ];
    let mut fs: Vec<f32> = Vec::new();
    for (i, (label, song, pack)) in cases.iter().enumerate() {
        let mut bank = SoundBank::load_from(&root.join("assets/Sounds"));
        if *pack {
            bank.extend_for_song(song);
        }
        let path = Path::new(TMP).join(format!("note{i}.wav"));
        let st = export_wav(song, &bank, &path, &opts).expect("渲染");
        let (x, rate) = read_wav(&path);
        let f = f_peak(&x, rate);
        // 半频/三分之一频若真有能量，说明最强分量其实是泛音，读数要打折看
        let half = tone(&x, rate, f64::from(f) / 2.0) / tone(&x, rate, f64::from(f)).max(1e-9);
        let third = tone(&x, rate, f64::from(f) / 3.0) / tone(&x, rate, f64::from(f)).max(1e-9);
        fs.push(f);
        println!(
            "{label:34} 帧 {:>7} 峰值 {:.3} | 最强分量 {:>8.2} Hz MIDI {:>5.1} | 半频比 {half:.3} 三分频比 {third:.3}",
            st.frames, st.peak, f, midi(f)
        );
    }

    let line = |tag: &str, got: f32, want: f32, tol: f32| {
        let ok = (got - want).abs() <= tol;
        println!("{tag:32} 实测 {got:+8.1} 音分 | 期望 {want:+6.0} ±{tol:.0} => {}", if ok { "OK" } else { "FAIL" });
        ok
    };
    println!();
    let ok_slope = line("内置 谱面+12", cents(fs[1], fs[0]), 1200.0, 15.0);
    let ok_pack_slope = line("资源包 谱面+12 (基准 69)", cents(fs[3], fs[2]), 1200.0, 15.0);
    let ok_align = line("基准69 与内置 同谱面9", cents(fs[2], fs[0]), 0.0, 30.0);
    let wrong = line("基准33 与内置 同谱面9（该错）", cents(fs[4], fs[0]), 0.0, 30.0);
    // 换乐器救音的档位对不对，直接量：长笛谱面 9 的音高应当等于 bass 谱面 45（=9+36），
    // 上一版按 S_flute=66 写成 bass 谱面 33，应当低整整一个八度。
    let ok_rescue = line("bass@45 与 flute@9 同音", cents(fs[6], fs[0]), 0.0, 30.0);
    let rescue_old = line("bass@33 与 flute@9（旧错档）", cents(fs[10], fs[0]), -1200.0, 20.0);
    println!(
        "\n锚点（谱面 45 听感 MIDI = 原始采样音高）：flute {:.1} | bass {:.1} | harp {:.1} | guitar {:.1} | didgeridoo {:.1}",
        midi(fs[5]),
        midi(fs[6]),
        midi(fs[7]),
        midi(fs[8]),
        midi(fs[9])
    );
    println!(
        "斜率 {}｜资源包近似与编辑器同音 {}｜错误基准 {}",
        yes(ok_slope && ok_pack_slope),
        yes(ok_align),
        if wrong { "意外通过，口径另有问题" } else { "如预期差两个八度（低频读数只作方向参考）" }
    );
}

fn yes(b: bool) -> &'static str {
    if b {
        "通过"
    } else {
        "不通过"
    }
}

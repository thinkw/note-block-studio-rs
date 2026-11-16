//! 临时探针：实测基频 + 波形级移位验证。不属于产品代码。
//! cargo run --release --example sample_f0 -p nbs-audio
use nbs_audio::samples::decode_ogg;
use std::path::{Path, PathBuf};

const RATE: f32 = 44100.0;
const EXT_DIR: &str = "D:/develop/rust-project/scratch/extranotes_probe/ext/assets/minecraft/sounds/block/note_block";

/// (事件名, 扩展资源包文件名词干, 随程序内置采样文件名)
const TABLE: [(&str, &str, &str); 20] = [
    ("harp", "harp", "harp"),
    ("bass", "bassattack", "dbass"),
    ("basedrum", "basedrum", "bdrum"),
    ("snare", "snare", "sdrum"),
    ("hat", "hat", "click"),
    ("guitar", "guitar", "guitar"),
    ("flute", "flute", "flute"),
    ("bell", "bell", "bell"),
    ("chime", "icechime", "icechime"),
    ("xylophone", "xylobone", "xylobone"),
    ("iron_xylophone", "iron_xylophone", "iron_xylophone"),
    ("cow_bell", "cow_bell", "cow_bell"),
    ("didgeridoo", "didgeridoo", "didgeridoo"),
    ("bit", "bit", "bit"),
    ("banjo", "banjo", "banjo"),
    ("pling", "pling", "pling"),
    ("trumpet", "trumpet", "trumpet"),
    ("trumpet_exposed", "trumpet_exposed", "trumpet_exposed"),
    ("trumpet_weathered", "trumpet_weathered", "trumpet_weathered"),
    ("trumpet_oxidized", "trumpet_oxidized", "trumpet_oxidized"),
];

const SHIFTS: [i32; 7] = [-36, -24, -12, 0, 12, 24, 36];
const PAIR_SHIFTS: [i32; 7] = [-48, -36, -24, 0, 24, 36, 48];

fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, &v| m.max(v.abs()))
}

/// 就地 radix-2 FFT。
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f32::consts::PI / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        let mut i = 0;
        while i < n {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let a = i + k;
                let b = a + len / 2;
                let xr = re[b] * cr - im[b] * ci;
                let xi = re[b] * ci + im[b] * cr;
                re[b] = re[a] - xr;
                im[b] = im[a] - xi;
                re[a] += xr;
                im[a] += xi;
                let nr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = nr;
            }
            i += len;
        }
        len <<= 1;
    }
}

/// 单窗口谐波求和：候选 f0 得分 = 其 1..8 次谐波平均幅度 / 全谱平均幅度。
/// 得分接近 1 => 没有周期结构（噪声型采样）。
fn f0_one(x: &[f32]) -> Option<(f32, f32)> {
    let n = {
        let mut p = 1024usize;
        while p * 2 <= x.len() && p < 8192 {
            p *= 2;
        }
        p
    };
    if x.len() < n {
        return None;
    }
    let mut re = vec![0.0f32; n];
    let mut im = vec![0.0f32; n];
    for i in 0..n {
        let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32).cos();
        re[i] = x[i] * w;
    }
    fft(&mut re, &mut im);
    let mut mag = vec![0.0f32; n / 2];
    let mut total = 0.0f32;
    for k in 1..n / 2 {
        mag[k] = (re[k] * re[k] + im[k] * im[k]).sqrt();
        total += mag[k];
    }
    let mean_all = total / (n / 2 - 1) as f32;
    let bin = |f: f32| (((f / RATE) * n as f32).round() as usize).min(n / 2 - 1);
    let mut best = (0.0f32, 0.0f32);
    let mut f = 50.0f32;
    while f < 3000.0 {
        let mut s = 0.0f32;
        let mut used = 0usize;
        for h in 1..9 {
            let fh = f * h as f32;
            if fh >= 20000.0 {
                break;
            }
            s += mag[bin(fh)];
            used += 1;
        }
        if used >= 3 {
            s /= used as f32;
            if s > best.0 {
                best = (s, f);
            }
        }
        f *= 1.0015;
    }
    if best.1 == 0.0 || mean_all <= 0.0 {
        return None;
    }
    Some((best.1, best.0 / mean_all))
}

/// 三处窗口取中位，返回 (Hz, 最大得分, 跨窗口最大偏差半音)。偏差大 => 估计不可信或音高漂移。
fn f0_stable(x: &[f32]) -> Option<(f32, f32, f32)> {
    let mut vals = Vec::new();
    let mut score = 0.0f32;
    for frac in [0.02f32, 0.25, 0.5] {
        let start = ((x.len() as f32 * frac) as usize).max(1).min(x.len().saturating_sub(1024));
        let win = &x[start..x.len().min(start + 8192)];
        if let Some((f, s)) = f0_one(win) {
            vals.push(f);
            if s > score {
                score = s;
            }
        }
    }
    if vals.len() < 3 {
        return None;
    }
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = vals[vals.len() / 2];
    let spread = 12.0 * (vals[vals.len() - 1] / vals[0]).log2();
    Some((med, score, spread))
}

/// factor>1 => 加速播放（变短、音高上升）。线性插值。
fn resample(x: &[f32], factor: f32) -> Vec<f32> {
    let out_len = (x.len() as f32 / factor) as usize;
    let mut out = Vec::with_capacity(out_len.min(1 << 22));
    for i in 0..out_len {
        let src = i as f32 * factor;
        let i0 = src as usize;
        let frac = src - i0 as f32;
        let a = x[i0.min(x.len() - 1)];
        let b = x[(i0 + 1).min(x.len() - 1)];
        out.push(a + (b - a) * frac);
    }
    out
}

/// 归一化互相关，最多比 1 秒。
fn ncc(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len()).min(44100);
    if n < 1000 {
        return -2.0;
    }
    let (ma, mb) = (rms(&a[..n]) as f64, rms(&b[..n]) as f64);
    if ma < 1e-6 || mb < 1e-6 {
        return -2.0;
    }
    let (mut sab, mut saa, mut sbb) = (0.0f64, 0.0f64, 0.0f64);
    for i in 0..n {
        let x = a[i] as f64 - ma;
        let y = b[i] as f64 - mb;
        sab += x * y;
        saa += x * x;
        sbb += y * y;
    }
    let d = (saa * sbb).sqrt();
    if d < 1e-12 {
        -2.0
    } else {
        (sab / d) as f32
    }
}

/// base 按候选半音移位后与 variant 比波形：(最优移位, 最优相关, 次优相关)。
fn best_shift_corr(base: &[f32], variant: &[f32], shifts: &[i32]) -> (i32, f32, f32) {
    let mut res: Vec<(i32, f32)> = Vec::new();
    for &s in shifts {
        let shifted = resample(base, 2f32.powf(s as f32 / 12.0));
        if shifted.len() < 2000 || shifted.len() > 4_000_000 {
            continue;
        }
        let mut v_best = -2.0f32;
        let mut lag = 0usize;
        while lag + 2000 < shifted.len() {
            let v = ncc(&shifted[lag..], variant);
            if v > v_best {
                v_best = v;
            }
            lag += 147;
        }
        res.push((s, v_best));
    }
    if res.is_empty() {
        return (0, -2.0, -2.0);
    }
    res.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    (res[0].0, res[0].1, res.get(1).map(|x| x.1).unwrap_or(-2.0))
}

fn owned(m: Result<Vec<f32>, String>) -> Option<Vec<f32>> {
    match m {
        Ok(v) => Some(v),
        Err(_) => None,
    }
}

fn main() {
    let sounds = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/Sounds");
    let ext = Path::new(EXT_DIR);
    println!("扩展目录存在: {}  {}", ext.display(), ext.is_dir());
    println!(
        "乐器             内置Hz 得分 跨度 反推key | _1Hz 跨度 | _-1Hz 跨度 | 内置→_1      | 内置→_-1     | _-1→_1    | dur  峰值"
    );
    let mut keys = Vec::new();
    let mut pair_ok = 0usize;
    let mut base_ok = 0usize;
    let mut unstable = Vec::new();
    for (event, stem, file) in TABLE {
        let b = match decode_ogg(&sounds.join(format!("{file}.ogg"))) {
            Ok(v) => v,
            Err(e) => {
                println!("{event:16} 内置解码失败 {e}");
                continue;
            }
        };
        let st = f0_stable(&b);
        let (bh, bs, bspread) = st.unwrap_or((0.0, 0.0, 99.0));
        let trusted = matches!(st, Some((_, _, sp)) if sp < 0.6) && bs > 1.5;
        let key = if trusted { 69.0 + (bh / 440.0).log2() * 12.0 - 21.0 } else { -99.0 };
        if key > -90.0 {
            keys.push(key);
        } else {
            unstable.push(event);
        }
        let up = owned(decode_ogg(&ext.join(format!("{stem}_1.ogg"))));
        let dn = owned(decode_ogg(&ext.join(format!("{stem}_-1.ogg"))));
        let (uh, _, usp) = up.as_ref().and_then(|v| f0_stable(v)).unwrap_or((0.0, 0.0, 99.0));
        let (dh, _, dsp) = dn.as_ref().and_then(|v| f0_stable(v)).unwrap_or((0.0, 0.0, 99.0));
        let (su, cu, cu2) = up.as_ref().map(|u| best_shift_corr(&b, u, &SHIFTS)).unwrap_or((0, -2.0, -2.0));
        let (sd, cd, cd2) = dn.as_ref().map(|d| best_shift_corr(&b, d, &SHIFTS)).unwrap_or((0, -2.0, -2.0));
        let pair = match (&dn, &up) {
            (Some(d), Some(u)) => Some(best_shift_corr(d, u, &PAIR_SHIFTS)),
            _ => None,
        };
        if su == 24 && sd == -24 && cu > 0.5 && cd > 0.5 {
            base_ok += 1;
        }
        if let Some((48, c, _)) = pair {
            if c > 0.8 {
                pair_ok += 1;
            }
        }
        println!(
            "{event:16} {:6.1} {:4.1} {:5.2} {:6.1} | {:6.1} {:5.2} | {:6.1} {:5.2} | {:+3}({:4.2}/{:4.2}) | {:+3}({:4.2}/{:4.2}) | {:9} | {:.3} {:.3}",
            bh, bs, bspread, key, uh, usp, dh, dsp, su, cu, cu2, sd, cd, cd2,
            pair.map(|(p, c, _)| format!("{:+3}({:4.2})", p, c)).unwrap_or("   -".into()),
            b.len() as f32 / RATE,
            peak(&b)
        );
    }
    keys.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "\n内置 ratio=1.0 反推 NBS key（三窗一致<0.6 半音的 {} 件）：{}",
        keys.len(),
        keys.iter().map(|v| format!("{v:.1}")).collect::<Vec<_>>().join(" ")
    );
    println!("三窗不一致/无周期结构，基频不可信的 {} 件：{}", unstable.len(), unstable.join(" "));
    println!("内置→变体 命中 +24/-24 且相关>0.5：{base_ok}/20");
    println!("_-1→_1 配对命中 +48 且相关>0.8：{pair_ok}/20");
}

// 离线标定：量 assets/Sounds 里 20 个内置采样的 RMS 包络，产出 MIDI 导入「包络延音」用的常量表。
// 分桶宽度、衰减判据、曲线分类和 12 点包络窗沿用 MiauNoteBlock tools/analyze_sf2.py（commit 87b1133）的口径，
// 差别只在数据源：那边是 FluidR3_GM.sf2 的 128 个 GM program，这边是我们真正播放的 20 个乐器采样。
// 用法：cargo run -p nbs-audio --example instrument_decay_probe [-- <采样目录>]

use nbs_audio::samples::{BUILTIN_FILES, SAMPLE_RATE, decode_ogg, find_sounds_dir};
use std::path::{Path, PathBuf};

const BUCKET_MS: f64 = 20.0;
const MAX_ANALYSIS_MS: f64 = 8000.0;
const DECAY_CAP_MS: f64 = 8000.0;
const ENV_POINTS: usize = 12;
const ENV_WINDOW_MS: f64 = 4000.0;
const CURVE_FIT_MS: f64 = 2000.0;

#[derive(Debug)]
struct Envelope {
    decay_ms: f64,
    v20: f64,
    curve: &'static str,
    env: Vec<f64>,
    duration_ms: f64,
    peak_rms: f64,
}

/// np.interp 等价：在归一化包络上按毫秒位置线性取值，越界钳到端点。
fn value_at(norm: &[f64], ms: f64) -> f64 {
    let t = (ms / BUCKET_MS).clamp(0.0, (norm.len() - 1) as f64);
    let lo = t.floor() as usize;
    let hi = (lo + 1).min(norm.len() - 1);
    let frac = t - lo as f64;
    norm[lo] * (1.0 - frac) + norm[hi] * frac
}

/// 与 Python 端 `lstsq` 同解的 2 参数最小二乘：返回 RMSE。
fn rmse_fit(xs: &[f64], ys: &[f64]) -> f64 {
    let n = xs.len() as f64;
    let sx = xs.iter().sum::<f64>();
    let sy = ys.iter().sum::<f64>();
    let sxx = xs.iter().map(|v| v * v).sum::<f64>();
    let sxy = xs.iter().zip(ys).map(|(x, y)| x * y).sum::<f64>();
    let det = n * sxx - sx * sx;
    if det.abs() < 1e-12 {
        return f64::MAX;
    }
    let a = (n * sxy - sx * sy) / det;
    let b = (sxx * sy - sx * sxy) / det;
    let mse = xs
        .iter()
        .zip(ys)
        .map(|(x, y)| (y - (a * x + b)).powi(2))
        .sum::<f64>()
        / n;
    mse.sqrt()
}

/// 非延音音色用线性/指数拟合比较，判据与阈值同 Miau 脚本（线性 RMSE < 指数 RMSE × 0.8 才算 linear）。
fn classify_curve(norm: &[f64], peak_idx: usize) -> &'static str {
    let end = (peak_idx + (CURVE_FIT_MS / BUCKET_MS) as usize + 1).min(norm.len());
    let segment = &norm[peak_idx..end];
    if segment.len() < 5 {
        return "exponential";
    }
    let kept: Vec<(f64, f64)> = segment
        .iter()
        .enumerate()
        .filter(|(_, v)| **v > 0.02)
        .map(|(i, v)| (i as f64 * (BUCKET_MS / 1000.0), *v))
        .collect();
    if kept.len() < 5 {
        return "exponential";
    }
    let xs: Vec<f64> = kept.iter().map(|(t, _)| *t).collect();
    let ys: Vec<f64> = kept.iter().map(|(_, v)| *v).collect();
    let logs: Vec<f64> = ys.iter().map(|v| v.max(1e-6).ln()).collect();
    let lin = rmse_fit(&xs, &ys);
    let exp = rmse_fit(&xs, &logs);
    if lin < exp * 0.8 { "linear" } else { "exponential" }
}

fn analyze(pcm: &[f32], sr: u32) -> Option<Envelope> {
    let bucket = ((sr as f64 * BUCKET_MS / 1000.0).round() as usize).max(1);
    let n = pcm.len() / bucket;
    if n < 4 {
        return None;
    }
    let rms: Vec<f64> = (0..n)
        .map(|i| {
            let block = &pcm[i * bucket..(i + 1) * bucket];
            (block.iter().map(|s| (*s as f64) * (*s as f64)).sum::<f64>() / block.len() as f64).sqrt()
        })
        .collect();
    let peak_idx = rms
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.total_cmp(b))?
        .0;
    let peak = rms[peak_idx];
    if !peak.is_finite() || peak < 1e-5 {
        return None;
    }
    let norm: Vec<f64> = rms.iter().map(|v| v / peak).collect();
    let duration_ms = pcm.len() as f64 * 1000.0 / sr as f64;

    let decay_ms = match norm[peak_idx..].iter().position(|v| *v <= 0.1) {
        Some(offset) => offset as f64 * BUCKET_MS,
        None => MAX_ANALYSIS_MS.min(duration_ms),
    }
    .min(DECAY_CAP_MS);

    let v20 = value_at(&norm, 2000.0);
    // 我们的 .ogg 不带 SoundFont 的 loop generator，延音只按 2 秒处仍 ≥0.8 判。
    let curve = if v20 >= 0.8 {
        "sustained"
    } else {
        classify_curve(&norm, peak_idx)
    };

    let peak_time_ms = peak_idx as f64 * BUCKET_MS;
    let window_ms = (ENV_WINDOW_MS.min(duration_ms - peak_time_ms)).max(BUCKET_MS);
    let mut env: Vec<f64> = (0..ENV_POINTS)
        .map(|i| value_at(&norm, peak_time_ms + window_ms * i as f64 / (ENV_POINTS - 1) as f64))
        .collect();
    env[0] = 1.0;

    Some(Envelope { decay_ms, v20, curve, env, duration_ms, peak_rms: peak })
}

fn sounds_dir(arg: Option<&str>) -> PathBuf {
    find_sounds_dir(arg.map(Path::new)).unwrap_or_else(|| {
        panic!("找不到 assets/Sounds（要含 harp.ogg），可用位置参数指定目录")
    })
}

fn main() {
    let dir = sounds_dir(std::env::args().nth(1).as_deref());
    println!("// 采样目录：{}", dir.display());
    println!("// [乐器] 时长ms  峰值rms  decay_ms  v20  曲线  env12");
    for (index, file) in BUILTIN_FILES.iter().enumerate() {
        let pcm = match decode_ogg(&dir.join(file)) {
            Ok(pcm) => pcm,
            Err(error) => {
                println!("// {index:2} {file:<22} 解码失败：{error}");
                continue;
            }
        };
        let Some(env) = analyze(&pcm, SAMPLE_RATE) else {
            let rms = (pcm.iter().map(|s| (*s as f64) * (*s as f64)).sum::<f64>()
                / pcm.len().max(1) as f64)
                .sqrt();
            println!(
                "// {index:2} {file:<22} 无法分析：解码 {:4.0}ms（{} 样本），整体 rms {rms:.7}",
                pcm.len() as f64 * 1000.0 / SAMPLE_RATE as f64,
                pcm.len()
            );
            continue;
        };
        let points = env
            .env
            .iter()
            .map(|v| format!("{v:.4}"))
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "// {index:2} {file:<22} {:7.0} {:7.4} {:6.0} {:5.3} {:<11} [{points}]",
            env.duration_ms, env.peak_rms, env.decay_ms, env.v20, env.curve
        );
    }

    println!();
    println!("// 可直接粘贴的常量表（decay_ms 取整，env 四位小数）：");
    println!("// pub(super) const SAMPLE_ENVELOPES: [[f32; {ENV_POINTS}]; 20] = [");
    for (index, file) in BUILTIN_FILES.iter().enumerate() {
        let Ok(pcm) = decode_ogg(&dir.join(file)) else { continue };
        let Some(env) = analyze(&pcm, SAMPLE_RATE) else { continue };
        let points = env
            .env
            .iter()
            .map(|v| format!("{v:.4}"))
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "//     [{points}], // {index} {file} decay_ms={:.0} curve={}",
            env.decay_ms, env.curve
        );
    }
    println!("// ];");
}

//! 实测「实时回调路径」在这首歌上能不能跟上真实时间：
//! 用和 cpal 回调完全相同的 `render_interleaved`，按缓冲区大小分批渲染，
//! 统计 CPU 耗时 / 音频时长的比值，以及同时在发的 voice 数。
//!
//! 运行：`cargo run --release -p nbs-audio --example live_bench -- <文件.nbs> [缓冲区帧数]`

use std::time::{Duration, Instant};

use nbs_audio::{make_shared, SoundBank};

fn main() {
    let mut args: Vec<String> = Vec::new();
    for a in std::env::args() {
        args.push(a);
    }
    args.remove(0);
    if args.is_empty() {
        eprintln!("用法: cargo run --release -p nbs-audio --example live_bench -- <文件.nbs> [帧缓冲]");
        std::process::exit(2);
    }
    let path = args[0].clone();
    let buf_frames: usize = if args.len() > 1 { args[1].parse().unwrap_or(512) } else { 512 };

    let song = nbs_core::nbs::load(std::path::Path::new(&path)).expect("加载失败");
    let mut bank = SoundBank::load_auto(None);
    bank.extend_for_song(&song);
    println!(
        "{}\n  tempo {:.2} t/s  长度 {} tick（{:.2} 秒）  层 {}  音符 {}  采样库 {:?}",
        path.rsplit(['/', '\\']).next().unwrap(),
        song.meta.tempo,
        song.song_length(),
        song.song_length() as f64 / song.meta.tempo as f64,
        song.layers.len(),
        song.total_blocks,
        bank.source,
    );

    let rate = 44100usize;
    let shared = make_shared(bank, rate as u32);
    {
        let mut st = shared.lock();
        let cloned = st.bank.clone();
        st.prepare(&song, cloned);
        st.params.playing = true;
        st.params.speed = 1.0;
    }

    let total_frames = (rate as f64 * 10.0) as usize; // 10 秒音频
    let mut out = vec![0f32; buf_frames * 2];
    let mut peak_voices = 0usize;
    let mut voice_sum = 0usize;
    let mut chunks = 0usize;
    let mut worst = Duration::ZERO;
    let t0 = Instant::now();
    let mut done = 0usize;
    while done < total_frames {
        let n = if buf_frames < total_frames - done { buf_frames } else { total_frames - done };
        let c0 = Instant::now();
        {
            let mut st = shared.lock();
            st.render_interleaved(&mut out[..n * 2], 2);
            if st.mixer.active() > peak_voices {
                peak_voices = st.mixer.active();
            }
            voice_sum += st.mixer.active();
        }
        let el = c0.elapsed();
        if el > worst {
            worst = el;
        }
        chunks += 1;
        done += n;
    }
    let wall = t0.elapsed().as_secs_f64();
    let audio = total_frames as f64 / rate as f64;
    let budget = (buf_frames as f64 / rate as f64) * 1000.0;
    println!(
        "  渲染 {:.2} 秒音频耗时 {:.3} 秒 → 实时倍率 {:.2}x（<1.0 才跟得上）",
        audio, wall, wall / audio
    );
    println!(
        "  缓冲 {buf_frames} 帧 = 每块预算 {:.2} 毫秒：平均 {:.2} 毫秒，最差 {:.2} 毫秒（{chunks} 块）",
        budget,
        wall * 1000.0 / chunks as f64,
        worst.as_secs_f64() * 1000.0
    );
    println!("  voice：峰值 {peak_voices}，平均 {:.1}", voice_sum as f64 / chunks as f64);
    let pos = shared.lock().position;
    {
        let st = shared.lock();
        println!(
            "  结束状态：playing={} tempo={:.3} speed={:.3} sample_rate={} 事件 {} 条 voice 现存 {}",
            st.params.playing,
            st.params.tempo,
            st.params.speed,
            st.sample_rate,
            st.events.len(),
            st.mixer.voices.len()
        );
        let tcs: Vec<String> = st
            .events
            .iter()
            .filter(|e| e.tempo_change_bpm.is_some())
            .map(|e| format!("tick {} bpm {:?}", e.tick, e.tempo_change_bpm))
            .collect();
        println!("  事件表里的变速音符：{}", tcs.join(" | "));
    }
    println!(
        "  播放头停在 {pos:.1} tick（应为 {:.1}）→ 每秒 {:.2} tick",
        10.0 * song.meta.tempo as f64,
        pos / 10.0
    );
}

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use console::Term;
use serde::Deserialize;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::thread::sleep;
use std::time::Duration;
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// One or more file or directory paths
    #[arg(required = true)]
    paths: Vec<String>,
}

// 常见视频扩展名（小写）
const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts", "mts", "m2ts", "mpg", "mpeg",
    "3gp", "ogv", "rmvb", "vob",
];

// 已处理后缀
// const SKIPPED_SUFFIXES: &[&str] = &["_H264", "_H265", "_AV1"];
const SKIPPED_SUFFIXES: &[&str] = &["_AV1"];

#[derive(Deserialize, Debug)]
struct Stream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    r_frame_rate: Option<String>, // e.g., "25/1"
    bit_rate: Option<String>,
    tags: Option<std::collections::HashMap<String, String>>,
}

#[derive(Deserialize, Debug)]
struct FfprobeOutput {
    streams: Vec<Stream>,
}

pub enum Color {
    Red,
    Green,
    Blue,
    Yellow,
    Magenta,
    Cyan,
    Custom(u8),
}

impl Color {
    fn code(&self) -> u8 {
        match self {
            Color::Red => 31,
            Color::Green => 32,
            Color::Blue => 34,
            Color::Yellow => 33,
            Color::Magenta => 35,
            Color::Cyan => 36,
            Color::Custom(code) => *code,
        }
    }
}

pub fn color_text(text: &str, color: Color) -> String {
    format!("\x1b[{}m{}\x1b[0m", color.code(), text)
}

fn is_video_file(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .map(|ext| VIDEO_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

fn should_skip_file(path: &Path) -> bool {
    if let Some(stem) = path.file_stem().and_then(OsStr::to_str) {
        let stem_upper = stem.to_uppercase();
        for suffix in SKIPPED_SUFFIXES {
            if stem_upper.ends_with(suffix) {
                return true;
            }
        }
    }
    false
}

fn collect_video_files(paths: &[String]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for input in paths {
        let path = Path::new(input);
        if !path.exists() {
            eprintln!("Warning: path does not exist: {}", input);
            continue;
        }

        if path.is_file() {
            if is_video_file(path) && !should_skip_file(path) {
                files.push(path.to_path_buf());
            }
        } else if path.is_dir() {
            for entry in WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
                let entry_path = entry.path();
                if entry_path.is_file()
                    && is_video_file(entry_path)
                    && !should_skip_file(entry_path)
                {
                    files.push(entry_path.to_path_buf());
                }
            }
        }
    }
    Ok(files)
}

fn run_ffprobe(video_path: &Path) -> Result<FfprobeOutput> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
            video_path.to_str().ok_or_else(|| anyhow!("Invalid path"))?,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .context("Failed to run ffprobe")?;

    if !output.status.success() {
        return Err(anyhow!("ffprobe failed on {:?}", video_path));
    }

    let stdout = String::from_utf8(output.stdout)?;
    let probe: FfprobeOutput = serde_json::from_str(&stdout)?;
    Ok(probe)
}

fn parse_frame_rate(r_frame_rate: &str) -> Option<f64> {
    if let Some(pos) = r_frame_rate.find('/') {
        let num_str = &r_frame_rate[..pos];
        let den_str = &r_frame_rate[pos + 1..];
        if let (Ok(num), Ok(den)) = (f64::from_str(num_str), f64::from_str(den_str)) {
            if den != 0.0 {
                return Some(num / den);
            }
        }
    } else {
        // 可能是整数字符串，如 "30"
        if let Ok(val) = f64::from_str(r_frame_rate) {
            return Some(val);
        }
    }
    None
}

fn calculate_bpp(probe: &FfprobeOutput) -> Option<f64> {
    // 找到第一个视频流
    let video_stream = probe.streams.iter().find(|s| {
        s.codec_type.as_deref() == Some("video") && s.width.is_some() && s.height.is_some()
    })?;

    let width = video_stream.width? as f64;
    let height = video_stream.height? as f64;
    let fps = parse_frame_rate(&video_stream.r_frame_rate.as_deref()?)?;

    let bitrate: f64 = video_stream
        .bit_rate
        .as_ref()
        .and_then(|br| br.parse().ok())
        .or_else(|| {
            video_stream
                .tags
                .as_ref()
                .and_then(|tags| tags.get("BPS"))
                .and_then(|bps| bps.parse::<f64>().ok())
        })
        .unwrap_or_else(|| return 0.0);

    let bpp_value = bitrate / (width * height * fps);

    println!(
        "BPS: {:.3}Mps {}x{} {:.2}fps ==> BPP: {}",
        bitrate / 1000000.0,
        width as i64,
        height as i64,
        fps,
        color_text(
            format!("{:.2}%", bpp_value * 100.0).as_str(),
            if bpp_value < 0.1 {
                Color::Green
            } else if bpp_value < 0.15 {
                Color::Yellow
            } else {
                Color::Red
            }
        )
    );

    if width <= 0.0 || height <= 0.0 || fps <= 0.0 || bitrate <= 0.0 {
        return None;
    }

    Some(bpp_value)
}

fn get_codec_name(probe: &FfprobeOutput) -> Option<String> {
    probe
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"))
        .and_then(|s| s.codec_name.clone())
}

fn rename_with_suffix(original: &Path, suffix: &str) -> Result<()> {
    let parent = original.parent().unwrap_or_else(|| Path::new("."));
    let stem = original
        .file_stem()
        .and_then(OsStr::to_str)
        .ok_or_else(|| anyhow!("Invalid filename"))?;
    let extension = original.extension().and_then(OsStr::to_str).unwrap_or("");

    let new_name = if extension.is_empty() {
        format!("{}{}", stem, suffix)
    } else {
        format!("{}{}.{}", stem, suffix, extension)
    };

    let new_path = parent.join(new_name);

    if new_path.exists() {
        return Err(anyhow!("Target file already exists: {:?}", new_path));
    }

    fs::rename(original, &new_path)
        .with_context(|| format!("Failed to rename {:?} to {:?}", original, new_path))?;
    println!("Renamed: {} -> {}", original.display(), new_path.display());
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!(concat!(
            "请提供至少一个视频文件或文件夹路径作为参数\n\n",
            "本软件用于给视频批量计算BPP值，根据数值情况决定是否进行转码，BPP大于15%则可以考虑转码到H265/AV1以节约存储空间\n\n",
            "请把视频文件或文件夹拖到本软件图标上即可，支持多个一起拖拽\n\n",
            "本软件依赖 ffmpeg，需确保 ffmpeg.exe 位于本程序同一目录下，或者将其所在文件夹添加到系统环境变量中\n\n",
            "ffmpeg.exe 下载地址: https://www.gyan.dev/ffmpeg/builds"
        ));
        sleep(Duration::from_secs(600)); // 10分钟后自动关闭
        std::process::exit(1);
    }

    let args = Args::parse();

    let mut video_files: Vec<PathBuf> = collect_video_files(&args.paths)?;
    if video_files.is_empty() {
        std::process::exit(1);
    }

    video_files.sort_by(|a, b| {
        let a_str = a.to_string_lossy();
        let b_str = b.to_string_lossy();
        natural_sort_rs::natural_cmp(&a_str, &b_str)
    });

    for file in video_files {
        println!("Processing: {}", file.display());

        let probe = match run_ffprobe(&file) {
            Ok(probe) => probe,
            Err(_) => {
                eprintln!("Failed to process {}", file.display());
                continue;
            }
        };

        let codec = get_codec_name(&probe)
            .unwrap_or_else(|| "unknown".to_string())
            .to_uppercase();

        if codec.to_uppercase() == "AV1" {
            rename_with_suffix(&file, "_AV1")?; // 重命名为 _AV1
            continue;
        }

        // 计算 BPP
        let _ = calculate_bpp(&probe);
    }

    // 程序结束，随便播放一个提示声
    print!("\x07");

    println!("按任意键退出...");
    Term::stdout().read_key().unwrap();

    Ok(())
}

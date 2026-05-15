use std::{
    env,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use chrono::Utc;
use reqwest::Client;
use serde::Serialize;
use tokio::{net::TcpStream, task::JoinSet};

const DOWNLOAD_URL: &str = "https://speed.cloudflare.com/__down?bytes=524288000";
const UPLOAD_URL: &str = "https://speed.cloudflare.com/__up";

#[derive(Debug)]
struct CliArgs {
    threads: usize,
    upload_threads: usize,
    ping_count: usize,
    download_seconds: u64,
    upload_seconds: u64,
}

#[derive(Serialize)]
struct SpeedTestResult {
    id: i64,
    ping: f64,
    download_speed: f64,
    upload_speed: f64,
    source: String,
    threads: usize,
    upload_threads: usize,
    timestamp: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    let client = Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?;

    eprintln!(
        "开始测速: ping={}次, 下载线程={}, 上传线程={}",
        args.ping_count, args.threads, args.upload_threads
    );

    let ping = tcp_ping_avg("speed.cloudflare.com", 443, args.ping_count)
        .await
        .unwrap_or(0.0)
        .max(1.0);

    let download_speed = run_download_test(
        Arc::new(client.clone()),
        args.threads,
        Duration::from_secs(args.download_seconds),
    )
    .await;

    let upload_speed = run_upload_test(
        Arc::new(client),
        args.upload_threads,
        Duration::from_secs(args.upload_seconds),
    )
    .await;

    let result = SpeedTestResult {
        id: Utc::now().timestamp_millis(),
        ping,
        download_speed,
        upload_speed,
        source: "cloudflare".to_string(),
        threads: args.threads,
        upload_threads: args.upload_threads,
        timestamp: Utc::now().to_rfc3339(),
    };

    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn parse_args() -> anyhow::Result<CliArgs> {
    let mut args = CliArgs {
        threads: 4,
        upload_threads: 2,
        ping_count: 3,
        download_seconds: 15,
        upload_seconds: 10,
    };

    let mut iter = env::args().skip(1);
    while let Some(flag) = iter.next() {
        let value = match flag.as_str() {
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            "--threads" | "--upload-threads" | "--ping-count" | "--download-seconds"
            | "--upload-seconds" => iter
                .next()
                .ok_or_else(|| anyhow::anyhow!("{} 缺少参数值", flag))?,
            _ => return Err(anyhow::anyhow!("未知参数: {}", flag)),
        };

        match flag.as_str() {
            "--threads" => args.threads = value.parse::<usize>()?.max(1),
            "--upload-threads" => args.upload_threads = value.parse::<usize>()?.max(1),
            "--ping-count" => args.ping_count = value.parse::<usize>()?.max(1),
            "--download-seconds" => args.download_seconds = value.parse::<u64>()?.max(1),
            "--upload-seconds" => args.upload_seconds = value.parse::<u64>()?.max(1),
            _ => {}
        }
    }

    Ok(args)
}

fn print_help() {
    println!(
        "用法:\n  ./pbox-speedtest-backend [参数]\n\n参数:\n  --threads <n>            下载并发线程数，默认 4\n  --upload-threads <n>     上传并发线程数，默认 2\n  --ping-count <n>         ping 次数，默认 3\n  --download-seconds <n>   下载测速时长（秒），默认 15\n  --upload-seconds <n>     上传测速时长（秒），默认 10\n  -h, --help               显示帮助"
    );
}

async fn tcp_ping_avg(host: &str, port: u16, count: usize) -> anyhow::Result<f64> {
    let mut values = vec![];
    for _ in 0..count {
        let start = Instant::now();
        if TcpStream::connect((host, port)).await.is_ok() {
            values.push(start.elapsed().as_secs_f64() * 1000.0);
        }
    }
    anyhow::ensure!(!values.is_empty(), "all ping failed");
    Ok(values.iter().sum::<f64>() / values.len() as f64)
}

async fn run_download_test(client: Arc<Client>, threads: usize, max_dur: Duration) -> f64 {
    let total = Arc::new(AtomicU64::new(0));
    let stop_at = Instant::now() + max_dur;
    let mut set = JoinSet::new();
    for _ in 0..threads {
        let client = client.clone();
        let total = total.clone();
        set.spawn(async move {
            while Instant::now() < stop_at {
                if let Ok(resp) = client.get(DOWNLOAD_URL).send().await {
                    if let Ok(bytes) = resp.bytes().await {
                        total.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                    }
                }
            }
        });
    }
    while set.join_next().await.is_some() {}
    total.load(Ordering::Relaxed) as f64 * 8.0 / max_dur.as_secs_f64() / 1e6
}

async fn run_upload_test(client: Arc<Client>, threads: usize, max_dur: Duration) -> f64 {
    let total = Arc::new(AtomicU64::new(0));
    let stop_at = Instant::now() + max_dur;
    let mut set = JoinSet::new();
    for _ in 0..threads {
        let client = client.clone();
        let total = total.clone();
        set.spawn(async move {
            let chunk = vec![0u8; 10 * 1024 * 1024];
            while Instant::now() < stop_at {
                let body = chunk.clone();
                if client.post(UPLOAD_URL).body(body).send().await.is_ok() {
                    total.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                }
            }
        });
    }
    while set.join_next().await.is_some() {}
    total.load(Ordering::Relaxed) as f64 * 8.0 / max_dur.as_secs_f64() / 1e6
}

use anyhow::{anyhow, Context, Result};
use clap::{Parser, ValueEnum};
use futures::stream::{self, StreamExt};
use reqwest::Client;
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::str::FromStr;
use std::time::{Duration, Instant};
use tokio::io::AsyncBufReadExt;
use tokio::net::TcpStream;
use tokio::time::timeout;

const TEST_HOST: &str = "speed.cloudflare.com";
const TEST_PATH: &str = "/__down?bytes=5000000";

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum IpSource {
    Auto,
    File,
    Stdin,
    Inline,
    Url,
}

#[derive(Parser, Debug)]
#[command(author, version, about = "Cloudflare 优选 IP 测试工具")]
struct Args {
    /// IP 来源类型（auto/file/stdin/inline/url）
    #[arg(long, value_enum, default_value_t = IpSource::Auto)]
    source: IpSource,

    /// IP 文件路径（每行一个 IP）
    #[arg(short, long)]
    input: Option<String>,

    /// 直接提供 IP 列表，逗号分隔（例如: 1.1.1.1,1.0.0.1）
    #[arg(long)]
    ips: Option<String>,

    /// 从 URL 获取 IP 列表（文本每行一个 IP）
    #[arg(long)]
    url: Option<String>,

    /// 测试端口
    #[arg(short, long, default_value_t = 443)]
    port: u16,

    /// 并发测试数量
    #[arg(short, long, default_value_t = 20)]
    concurrency: usize,

    /// 连接超时（毫秒）
    #[arg(long, default_value_t = 1200)]
    connect_timeout_ms: u64,

    /// 测速时长上限（秒）
    #[arg(long, default_value_t = 4)]
    speed_test_seconds: u64,

    /// CIDR 展开最大 IP 数（防止一次展开过大网段）
    #[arg(long, default_value_t = 4096)]
    max_cidr_expand: usize,

    /// 输出数量
    #[arg(short, long, default_value_t = 10)]
    top: usize,
}

#[derive(Debug, Clone)]
struct IpResult {
    ip: IpAddr,
    latency_ms: f64,
    speed_mbps: f64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let ips = read_ips(&args).await?;

    if ips.is_empty() {
        anyhow::bail!("没有读取到 IP，请通过 --input / --ips / --url / STDIN 提供 IP 列表");
    }

    println!("开始测试 {} 个 IP，端口 {} ...", ips.len(), args.port);

    let connect_timeout = Duration::from_millis(args.connect_timeout_ms);
    let speed_test_time = Duration::from_secs(args.speed_test_seconds);

    let results: Vec<IpResult> = stream::iter(ips.into_iter())
        .map(|ip| async move { test_ip(ip, args.port, connect_timeout, speed_test_time).await })
        .buffer_unordered(args.concurrency)
        .filter_map(|res| async move {
            match res {
                Ok(v) => Some(v),
                Err(e) => {
                    eprintln!("[跳过] {}", e);
                    None
                }
            }
        })
        .collect()
        .await;

    if results.is_empty() {
        anyhow::bail!("所有 IP 测试失败");
    }

    let mut sorted = results;
    sorted.sort_by(rank);

    println!("\n=== 最优 {} 个 IP ===", args.top.min(sorted.len()));
    println!(
        "{:<4} {:<40} {:>12} {:>14}",
        "#", "IP", "延迟(ms)", "速度(Mbps)"
    );

    for (idx, r) in sorted.iter().take(args.top).enumerate() {
        println!(
            "{:<4} {:<40} {:>12.2} {:>14.2}",
            idx + 1,
            r.ip,
            r.latency_ms,
            r.speed_mbps
        );
    }

    Ok(())
}

fn rank(a: &IpResult, b: &IpResult) -> Ordering {
    a.latency_ms
        .partial_cmp(&b.latency_ms)
        .unwrap_or(Ordering::Equal)
        .then_with(|| {
            b.speed_mbps
                .partial_cmp(&a.speed_mbps)
                .unwrap_or(Ordering::Equal)
        })
}

async fn read_ips(args: &Args) -> Result<Vec<IpAddr>> {
    let text = match args.source {
        IpSource::File => {
            let path = args
                .input
                .as_deref()
                .ok_or_else(|| anyhow!("source=file 时必须提供 --input"))?;
            tokio::fs::read_to_string(path)
                .await
                .with_context(|| format!("读取文件失败: {path}"))?
        }
        IpSource::Inline => args
            .ips
            .clone()
            .ok_or_else(|| anyhow!("source=inline 时必须提供 --ips"))?,
        IpSource::Url => {
            let url = args
                .url
                .as_deref()
                .ok_or_else(|| anyhow!("source=url 时必须提供 --url"))?;
            reqwest::get(url)
                .await
                .with_context(|| format!("拉取 URL 失败: {url}"))?
                .text()
                .await
                .context("读取 URL 内容失败")?
        }
        IpSource::Stdin => read_stdin().await?,
        IpSource::Auto => {
            if let Some(path) = args.input.as_deref() {
                tokio::fs::read_to_string(path)
                    .await
                    .with_context(|| format!("读取文件失败: {path}"))?
            } else if let Some(ips) = args.ips.clone() {
                ips
            } else if let Some(url) = args.url.as_deref() {
                reqwest::get(url)
                    .await
                    .with_context(|| format!("拉取 URL 失败: {url}"))?
                    .text()
                    .await
                    .context("读取 URL 内容失败")?
            } else {
                read_stdin().await?
            }
        }
    };

    let segments: Vec<String> = if matches!(args.source, IpSource::Inline)
        || args.ips.is_some() && args.input.is_none() && args.url.is_none()
    {
        text.split(',').map(|s| s.trim().to_string()).collect()
    } else {
        text.lines().map(|s| s.trim().to_string()).collect()
    };

    let mut out = BTreeSet::new();
    for s in segments {
        if s.is_empty() || s.starts_with('#') {
            continue;
        }

        if s.contains('/') {
            match expand_cidr(&s, args.max_cidr_expand) {
                Ok(ips) => {
                    for ip in ips {
                        out.insert(ip);
                    }
                }
                Err(e) => eprintln!("[忽略] CIDR 无效或过大: {s} ({e})"),
            }
            continue;
        }

        match IpAddr::from_str(&s) {
            Ok(ip) => {
                out.insert(ip);
            }
            Err(_) => eprintln!("[忽略] 非法 IP: {s}"),
        }
    }
    Ok(out.into_iter().collect())
}

fn expand_cidr(cidr: &str, max_expand: usize) -> Result<Vec<IpAddr>> {
    let (ip_raw, prefix_raw) = cidr
        .split_once('/')
        .ok_or_else(|| anyhow!("CIDR 格式错误"))?;
    let base_ip =
        IpAddr::from_str(ip_raw.trim()).with_context(|| format!("CIDR IP 无效: {ip_raw}"))?;
    let prefix: u8 = prefix_raw
        .trim()
        .parse()
        .with_context(|| format!("CIDR 前缀无效: {prefix_raw}"))?;

    match base_ip {
        IpAddr::V4(v4) => expand_cidr_v4(v4, prefix, max_expand),
        IpAddr::V6(v6) => expand_cidr_v6(v6, prefix, max_expand),
    }
}

fn expand_cidr_v4(base: Ipv4Addr, prefix: u8, max_expand: usize) -> Result<Vec<IpAddr>> {
    if prefix > 32 {
        anyhow::bail!("IPv4 前缀必须在 0..=32");
    }
    let host_bits = 32 - prefix as u32;
    let count: u128 = 1u128 << host_bits;
    if count > max_expand as u128 {
        anyhow::bail!("CIDR 包含 {count} 个 IP，超过 --max-cidr-expand={max_expand}");
    }

    let base_u32 = u32::from(base);
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << host_bits
    };
    let network = base_u32 & mask;

    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count as u32 {
        out.push(IpAddr::V4(Ipv4Addr::from(network.wrapping_add(i))));
    }
    Ok(out)
}

fn expand_cidr_v6(base: Ipv6Addr, prefix: u8, max_expand: usize) -> Result<Vec<IpAddr>> {
    if prefix > 128 {
        anyhow::bail!("IPv6 前缀必须在 0..=128");
    }
    let host_bits = 128 - prefix as u32;
    if host_bits >= 128 {
        anyhow::bail!("CIDR 范围过大");
    }
    let count: u128 = 1u128 << host_bits;
    if count > max_expand as u128 {
        anyhow::bail!("CIDR 包含 {count} 个 IP，超过 --max-cidr-expand={max_expand}");
    }

    let base_u128 = u128::from(base);
    let mask = if prefix == 0 {
        0
    } else {
        u128::MAX << host_bits
    };
    let network = base_u128 & mask;

    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        out.push(IpAddr::V6(Ipv6Addr::from(network.wrapping_add(i))));
    }
    Ok(out)
}
async fn read_stdin() -> Result<String> {
    let mut lines = Vec::new();
    let mut reader = tokio::io::BufReader::new(tokio::io::stdin());
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }
        lines.push(line.trim_end().to_string());
    }
    Ok(lines.join("\n"))
}

async fn test_ip(
    ip: IpAddr,
    port: u16,
    connect_timeout: Duration,
    speed_test_time: Duration,
) -> Result<IpResult> {
    let latency_ms = test_latency(ip, port, connect_timeout)
        .await
        .with_context(|| format!("{}:{} 延迟测试失败", ip, port))?;

    let speed_mbps = test_speed(ip, port, speed_test_time)
        .await
        .with_context(|| format!("{}:{} 测速失败", ip, port))?;

    Ok(IpResult {
        ip,
        latency_ms,
        speed_mbps,
    })
}

async fn test_latency(ip: IpAddr, port: u16, connect_timeout: Duration) -> Result<f64> {
    let addr = SocketAddr::new(ip, port);
    let start = Instant::now();
    let stream = timeout(connect_timeout, TcpStream::connect(addr)).await??;
    drop(stream);
    Ok(start.elapsed().as_secs_f64() * 1000.0)
}

async fn test_speed(ip: IpAddr, port: u16, speed_test_time: Duration) -> Result<f64> {
    let client = Client::builder()
        .resolve(TEST_HOST, SocketAddr::new(ip, port))
        .danger_accept_invalid_certs(false)
        .build()?;

    let url = format!("https://{TEST_HOST}{TEST_PATH}");

    let started = Instant::now();
    let mut resp = timeout(speed_test_time, client.get(url).send()).await??;

    let mut bytes: usize = 0;
    loop {
        let remaining = speed_test_time.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            break;
        }
        match timeout(remaining, resp.chunk()).await {
            Ok(Ok(Some(chunk))) => bytes += chunk.len(),
            Ok(Ok(None)) => break,
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => break,
        }
    }

    let secs = started.elapsed().as_secs_f64().max(0.001);
    let mbps = (bytes as f64 * 8.0) / secs / 1_000_000.0;
    Ok(mbps)
}

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::{Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::{
    io::AsyncWriteExt,
    net::TcpStream,
    sync::atomic::{AtomicU64, Ordering},
    task::JoinSet,
};

#[derive(Clone)]
struct AppState {
    client: Client,
}

#[derive(Serialize)]
struct SpeedTestSource {
    id: &'static str,
    name: &'static str,
    url: &'static str,
    upload_url: &'static str,
    size: u64,
}

#[derive(Deserialize)]
struct SpeedTestRequest {
    source: Option<String>,
    threads: Option<usize>,
    upload_threads: Option<usize>,
}

#[derive(Serialize)]
struct SpeedTestResult {
    id: i64,
    ping: f64,
    download_speed: f64,
    upload_speed: f64,
    source: String,
    threads: usize,
    timestamp: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let client = Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .expect("client");
    let state = AppState { client };
    let app = Router::new()
        .route("/api/speedtest/sources", get(get_sources))
        .route("/api/speedtest/start", post(start_speedtest))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    tracing::info!("rust backend listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
}

async fn get_sources() -> Json<Vec<SpeedTestSource>> {
    Json(vec![SpeedTestSource {
        id: "cloudflare",
        name: "Cloudflare CDN",
        url: "https://speed.cloudflare.com/__down?bytes=524288000",
        upload_url: "https://speed.cloudflare.com/__up",
        size: 500 * 1024 * 1024,
    }])
}

async fn start_speedtest(
    State(state): State<AppState>,
    Query(req): Query<SpeedTestRequest>,
) -> Json<SpeedTestResult> {
    let threads = req.threads.unwrap_or(4).max(1);
    let upload_threads = req.upload_threads.unwrap_or(2).max(1);
    let source = req.source.unwrap_or_else(|| "cloudflare".to_string());

    let ping = tcp_ping_avg("speed.cloudflare.com", 443, 3)
        .await
        .unwrap_or(0.0)
        .max(1.0);
    let download_speed = run_download_test(
        Arc::new(state.client.clone()),
        threads,
        Duration::from_secs(15),
    )
    .await;
    let upload_speed = run_upload_test(
        Arc::new(state.client),
        upload_threads,
        Duration::from_secs(10),
    )
    .await;

    Json(SpeedTestResult {
        id: Utc::now().timestamp_millis(),
        ping,
        download_speed,
        upload_speed,
        source,
        threads,
        timestamp: Utc::now().to_rfc3339(),
    })
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
                if let Ok(resp) = client
                    .get("https://speed.cloudflare.com/__down?bytes=524288000")
                    .send()
                    .await
                {
                    if let Ok(bytes) = resp.bytes().await {
                        total.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                    }
                }
            }
        });
    }
    while set.join_next().await.is_some() {}
    let secs = max_dur.as_secs_f64();
    total.load(Ordering::Relaxed) as f64 * 8.0 / secs / 1e6
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
                if client
                    .post("https://speed.cloudflare.com/__up")
                    .body(body)
                    .send()
                    .await
                    .is_ok()
                {
                    total.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                }
            }
        });
    }
    while set.join_next().await.is_some() {}
    let secs = max_dur.as_secs_f64();
    total.load(Ordering::Relaxed) as f64 * 8.0 / secs / 1e6
}

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use rand::Rng;
use serde::Deserialize;
use tokio::{io::AsyncWriteExt, task::JoinSet};

const BASE_URL: &str = "https://files.catbox.moe/";
const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
const FILENAME_LEN: usize = 6;
const DATA_DIR: &str = "data";

#[derive(Deserialize)]
struct Config {
    file_extensions: Vec<String>,
    threads: usize,
    #[serde(default = "default_update_rate")]
    update_rate: f64,
}

fn default_update_rate() -> f64 {
    0.5
}

struct Stats {
    checked: AtomicU64,
    hits: AtomicU64,
    errors: AtomicU64,
    is_running: AtomicBool,
    started_at: Instant,
}

fn random_filename(ext: &str) -> String {
    let mut rng = rand::rng();
    let name: String = (0..FILENAME_LEN)
        .map(|_| CHARSET[rng.random_range(0..CHARSET.len())] as char)
        .collect();
    format!("{name}{ext}")
}

fn format_elapsed(secs: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

fn dir_for_ext(ext: &str) -> std::path::PathBuf {
    Path::new(DATA_DIR).join(ext.trim_start_matches('.'))
}

async fn load_config(path: &str) -> Config {
    let content = tokio::fs::read_to_string(path)
        .await
        .unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
    toml::from_str(&content).unwrap_or_else(|e| panic!("bad config: {e}"))
}

async fn save_hit(url: &str, bytes: &[u8], dir: &Path, filename: &str) -> std::io::Result<()> {
    let file_path = dir.join(filename);
    if file_path.exists() {
        return Ok(());
    }
    tokio::fs::create_dir_all(dir).await?;
    tokio::fs::write(&file_path, bytes).await?;

    let mut log = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("valids.txt"))
        .await?;
    log.write_all(format!("{url}\n").as_bytes()).await?;
    Ok(())
}

fn resolve_proxy_url() -> Option<String> {
    let host = std::env::var("PROXY_HOST").ok()?;
    let port = std::env::var("PROXY_PORT").ok()?;
    let url = match (std::env::var("PROXY_USER"), std::env::var("PROXY_PASS")) {
        (Ok(user), Ok(pass)) => format!("http://{user}:{pass}@{host}:{port}"),
        _ => format!("http://{host}:{port}"),
    };
    eprintln!("using proxy {host}:{port}");
    Some(url)
}

fn build_worker_client(proxy_url: &Option<String>) -> reqwest::Client {
    let mut builder = reqwest::Client::builder()
        .pool_max_idle_per_host(1)
        .pool_idle_timeout(Duration::from_secs(90))
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(10))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36");

    if let Some(url) = proxy_url {
        builder = builder.proxy(reqwest::Proxy::all(url).expect("invalid proxy url"));
    }

    builder.build().expect("failed to build worker client")
}

async fn worker(client: reqwest::Client, extensions: Arc<Vec<String>>, stats: Arc<Stats>) {
    while stats.is_running.load(Ordering::Relaxed) {
        for ext in extensions.iter() {
            if !stats.is_running.load(Ordering::Relaxed) {
                return;
            }

            let filename = random_filename(ext);
            let url = format!("{BASE_URL}{filename}");
            let result =
                tokio::time::timeout(Duration::from_secs(15), client.get(&url).send()).await;

            stats.checked.fetch_add(1, Ordering::Relaxed);

            match result {
                Ok(Ok(resp)) if resp.status() == reqwest::StatusCode::OK => {
                    if let Ok(bytes) = resp.bytes().await {
                        if !bytes.is_empty() {
                            stats.hits.fetch_add(1, Ordering::Relaxed);
                            let dir = dir_for_ext(ext);
                            if let Err(e) = save_hit(&url, &bytes, &dir, &filename).await {
                                eprintln!("save failed {url}: {e}");
                            }
                        }
                    }
                }
                Ok(Ok(_)) => {}
                _ => {
                    stats.errors.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

async fn status_display(stats: Arc<Stats>, update_rate: f64) {
    let dur = Duration::from_secs_f64(update_rate.max(0.1));
    let mut interval = tokio::time::interval(dur);

    print!("\x1b[2J\x1b[H\x1b[?25l");
    println!(" CATBOX SCRAPER");
    println!("[==============]\n");

    while stats.is_running.load(Ordering::Relaxed) {
        interval.tick().await;

        let checked = stats.checked.load(Ordering::Relaxed);
        let hits = stats.hits.load(Ordering::Relaxed);
        let errors = stats.errors.load(Ordering::Relaxed);
        let elapsed = stats.started_at.elapsed().as_secs();
        let per_sec = if elapsed > 0 { checked / elapsed } else { 0 };

        print!("\x1b[4;1H");
        println!("[-----------------------]");
        println!(" PER SECOND   : {per_sec:<12}");
        println!(" TIME ELAPSED : {:<12}", format_elapsed(elapsed));
        println!(" CHECKS       : {checked:<12}");
        println!(" HITS         : {hits:<12}");
        println!(" ERRORS       : {errors:<12}");
        println!("[-----------------------]");
    }
}

#[tokio::main]
async fn main() {
    let config = load_config("config.toml").await;
    let proxy_url = resolve_proxy_url();

    if proxy_url.is_none() {
        eprintln!("no proxy configured, running direct");
    }

    let stats = Arc::new(Stats {
        checked: AtomicU64::new(0),
        hits: AtomicU64::new(0),
        errors: AtomicU64::new(0),
        is_running: AtomicBool::new(true),
        started_at: Instant::now(),
    });

    let extensions = Arc::new(config.file_extensions);

    let stats_clone = Arc::clone(&stats);
    tokio::spawn(status_display(stats_clone, config.update_rate));

    let mut tasks = JoinSet::new();
    for _ in 0..config.threads {
        let client = build_worker_client(&proxy_url);
        let e = Arc::clone(&extensions);
        let s = Arc::clone(&stats);
        tasks.spawn(worker(client, e, s));
    }

    let stats_shutdown = Arc::clone(&stats);
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        stats_shutdown.is_running.store(false, Ordering::Relaxed);
        print!("\x1b[?25h");
        eprintln!("\nshutting down...");
    });

    while tasks.join_next().await.is_some() {}
    print!("\x1b[?25h");
}

use anyhow::{Context, Result, anyhow, bail};
use chrono::{FixedOffset, NaiveDate, TimeZone};
use encoding_rs::{Encoding, SHIFT_JIS, UTF_8};
use rand::Rng;
use reqwest::blocking::{Client, Response, multipart};
use reqwest::header::ACCEPT;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use sysinfo::{ProcessRefreshKind, System};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub epgstation_upload: UploadConfig,
    pub epgstation_encode: EncodeConfig,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadConfig {
    pub epgstation_url: String,
    pub parent_directory_name: String,
    pub view_name: String,
    pub file_type: String,
    pub rec_details_program_folder: Option<String>,
    pub text_encoding: String,
    #[serde(rename = "deleteEDCBRecFile")]
    pub delete_edcb_rec_file: bool,
    pub delay_delete_time: u64,
    pub wait_recorded_process: bool,
    pub wait_time_interval: u64,
    pub wait_time_random_margin: u64,
    pub cpu_usage_lower_limit: i32,
    #[serde(default = "default_mode")]
    pub upload_mode: String,
    #[serde(default, rename = "limitRateMBps")]
    pub limit_rate_mbps: f64,
    #[serde(default = "default_retries")]
    pub upload_max_retry: usize,
    #[serde(default = "default_wait")]
    pub max_wait_minutes: u64,
    #[serde(default = "default_log_level")]
    pub log_level: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncodeConfig {
    pub run_encode: bool,
    pub parent_dir: Option<String>,
    pub directory: Option<String>,
    pub is_save_same_directory: bool,
    pub mode: String,
    pub remove_original: bool,
}
fn default_mode() -> String {
    "upload".into()
}
fn default_retries() -> usize {
    5
}
fn default_wait() -> u64 {
    720
}
fn default_log_level() -> String {
    "info".into()
}

#[derive(Clone)]
pub struct Logger {
    path: PathBuf,
    debug: bool,
}
impl Logger {
    pub fn new(path: PathBuf, debug: bool) -> Self {
        if fs::metadata(&path)
            .map(|m| m.len() > 5 * 1024 * 1024)
            .unwrap_or(false)
        {
            rotate(&path);
        }
        Self { path, debug }
    }
    fn log(&self, level: &str, msg: &str) {
        if level == "DEBUG" && !self.debug {
            return;
        }
        let line = format!(
            "{} [{}] [{}] {}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
            std::process::id(),
            level,
            msg.replace('\r', "\\r").replace('\n', "\\n")
        );
        let _ = io::stderr().write_all(line.as_bytes());
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = f.write_all(line.as_bytes());
        }
    }
    fn info(&self, s: &str) {
        self.log("INFO", s)
    }
    fn warn(&self, s: &str) {
        self.log("WARN", s)
    }
    fn error(&self, s: &str) {
        self.log("ERROR", s)
    }
    fn debug(&self, s: &str) {
        self.log("DEBUG", s)
    }
}
fn rotate(path: &Path) {
    let oldest = PathBuf::from(format!("{}.3", path.display()));
    let _ = fs::remove_file(oldest);
    for n in (1..=2).rev() {
        let from = PathBuf::from(format!("{}.{}", path.display(), n));
        let to = PathBuf::from(format!("{}.{}", path.display(), n + 1));
        let _ = fs::rename(from, to);
    }
    let _ = fs::rename(path, PathBuf::from(format!("{}.1", path.display())));
}

#[derive(Debug)]
pub struct Recording {
    pub filepath: PathBuf,
    pub filename: String,
    pub title: String,
    pub onid: String,
    pub sid: String,
    pub start: i64,
    pub end: i64,
}
const ENV_KEYS: &[&str] = &[
    "FILEPATH", "FILENAME", "TITLE", "ONID10", "SID10", "SDYYYY", "SDMM", "SDDD", "STHH", "STMM",
    "STSS", "EDYYYY", "EDMM", "EDDD", "ETHH", "ETMM", "ETSS",
];
pub fn recording_from<I: Fn(&str) -> Option<String>>(get: I) -> Result<Recording> {
    let missing: Vec<_> = ENV_KEYS
        .iter()
        .filter(|k| get(k).is_none())
        .copied()
        .collect();
    if !missing.is_empty() {
        bail!("必須環境変数がありません: {}", missing.join(", "));
    }
    let val = |k: &str| get(k).ok_or_else(|| anyhow!("環境変数 {k} がありません"));
    let part = |y: &str, mo: &str, d: &str, h: &str, mi: &str, s: &str| -> Result<i64> {
        let date = NaiveDate::from_ymd_opt(val(y)?.parse()?, val(mo)?.parse()?, val(d)?.parse()?)
            .ok_or_else(|| anyhow!("日時が不正"))?;
        let dt = date
            .and_hms_opt(val(h)?.parse()?, val(mi)?.parse()?, val(s)?.parse()?)
            .ok_or_else(|| anyhow!("日時が不正"))?;
        Ok(FixedOffset::east_opt(9 * 3600)
            .ok_or_else(|| anyhow!("JST offset"))?
            .from_local_datetime(&dt)
            .single()
            .ok_or_else(|| anyhow!("日時が不正"))?
            .timestamp_millis())
    };
    let sid = format!("{:0>5}", val("SID10")?);
    Ok(Recording {
        filepath: val("FILEPATH")?.into(),
        filename: val("FILENAME")?,
        #[allow(clippy::no_effect_replace)]
        title: val("TITLE")?
            .replace('\u{200b}', " ")
            .replace('\u{3000}', "　"),
        onid: val("ONID10")?,
        sid,
        start: part("SDYYYY", "SDMM", "SDDD", "STHH", "STMM", "STSS")?,
        end: part("EDYYYY", "EDMM", "EDDD", "ETHH", "ETMM", "ETSS")?,
    })
}
pub fn channel_id(onid: &str, sid: &str) -> Result<i64> {
    Ok(format!("{onid}{sid}").parse()?)
}
pub fn paths(r: &Recording, folder: Option<&str>) -> (PathBuf, PathBuf) {
    match folder.filter(|s| s.chars().count() >= 3) {
        Some(folder) => {
            let dir = PathBuf::from(folder);
            (
                dir.join(format!("{}.ts.program.txt", r.filename)),
                dir.join(format!("{}.ts.err", r.filename)),
            )
        }
        None => (
            PathBuf::from(format!("{}.program.txt", r.filepath.display())),
            PathBuf::from(format!("{}.err", r.filepath.display())),
        ),
    }
}
pub fn decode(bytes: &[u8], encoding: &str) -> Result<String> {
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return String::from_utf8(bytes[3..].to_vec()).context("UTF-8 BOM decode");
    }
    let norm = encoding.to_ascii_lowercase().replace(['-', '_'], "");
    let enc: &'static Encoding = match norm.as_str() {
        "utf8" => UTF_8,
        "shiftjis" | "sjis" | "cp932" | "windows31j" => SHIFT_JIS,
        _ => bail!("未対応の textEncoding: {encoding}"),
    };
    let (text, _, had_errors) = enc.decode(bytes);
    if had_errors {
        bail!("文字コード変換に失敗: {encoding}");
    }
    Ok(text.into_owned())
}
pub fn parse_program(prog: &str, drop: &str) -> Result<(String, String)> {
    let normalized = prog.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<_> = normalized.split('\n').collect();
    let desc = lines
        .get(4)
        .ok_or_else(|| anyhow!("description: index 4 の行がありません"))?
        .to_string();
    let delimiters = ["詳細情報\n", "ジャンル : \n"];
    let first = delimiters
        .iter()
        .filter_map(|d| normalized.find(d).map(|i| (i, d.len())))
        .min_by_key(|(i, _)| *i)
        .ok_or_else(|| anyhow!("extended: 詳細情報 / ジャンル : 区切りがありません"))?;
    let tail = &normalized[first.0 + first.1..];
    let end = delimiters
        .iter()
        .filter_map(|d| tail.find(d))
        .min()
        .unwrap_or(tail.len());
    let ext = &tail[..end];
    Ok((
        desc,
        format!(
            "{}\n{}",
            ext,
            drop.replace("\r\n", "\n").replace('\r', "\n")
        ),
    ))
}
pub fn read_program(r: &Recording, cfg: &UploadConfig, logger: &Logger) -> (String, String) {
    let (p, d) = paths(r, cfg.rec_details_program_folder.as_deref());
    let result = (|| -> Result<_> {
        let mut b = Vec::new();
        File::open(&p)
            .with_context(|| format!("番組情報ファイルを開けません: {}", p.display()))?
            .read_to_end(&mut b)?;
        let prog = decode(&b, &cfg.text_encoding)?;
        let mut db = Vec::new();
        let drop = match File::open(&d) {
            Ok(mut f) => {
                f.read_to_end(&mut db)?;
                decode(&db, &cfg.text_encoding)?
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.into()),
        };
        parse_program(&prog, &drop)
    })();
    result.unwrap_or_else(|e| {
        logger.warn(&format!(
            "番組情報 {} / ドロップログ {} を解析できません: {e:#}",
            p.display(),
            d.display()
        ));
        (String::new(), String::new())
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecordedBody<'a> {
    channel_id: i64,
    start_at: i64,
    end_at: i64,
    name: &'a str,
    description: &'a str,
    extended: &'a str,
}
fn response_json(mut r: Response) -> Result<serde_json::Value> {
    let status = r.status();
    let mut s = String::new();
    r.read_to_string(&mut s).context("応答読み込み失敗")?;
    if !status.is_success() {
        bail!("HTTP {status}: {s}")
    }
    if s.is_empty() {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_str(&s).with_context(|| format!("応答JSONが不正: {s}"))
}
fn client() -> Result<Client> {
    Ok(Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()?)
}
pub trait Clock: Send + Sync {
    fn now(&self) -> Duration;
    fn sleep(&self, d: Duration);
}
pub struct RealClock(Instant);
impl Default for RealClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}
impl Clock for RealClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
    fn sleep(&self, d: Duration) {
        thread::sleep(d)
    }
}
pub trait Http {
    fn post_success(&self, url: &str, body: serde_json::Value) -> Result<()> {
        self.post_json(url, body).map(|_| ())
    }
    fn post_multipart(&self, url: &str, form: multipart::Form) -> Result<serde_json::Value>;
    fn post_json(&self, url: &str, body: serde_json::Value) -> Result<serde_json::Value>;
    fn get_json(&self, url: &str) -> Result<serde_json::Value>;
}
pub struct ReqwestHttp {
    c: Client,
}
impl ReqwestHttp {
    pub fn new() -> Result<Self> {
        Ok(Self { c: client()? })
    }
}
impl Http for ReqwestHttp {
    fn post_success(&self, url: &str, body: serde_json::Value) -> Result<()> {
        let response = self.c.post(url).json(&body).send()?;
        if !response.status().is_success() {
            bail!("POST {url}: HTTP {}", response.status());
        }
        Ok(())
    }
    fn post_multipart(&self, url: &str, form: multipart::Form) -> Result<serde_json::Value> {
        response_json(
            self.c
                .post(url)
                .timeout(Duration::from_secs(24 * 3600))
                .multipart(form)
                .send()?,
        )
    }

    fn post_json(&self, u: &str, b: serde_json::Value) -> Result<serde_json::Value> {
        response_json(self.c.post(u).json(&b).send()?).context("POST失敗")
    }
    fn get_json(&self, u: &str) -> Result<serde_json::Value> {
        response_json(self.c.get(u).header(ACCEPT, "application/json").send()?).context("GET失敗")
    }
}
pub trait Readiness {
    fn recording_active(&mut self) -> bool;
    fn refresh_cpu(&mut self);
    fn cpu_usage(&self) -> f32;
}
#[derive(Default)]
pub struct SystemReadiness(System);
impl Readiness for SystemReadiness {
    fn recording_active(&mut self) -> bool {
        self.0.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        self.0
            .processes()
            .values()
            .any(|p| p.name().to_string_lossy().contains("EpgDataCap_Bon"))
    }
    fn refresh_cpu(&mut self) {
        self.0.refresh_cpu_usage();
    }
    fn cpu_usage(&self) -> f32 {
        self.0.global_cpu_usage()
    }
}
pub fn wait_for_ready(
    cfg: &UploadConfig,
    clock: &dyn Clock,
    readiness: &mut dyn Readiness,
    logger: &Logger,
    margin: Duration,
) {
    let start = clock.now();
    let deadline = Duration::from_secs(cfg.max_wait_minutes.saturating_mul(60));
    let remaining = || deadline.saturating_sub(clock.now().saturating_sub(start));
    let pause = |d: Duration| clock.sleep(d.min(remaining()));
    pause(margin);
    let cpu_enabled = (1..=100).contains(&cfg.cpu_usage_lower_limit);
    if !cpu_enabled {
        logger.warn("cpuUsageLowerLimit が範囲外のため CPU 待機をスキップ");
    }
    while !remaining().is_zero() {
        if cfg.wait_recorded_process && readiness.recording_active() {
            pause(Duration::from_secs(cfg.wait_time_interval.max(1)));
            continue;
        }
        break;
    }
    while cpu_enabled && !remaining().is_zero() {
        readiness.refresh_cpu();
        let interval = Duration::from_secs(1).max(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        if remaining() < interval {
            pause(remaining());
            break;
        }
        pause(interval);
        readiness.refresh_cpu();
        if readiness.cpu_usage() < cfg.cpu_usage_lower_limit as f32 {
            return;
        }
        pause(Duration::from_secs(30));
    }
    if remaining().is_zero() {
        logger.warn("待機上限を超過。後続処理へ進みます");
    }
}
pub struct RateReader<R> {
    inner: R,
    rate: f64,
    start: Duration,
    clock: Arc<dyn Clock>,
    bytes: u64,
}
impl<R> RateReader<R> {
    pub fn new(inner: R, rate: f64, clock: Arc<dyn Clock>) -> Self {
        Self {
            inner,
            rate,
            start: clock.now(),
            clock,
            bytes: 0,
        }
    }
}
impl<R: Read> Read for RateReader<R> {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(b)?;
        self.bytes += n as u64;
        if self.rate > 0.0 {
            let target = Duration::try_from_secs_f64(self.bytes as f64 / (self.rate * 1_000_000.0))
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
            if let Some(wait) = target.checked_sub(self.clock.now().saturating_sub(self.start)) {
                self.clock.sleep(wait);
            }
        }
        Ok(n)
    }
}
pub fn upload(
    http: &dyn Http,
    cfg: &UploadConfig,
    r: &Recording,
    id: i64,
    logger: &Logger,
    clock: Arc<dyn Clock>,
) -> Result<()> {
    let url = format!(
        "{}/api/videos/upload",
        cfg.epgstation_url.trim_end_matches('/')
    );
    for attempt in 0..cfg.upload_max_retry {
        let result = (|| -> Result<serde_json::Value> {
            let base = multipart::Form::new()
                .text("recordedId", id.to_string())
                .text("parentDirectoryName", cfg.parent_directory_name.clone())
                .text("viewName", cfg.view_name.clone())
                .text("fileType", cfg.file_type.clone());
            let form = if cfg.upload_mode.eq_ignore_ascii_case("path") {
                base.text("localFilePath", r.filepath.to_string_lossy().to_string())
            } else if cfg.upload_mode.eq_ignore_ascii_case("upload") {
                let f = File::open(&r.filepath).with_context(|| {
                    format!("録画ファイルを開けません: {}", r.filepath.display())
                })?;
                let reader = RateReader::new(f, cfg.limit_rate_mbps, clock.clone());
                let part =
                    multipart::Part::reader_with_length(reader, fs::metadata(&r.filepath)?.len())
                        .file_name(
                            r.filepath
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned(),
                        )
                        .mime_str("video/mp2t")?;
                base.part("file", part)
            } else {
                bail!("uploadMode は upload または path");
            };
            let json = http.post_multipart(&url, form)?;
            if json.get("recordedId").is_none() {
                bail!("応答JSONに recordedId がありません: {json}")
            };
            Ok(json)
        })();
        match result {
            Ok(_) => return Ok(()),
            Err(e) => {
                logger.error(&format!(
                    "upload {}/{} 回目失敗: {e:#}",
                    attempt + 1,
                    cfg.upload_max_retry
                ));
                if attempt + 1 < cfg.upload_max_retry {
                    let sec = (30u64.saturating_mul(1u64 << attempt.min(10))).min(600);
                    clock.sleep(Duration::from_secs(sec));
                }
            }
        }
    }
    bail!("アップロードが最大試行回数で失敗")
}
pub fn delete_recording(path: &Path, cfg: &UploadConfig, sleeper: &dyn Clock, logger: &Logger) {
    if !cfg.delete_edcb_rec_file {
        return;
    }
    sleeper.sleep(Duration::from_secs(cfg.delay_delete_time));
    if matches!(path.try_exists(), Ok(false)) {
        logger.info("録画ファイルは既に存在しないため削除をスキップ");
        return;
    }
    for i in 0..10 {
        match fs::remove_file(path) {
            Ok(()) => {
                logger.info(&format!("録画データを削除しました: {}", path.display()));
                return;
            }
            Err(e) => {
                logger.warn(&format!("録画ファイル削除 {}/10 回目失敗: {e}", i + 1));
                sleeper.sleep(Duration::from_secs(30));
            }
        }
    }
    logger.error(&format!(
        "録画ファイルを10回試行しても削除できません: {}",
        path.display()
    ));
}
fn encode(
    http: &dyn Http,
    cfg: &Config,
    id: i64,
    sleeper: &dyn Clock,
    logger: &Logger,
) -> Result<()> {
    let base = cfg.epgstation_upload.epgstation_url.trim_end_matches('/');
    let mut video = None;
    for attempt in 0..5 {
        match http
            .get_json(&format!("{base}/api/recorded/{id}?isHalfWidth=true"))
            .and_then(|v| {
                v["videoFiles"][0]["id"]
                    .as_i64()
                    .ok_or_else(|| anyhow!("videoFiles[0].id がありません"))
            }) {
            Ok(v) => {
                video = Some(v);
                break;
            }
            Err(e) => {
                logger.warn(&format!(
                    "videoFileId取得 {}/5 回目失敗: {e:#}",
                    attempt + 1
                ));
                if attempt < 4 {
                    sleeper.sleep(Duration::from_secs(30));
                }
            }
        }
    }
    let source = video.ok_or_else(|| anyhow!("videoFileIdを取得できません"))?;
    let body = serde_json::json!({"recordedId":id,"sourceVideoFileId":source,"parentDir":cfg.epgstation_encode.parent_dir.as_deref().unwrap_or(""),"directory":cfg.epgstation_encode.directory.as_deref().unwrap_or(""),"isSaveSameDirectory":cfg.epgstation_encode.is_save_same_directory,"mode":cfg.epgstation_encode.mode,"removeOriginal":cfg.epgstation_encode.remove_original});
    http.post_success(&format!("{base}/api/encode"), body)?;
    Ok(())
}
fn run() -> Result<()> {
    let exe = env::current_exe().context("exe path取得失敗")?;
    let dir = exe.parent().ok_or_else(|| anyhow!("exe directoryなし"))?;
    let logger = Logger::new(dir.join("edcb_to_epgstation.log"), false);
    logger.info("start");
    let r = recording_from(|k| env::var(k).ok())?;
    for k in ENV_KEYS {
        if let Ok(v) = env::var(k) {
            logger.info(&format!("{k}={v}"));
        }
    }
    let config_path = dir.join("config.yml");
    let cfg = load_config(&config_path)?;
    let logger = Logger::new(
        dir.join("edcb_to_epgstation.log"),
        cfg.epgstation_upload
            .log_level
            .eq_ignore_ascii_case("debug"),
    );
    logger.debug(&format!("config: {cfg:?}"));
    let http = ReqwestHttp::new()?;
    let clock = Arc::new(RealClock::default());
    let margin = Duration::from_secs(
        rand::rng().random_range(0..=cfg.epgstation_upload.wait_time_random_margin),
    );
    process_recording(
        &cfg,
        &r,
        &http,
        clock,
        &mut SystemReadiness::default(),
        &logger,
        margin,
    )
}

pub fn load_config(path: &Path) -> Result<Config> {
    let cfg: Config = serde_yaml::from_slice(
        &fs::read(path).with_context(|| format!("config.ymlを読めません: {}", path.display()))?,
    )?;
    if !["upload", "path"]
        .iter()
        .any(|m| cfg.epgstation_upload.upload_mode.eq_ignore_ascii_case(m))
    {
        bail!("uploadMode は upload または path");
    }
    if !cfg.epgstation_upload.limit_rate_mbps.is_finite()
        || cfg.epgstation_upload.limit_rate_mbps < 0.0
    {
        bail!("limitRateMBps は有限の非負数");
    }
    if cfg.epgstation_upload.upload_max_retry == 0 {
        bail!("uploadMaxRetry は1以上");
    }
    Ok(cfg)
}

pub fn process_recording(
    cfg: &Config,
    r: &Recording,
    http: &dyn Http,
    clock: Arc<dyn Clock>,
    readiness: &mut dyn Readiness,
    logger: &Logger,
    margin: Duration,
) -> Result<()> {
    let (description, extended) = read_program(r, &cfg.epgstation_upload, logger);
    let base = cfg.epgstation_upload.epgstation_url.trim_end_matches('/');
    let created = http.post_json(
        &format!("{base}/api/recorded"),
        serde_json::to_value(RecordedBody {
            channel_id: channel_id(&r.onid, &r.sid)?,
            start_at: r.start,
            end_at: r.end,
            name: &r.title,
            description: &description,
            extended: &extended,
        })?,
    )?;
    let id = created["recordedId"]
        .as_i64()
        .ok_or_else(|| anyhow!("recorded応答に recordedId がありません"))?;
    wait_for_ready(
        &cfg.epgstation_upload,
        clock.as_ref(),
        readiness,
        logger,
        margin,
    );
    upload(http, &cfg.epgstation_upload, r, id, logger, clock.clone())?;
    delete_recording(&r.filepath, &cfg.epgstation_upload, clock.as_ref(), logger);
    if cfg.epgstation_encode.run_encode {
        encode(http, cfg, id, clock.as_ref(), logger)?;
    }
    Ok(())
}
pub fn entry() {
    if let Err(e) = run() {
        let path = env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("edcb_to_epgstation.log")))
            .unwrap_or_else(|| PathBuf::from("edcb_to_epgstation.log"));
        Logger::new(path, false).error(&format!("{e:#}"));
        std::process::exit(1);
    }
}

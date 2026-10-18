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
    30
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

#[derive(Debug, Clone)]
pub struct Recording {
    pub filepath: PathBuf,
    pub filename: String,
    pub title: String,
    pub onid: String,
    pub sid: String,
    pub start: i64,
    pub end: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestEntry {
    filepath: PathBuf,
    filename: String,
    title: String,
    onid: String,
    sid: String,
    start_at: i64,
    end_at: i64,
    recorded_id: Option<i64>,
}

#[derive(Debug, Clone, Copy)]
pub struct BackfillOptions {
    pub apply: bool,
    pub interval_sec: u64,
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct BackfillSummary {
    pub succeeded: usize,
    pub skipped: usize,
    pub failed: usize,
}

#[derive(Debug, Deserialize)]
struct BackfillState {
    filepath: PathBuf,
    #[serde(rename = "recordedId")]
    recorded_id: i64,
    phase: String,
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
    let (desc, ext) = parse_program_parts(prog)?;
    Ok((
        desc,
        format!(
            "{}\n{}",
            ext,
            drop.replace("\r\n", "\n").replace('\r', "\n")
        ),
    ))
}
const MAX_EXTENDED_BYTES: usize = 60_000;

fn parse_program_parts(prog: &str) -> Result<(String, String)> {
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
    Ok((desc, ext.to_string()))
}

fn limit_extended(program: &str, drop: &str, logger: &Logger) -> String {
    let original_bytes = program.len() + 1 + drop.len();
    let result = if original_bytes <= MAX_EXTENDED_BYTES {
        format!("{program}\n{drop}")
    } else if program.len() + 1 > MAX_EXTENDED_BYTES {
        let mut end = MAX_EXTENDED_BYTES;
        while !program.is_char_boundary(end) {
            end -= 1;
        }
        program[..end].to_string()
    } else {
        let available = MAX_EXTENDED_BYTES - program.len() - 1;
        let lines: Vec<&str> = drop.split_inclusive('\n').collect();
        // 先頭行から順に落とし、残りの長さは引き算で追う (毎回連結しない)。
        let mut omitted = 0;
        let mut tail_len = drop.len();
        loop {
            let note = format!("(ドロップログが長いため先頭 {omitted} 行を省略)\n");
            if note.len() > available {
                let note = format!("(ドロップログが長いため先頭 {} 行を省略)\n", lines.len());
                let mut end = MAX_EXTENDED_BYTES - note.len() - 1;
                while !program.is_char_boundary(end) {
                    end -= 1;
                }
                break format!("{}\n{note}", &program[..end]);
            }
            if note.len() + tail_len <= available {
                break format!("{program}\n{note}{}", lines[omitted..].concat());
            }
            if omitted == lines.len() {
                break format!("{program}\n{note}");
            }
            tail_len -= lines[omitted].len();
            omitted += 1;
        }
    };
    if original_bytes > MAX_EXTENDED_BYTES {
        logger.warn(&format!(
            "extended を切り詰め: 元={} bytes, 後={} bytes",
            original_bytes,
            result.len()
        ));
    }
    result
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
        let (description, program) = parse_program_parts(&prog)?;
        let drop = drop.replace("\r\n", "\n").replace('\r', "\n");
        Ok((description, limit_extended(&program, &drop, logger)))
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
    fn post_success(&self, url: &str, body: serde_json::Value) -> Result<serde_json::Value> {
        self.post_json(url, body)
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
    fn post_success(&self, url: &str, body: serde_json::Value) -> Result<serde_json::Value> {
        let mut response = self.c.post(url).json(&body).send()?;
        if !response.status().is_success() {
            bail!("POST {url}: HTTP {}", response.status());
        }
        let mut text = String::new();
        if response.read_to_string(&mut text).is_err() || text.is_empty() {
            return Ok(serde_json::Value::Null);
        }
        Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null))
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
        let attempt_start = clock.now();
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
            Ok(_) => {
                let elapsed = clock.now().saturating_sub(attempt_start).as_secs_f64();
                if cfg.upload_mode.eq_ignore_ascii_case("path") {
                    logger.info(&format!(
                        "path 登録成功: recordedId={id} ({}/{} 回目)",
                        attempt + 1,
                        cfg.upload_max_retry
                    ));
                } else {
                    let size = fs::metadata(&r.filepath).map(|m| m.len()).unwrap_or(0);
                    logger.info(&format!(
                        "upload 成功: recordedId={id} size={size} elapsed={elapsed:.3} ({}/{} 回目)",
                        attempt + 1,
                        cfg.upload_max_retry
                    ));
                }
                return Ok(());
            }
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
    // path モードでは EPGStation がファイルを移動済みのことが多い。無駄な待機で後続バッチを止めない。
    if matches!(path.try_exists(), Ok(false)) {
        logger.info("録画ファイルは既に存在しないため削除をスキップ");
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
    logger.info(&format!("videoFileId={source}"));
    let body = serde_json::json!({"recordedId":id,"sourceVideoFileId":source,"parentDir":cfg.epgstation_encode.parent_dir.as_deref().unwrap_or(""),"directory":cfg.epgstation_encode.directory.as_deref().unwrap_or(""),"isSaveSameDirectory":cfg.epgstation_encode.is_save_same_directory,"mode":cfg.epgstation_encode.mode,"removeOriginal":cfg.epgstation_encode.remove_original});
    let response = http.post_success(&format!("{base}/api/encode"), body)?;
    let encode_id = response["encodeId"]
        .as_i64()
        .map(|id| id.to_string())
        .unwrap_or_else(|| "不明".to_string());
    logger.info(&format!(
        "エンコード依頼: encodeId={encode_id} mode={}",
        cfg.epgstation_encode.mode
    ));
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

fn run_backfill(args: &[String]) -> Result<()> {
    let usage = "使い方: edcb_to_epgstation_ts.exe backfill <manifest.jsonl> [--apply] [--interval-sec N] [--limit N]";
    if args.len() < 2 || args[1].starts_with("--") {
        bail!("{usage}");
    }
    let manifest = PathBuf::from(&args[1]);
    let mut opts = BackfillOptions {
        apply: false,
        interval_sec: 5,
        limit: None,
    };
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--apply" => opts.apply = true,
            "--interval-sec" | "--limit" => {
                let flag = args[i].as_str();
                i += 1;
                let value = args.get(i).ok_or_else(|| anyhow!("{usage}"))?;
                if flag == "--interval-sec" {
                    opts.interval_sec = value.parse().with_context(|| usage.to_string())?;
                } else {
                    opts.limit = Some(value.parse().with_context(|| usage.to_string())?);
                }
            }
            _ => bail!("{usage}"),
        }
        i += 1;
    }
    let exe = env::current_exe().context("exe path取得失敗")?;
    let dir = exe.parent().ok_or_else(|| anyhow!("exe directoryなし"))?;
    let cfg = load_config(&dir.join("config.yml"))?;
    let logger = Logger::new(
        dir.join("edcb_to_epgstation.log"),
        cfg.epgstation_upload
            .log_level
            .eq_ignore_ascii_case("debug"),
    );
    let http = ReqwestHttp::new()?;
    let summary = backfill(
        &cfg,
        &manifest,
        &opts,
        &http,
        Arc::new(RealClock::default()),
        &logger,
    )?;
    if summary.failed > 0 {
        bail!("backfill失敗: {}件", summary.failed);
    }
    Ok(())
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
    let started = clock.now();
    let metadata = fs::metadata(&r.filepath)
        .with_context(|| format!("録画ファイルを確認できません: {}", r.filepath.display()))?;
    if metadata.len() == 0 {
        logger.warn("録画ファイルが 0 バイトのため登録をスキップ");
        return Ok(());
    }
    let path_mode = cfg
        .epgstation_upload
        .upload_mode
        .eq_ignore_ascii_case("path");
    if path_mode {
        logger.info("path モードのため待機をスキップ");
    } else {
        wait_for_ready(
            &cfg.epgstation_upload,
            clock.as_ref(),
            readiness,
            logger,
            margin,
        );
    }
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
    logger.info(&format!("recorded 作成: recordedId={id} title={}", r.title));
    upload(http, &cfg.epgstation_upload, r, id, logger, clock.clone())?;
    delete_recording(&r.filepath, &cfg.epgstation_upload, clock.as_ref(), logger);
    if cfg.epgstation_encode.run_encode {
        encode(http, cfg, id, clock.as_ref(), logger)?;
    } else {
        logger.info("エンコードは無効 (runEncode=false)");
    }
    logger.info(&format!(
        "完了: recordedId={id} 所要 {:.3}s",
        clock.now().saturating_sub(started).as_secs_f64()
    ));
    Ok(())
}

fn state_path(manifest: &Path) -> PathBuf {
    let mut name = manifest
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".state.jsonl");
    manifest.with_file_name(name)
}

fn read_backfill_state(path: &Path, logger: &Logger) -> Result<Vec<BackfillState>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            logger.error(&format!("[backfill] 状態ファイルを読めません: {e}"));
            return Err(e.into());
        }
    };
    Ok(text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| match serde_json::from_str(line) {
            Ok(state) => Some(state),
            Err(e) => {
                logger.warn(&format!("[backfill] 状態ファイル行を無視: {e}"));
                None
            }
        })
        .collect())
}

fn append_backfill_state(path: &Path, filepath: &Path, id: i64, phase: &str) -> Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(
        &mut file,
        &serde_json::json!({"filepath": filepath, "recordedId": id, "phase": phase}),
    )?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}

fn recording_from_manifest(item: &ManifestEntry) -> Recording {
    Recording {
        filepath: item.filepath.clone(),
        filename: item.filename.clone(),
        title: item.title.clone(),
        onid: item.onid.clone(),
        sid: format!("{:0>5}", item.sid),
        start: item.start_at,
        end: item.end_at,
    }
}

/// 登録済み録画の一括補完。HTTP・時計を差し替え可能。
pub fn backfill(
    cfg: &Config,
    manifest: &Path,
    opts: &BackfillOptions,
    http: &dyn Http,
    clock: Arc<dyn Clock>,
    logger: &Logger,
) -> Result<BackfillSummary> {
    let text = fs::read_to_string(manifest)
        .with_context(|| format!("manifestを読めません: {}", manifest.display()))?;
    let state_file = state_path(manifest);
    let mut states = read_backfill_state(&state_file, logger)?;
    let mut summary = BackfillSummary::default();
    let mut processed = 0usize;
    let mut pending_interval = false;

    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let line_no = index + 1;
        let item: ManifestEntry = match serde_json::from_str(line) {
            Ok(item) => item,
            Err(e) => {
                logger.error(&format!("[backfill] 行={line_no} JSON不正: {e}"));
                summary.failed += 1;
                continue;
            }
        };
        let id_text = item
            .recorded_id
            .map_or_else(|| "新規".into(), |id| id.to_string());
        let prefix = format!(
            "[backfill] 行={line_no} filename={} recordedId={id_text}",
            item.filename
        );
        let metadata = match fs::metadata(&item.filepath) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                logger.info(&format!(
                    "{prefix} スキップ(ファイルなし。登録済みの可能性)"
                ));
                summary.skipped += 1;
                continue;
            }
            Err(e) => {
                logger.error(&format!("{prefix} ファイル確認失敗: {e}"));
                summary.failed += 1;
                continue;
            }
        };
        if metadata.len() == 0 {
            logger.info(&format!("{prefix} スキップ(0バイト)"));
            summary.skipped += 1;
            continue;
        }

        let matching: Vec<_> = states
            .iter()
            .filter(|s| s.filepath == item.filepath)
            .collect();
        let uploaded = matching.iter().any(|s| s.phase == "uploaded");
        let resume_id = matching
            .iter()
            .rev()
            .find(|s| s.phase == "created")
            .map(|s| s.recorded_id);
        if item.recorded_id.is_none() && uploaded {
            logger.warn(&format!(
                "{prefix} スキップ(uploaded状態あり・ファイル残存)"
            ));
            summary.skipped += 1;
            continue;
        }
        if opts.limit.is_some_and(|limit| processed >= limit) {
            break;
        }
        if !opts.apply {
            let action = if let Some(id) = item.recorded_id {
                format!("既存 recordedId={id} に紐付け予定")
            } else if let Some(id) = resume_id {
                format!("既存 recordedId={id} に再開予定")
            } else {
                "新規登録予定".to_string()
            };
            logger.info(&format!("{prefix} ドライラン: {action}"));
            processed += 1;
            summary.succeeded += 1;
            continue;
        }
        let result = (|| -> Result<bool> {
            let mut upload_cfg = cfg.epgstation_upload.clone();
            upload_cfg.upload_mode = "path".into();
            let recording = recording_from_manifest(&item);
            let base = upload_cfg.epgstation_url.trim_end_matches('/');
            let id = if let Some(id) = item.recorded_id {
                let existing =
                    http.get_json(&format!("{base}/api/recorded/{id}?isHalfWidth=true"))?;
                let videos = existing["videoFiles"]
                    .as_array()
                    .ok_or_else(|| anyhow!("GET応答に videoFiles 配列がありません"))?;
                if !videos.is_empty() {
                    logger.info(&format!("{prefix} スキップ(既に動画あり)"));
                    return Ok(true);
                }
                if pending_interval {
                    clock.sleep(Duration::from_secs(opts.interval_sec));
                }
                id
            } else if let Some(id) = resume_id {
                if pending_interval {
                    clock.sleep(Duration::from_secs(opts.interval_sec));
                }
                id
            } else {
                if pending_interval {
                    clock.sleep(Duration::from_secs(opts.interval_sec));
                }
                let (description, extended) = read_program(&recording, &upload_cfg, logger);
                let created = http.post_json(
                    &format!("{base}/api/recorded"),
                    serde_json::to_value(RecordedBody {
                        channel_id: channel_id(&recording.onid, &recording.sid)?,
                        start_at: recording.start,
                        end_at: recording.end,
                        name: &recording.title,
                        description: &description,
                        extended: &extended,
                    })?,
                )?;
                let id = created["recordedId"]
                    .as_i64()
                    .ok_or_else(|| anyhow!("recorded応答に recordedId がありません"))?;
                append_backfill_state(&state_file, &item.filepath, id, "created")?;
                states.push(BackfillState {
                    filepath: item.filepath.clone(),
                    recorded_id: id,
                    phase: "created".into(),
                });
                id
            };
            logger.info(&format!("{prefix} upload開始 recordedId={id}"));
            upload(http, &upload_cfg, &recording, id, logger, clock.clone())?;
            if item.recorded_id.is_none() {
                append_backfill_state(&state_file, &item.filepath, id, "uploaded")?;
                states.push(BackfillState {
                    filepath: item.filepath.clone(),
                    recorded_id: id,
                    phase: "uploaded".into(),
                });
            }
            Ok(false)
        })();
        let was_skipped = matches!(result, Ok(true));
        match result {
            Ok(true) => {
                summary.skipped += 1;
            }
            Ok(false) => {
                processed += 1;
                summary.succeeded += 1;
                logger.info(&format!("{prefix} 処理成功"));
            }
            Err(e) => {
                processed += 1;
                logger.error(&format!("{prefix} 処理失敗: {e:#}"));
                summary.failed += 1;
            }
        }
        if !was_skipped {
            pending_interval = true;
        }
    }
    logger.info(&format!(
        "[backfill] 完了: 成功={} スキップ={} 失敗={}",
        summary.succeeded, summary.skipped, summary.failed
    ));
    Ok(summary)
}

pub fn entry() {
    let args: Vec<String> = env::args().collect();
    let result = if args.get(1).is_some_and(|a| a == "backfill") {
        run_backfill(&args[1..])
    } else {
        run()
    };
    if let Err(e) = result {
        let path = env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("edcb_to_epgstation.log")))
            .unwrap_or_else(|| PathBuf::from("edcb_to_epgstation.log"));
        Logger::new(path, false).error(&format!("{e:#}"));
        std::process::exit(1);
    }
}

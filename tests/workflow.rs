use edcb_to_epgstation::*;
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

#[derive(Default)]
struct FakeClock {
    state: Mutex<(Duration, Vec<Duration>, usize)>,
}
impl Clock for FakeClock {
    fn now(&self) -> Duration {
        self.state.lock().unwrap().0
    }
    fn sleep(&self, d: Duration) {
        let mut s = self.state.lock().unwrap();
        s.0 += d;
        s.2 += 1;
        if !d.is_zero() {
            s.1.push(d);
        }
    }
}
impl FakeClock {
    fn sleep_calls(&self) -> usize {
        self.state.lock().unwrap().2
    }
    fn sleeps(&self) -> Vec<u64> {
        self.state
            .lock()
            .unwrap()
            .1
            .iter()
            .map(Duration::as_secs)
            .collect()
    }
}
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static ID: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "edcb-rust-{}-{}",
            std::process::id(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn logger(&self) -> Logger {
        Logger::new(self.0.join("test.log"), false)
    }
    fn recording(&self) -> Recording {
        let filepath = self.0.join("録画.ts");
        fs::write(&filepath, b"TS-file-payload\x00\xff").unwrap();
        fs::write(
            format!("{}.program.txt", filepath.display()),
            "0\n1\n2\n3\n概要\n詳細情報\n本文\nジャンル : \n対象外",
        )
        .unwrap();
        fs::write(format!("{}.err", filepath.display()), "ドロップ\r\n").unwrap();
        Recording {
            filepath,
            filename: "録画".into(),
            title: "番組".into(),
            onid: "4".into(),
            sid: "00012".into(),
            start: 100,
            end: 200,
        }
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn config() -> Config {
    let mut c: Config = serde_yaml::from_str(include_str!("../config.yml.template")).unwrap();
    let u = &mut c.epgstation_upload;
    u.rec_details_program_folder = None;
    u.wait_recorded_process = false;
    u.cpu_usage_lower_limit = 0;
    u.wait_time_random_margin = 0;
    u.delay_delete_time = 2;
    c.epgstation_encode.run_encode = false;
    c
}
#[derive(Default)]
struct Ready {
    refreshes: usize,
    process_checks: usize,
    active_for: usize,
    usages: Vec<f32>,
}
impl Readiness for Ready {
    fn recording_active(&mut self) -> bool {
        self.process_checks += 1;
        self.process_checks <= self.active_for
    }
    fn refresh_cpu(&mut self) {
        self.refreshes += 1;
    }
    fn cpu_usage(&self) -> f32 {
        assert_eq!(self.refreshes % 2, 0);
        self.usages
            .get(self.refreshes / 2 - 1)
            .copied()
            .unwrap_or(0.0)
    }
}
#[derive(Debug)]
struct Request {
    line: String,
    headers: String,
    body: Vec<u8>,
}
struct Server {
    url: String,
    join: thread::JoinHandle<Vec<Request>>,
}
impl Server {
    fn new(responses: Vec<(u16, &'static str)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let join = thread::spawn(move || {
            responses.into_iter().map(|(status, body)| {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut data = Vec::new(); let mut byte = [0];
                while !data.ends_with(b"\r\n\r\n") { stream.read_exact(&mut byte).unwrap(); data.push(byte[0]); }
                let headers = String::from_utf8(data).unwrap();
                let length: usize = headers.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap())).unwrap_or(0);
                let mut data = vec![0; length]; stream.read_exact(&mut data).unwrap();
                write!(stream, "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/json\r\n\r\n{body}", body.len()).unwrap();
                Request { line: headers.lines().next().unwrap().to_owned(), headers, body: data }
            }).collect()
        });
        Self { url, join }
    }
    fn finish(self) -> Vec<Request> {
        self.join.join().unwrap()
    }
}
fn execute(
    c: &Config,
    r: &Recording,
    clock: Arc<FakeClock>,
    logger: &Logger,
) -> anyhow::Result<()> {
    process_recording(
        c,
        r,
        &ReqwestHttp::new()?,
        clock,
        &mut Ready::default(),
        logger,
        Duration::ZERO,
    )
}
fn field(req: &Request, name: &str) -> Vec<u8> {
    let boundary = req
        .headers
        .lines()
        .find_map(|l| l.split_once("boundary=").map(|(_, b)| b))
        .unwrap();
    let marker = format!("\r\n--{boundary}").into_bytes();
    let header = format!("name=\"{name}\"").into_bytes();
    let offset = req
        .body
        .windows(header.len())
        .position(|w| w == header)
        .unwrap();
    let start = offset
        + req.body[offset..]
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap()
        + 4;
    let end = start
        + req.body[start..]
            .windows(marker.len())
            .position(|w| w == marker)
            .unwrap();
    req.body[start..end].to_vec()
}
#[test]
fn template_and_omitted_new_defaults() {
    let t = Temp::new();
    let c = load_config(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/config.yml.template"
    )))
    .unwrap();
    assert_eq!(c.epgstation_upload.upload_mode, "upload");
    let mut v: serde_yaml::Value =
        serde_yaml::from_str(include_str!("../config.yml.template")).unwrap();
    for key in [
        "uploadMode",
        "limitRateMBps",
        "uploadMaxRetry",
        "maxWaitMinutes",
        "logLevel",
    ] {
        v["epgstationUpload"]
            .as_mapping_mut()
            .unwrap()
            .remove(serde_yaml::Value::from(key));
    }
    let p = t.0.join("config.yml");
    fs::write(&p, serde_yaml::to_string(&v).unwrap()).unwrap();
    let u = load_config(&p).unwrap().epgstation_upload;
    assert_eq!(
        (
            u.upload_mode.as_str(),
            u.limit_rate_mbps,
            u.upload_max_retry,
            u.max_wait_minutes,
            u.log_level.as_str()
        ),
        ("upload", 0.0, 5, 30, "info")
    );
}
#[test]
fn file_workflow_multipart_delete_then_encode() {
    let t = Temp::new();
    let r = t.recording();
    let mut c = config();
    c.epgstation_encode.run_encode = true;
    let server = Server::new(vec![
        (200, r#"{"recordedId":42}"#),
        (201, r#"{"recordedId":42,"message":"登録"}"#),
        (200, r#"{"videoFiles":[{"id":8}]}"#),
        (200, r#"{"encodeId":99}"#),
    ]);
    c.epgstation_upload.epgstation_url = server.url.clone();
    c.epgstation_encode.parent_dir = None;
    let clock = Arc::new(FakeClock::default());
    execute(&c, &r, clock.clone(), &t.logger()).unwrap();
    assert!(!r.filepath.exists());
    assert_eq!(clock.sleeps(), [2]);
    let log = fs::read_to_string(t.0.join("test.log")).unwrap();
    assert!(log.contains("recorded 作成: recordedId=42 title=番組"));
    assert!(log.contains("upload 成功: recordedId=42 size=17 elapsed=0.000 (1/5 回目)"));
    assert!(log.contains("videoFileId=8"));
    assert!(log.contains("エンコード依頼: encodeId=99 mode=HEVC_CMカット"));
    assert!(log.contains("完了: recordedId=42 所要 2.000s"));
    let req = server.finish();
    assert!(req[0].line.starts_with("POST /api/recorded "));
    let body: Value = serde_json::from_slice(&req[0].body).unwrap();
    assert_eq!(
        body,
        json!({"channelId":400012,"startAt":100,"endAt":200,"name":"番組","description":"概要","extended":"本文\n\nドロップ\n"})
    );
    assert!(req[1].line.starts_with("POST /api/videos/upload "));
    assert_eq!(field(&req[1], "file"), b"TS-file-payload\x00\xff");
    assert!(String::from_utf8_lossy(&req[1].body).contains("filename=\"録画.ts\""));
    assert!(String::from_utf8_lossy(&req[1].body).contains("Content-Type: video/mp2t"));
    for (key, value) in [
        ("recordedId", "42"),
        ("parentDirectoryName", "TS2"),
        ("viewName", "TS"),
        ("fileType", "ts"),
    ] {
        assert_eq!(field(&req[1], key), value.as_bytes());
    }
    assert!(req[2].line.contains("/api/recorded/42?isHalfWidth=true"));
    let b: Value = serde_json::from_slice(&req[3].body).unwrap();
    assert_eq!(
        b,
        json!({"recordedId":42,"sourceVideoFileId":8,"parentDir":"","directory":"","isSaveSameDirectory":false,"mode":"HEVC_CMカット","removeOriginal":false})
    );
}
#[test]
fn path_workflow_sends_exact_path_without_file() {
    let t = Temp::new();
    let r = t.recording();
    let mut c = config();
    c.epgstation_upload.upload_mode = "path".into();
    c.epgstation_upload.delete_edcb_rec_file = false;
    c.epgstation_encode.run_encode = true;
    let s = Server::new(vec![
        (200, r#"{"recordedId":1}"#),
        (200, r#"{"recordedId":1}"#),
        (200, r#"{"videoFiles":[{"id":2}]}"#),
        (200, r#"{"encodeId":3}"#),
    ]);
    c.epgstation_upload.epgstation_url = s.url.clone();
    let clock = Arc::new(FakeClock::default());
    let mut readiness = Ready::default();
    process_recording(
        &c,
        &r,
        &ReqwestHttp::new().unwrap(),
        clock.clone(),
        &mut readiness,
        &t.logger(),
        Duration::from_secs(9),
    )
    .unwrap();
    assert_eq!(clock.sleep_calls(), 0);
    assert_eq!((readiness.refreshes, readiness.process_checks), (0, 0));
    assert!(
        fs::read_to_string(t.0.join("test.log"))
            .unwrap()
            .contains("path モードのため待機をスキップ")
    );
    let log = fs::read_to_string(t.0.join("test.log")).unwrap();
    assert!(log.contains("recorded 作成: recordedId=1 title=番組"));
    assert!(log.contains("path 登録成功: recordedId=1 (1/5 回目)"));
    assert!(log.contains("エンコード依頼: encodeId=3 mode=HEVC_CMカット"));
    assert!(log.contains("完了: recordedId=1 所要 0.000s"));
    let req = s.finish();
    assert_eq!(
        field(&req[1], "localFilePath"),
        r.filepath.to_str().unwrap().as_bytes()
    );
    assert!(!String::from_utf8_lossy(&req[1].body).contains("name=\"file\""));
    assert!(req[0].line.starts_with("POST /api/recorded "));
    assert!(req[2].line.starts_with("GET /api/recorded/1"));
    assert!(req[3].line.starts_with("POST /api/encode "));
    assert_eq!(req.len(), 4);
    assert!(r.filepath.exists());
}
#[test]
fn upload_failure_backoff_capped_and_preserves_file() {
    let t = Temp::new();
    let r = t.recording();
    let mut c = config();
    c.epgstation_upload.upload_max_retry = 8;
    c.epgstation_encode.run_encode = true;
    let mut responses = vec![(200, r#"{"recordedId":1}"#)];
    responses.extend([
        (500, "{}"),
        (200, "{}"),
        (200, "invalid"),
        (400, "{}"),
        (500, "{}"),
        (500, "{}"),
        (500, "{}"),
        (500, "{}"),
    ]);
    let s = Server::new(responses);
    c.epgstation_upload.epgstation_url = s.url.clone();
    let clock = Arc::new(FakeClock::default());
    assert!(execute(&c, &r, clock.clone(), &t.logger()).is_err());
    assert_eq!(clock.sleeps(), [30, 60, 120, 240, 480, 600, 600]);
    assert!(r.filepath.exists());
    assert_eq!(s.finish().len(), 9);
}
#[test]
fn upload_retry_then_success() {
    let t = Temp::new();
    let r = t.recording();
    let mut c = config();
    let s = Server::new(vec![
        (200, r#"{"recordedId":1}"#),
        (503, "{}"),
        (200, r#"{"recordedId":1}"#),
    ]);
    c.epgstation_upload.epgstation_url = s.url.clone();
    let clock = Arc::new(FakeClock::default());
    execute(&c, &r, clock.clone(), &t.logger()).unwrap();
    assert_eq!(clock.sleeps(), [30, 2]);
    assert!(!r.filepath.exists());
    assert_eq!(s.finish().len(), 3);
}
#[test]
fn recorded_http_error_stops_before_upload() {
    let t = Temp::new();
    let r = t.recording();
    let mut c = config();
    let s = Server::new(vec![(500, r#"{"recordedId":1}"#)]);
    c.epgstation_upload.epgstation_url = s.url.clone();
    assert!(execute(&c, &r, Arc::new(FakeClock::default()), &t.logger()).is_err());
    assert!(r.filepath.exists());
    assert_eq!(s.finish().len(), 1);
}
#[test]
fn encode_get_retries_and_post_failure_propagates() {
    let t = Temp::new();
    let r = t.recording();
    let mut c = config();
    c.epgstation_encode.run_encode = true;
    c.epgstation_upload.delete_edcb_rec_file = false;
    let s = Server::new(vec![
        (200, r#"{"recordedId":1}"#),
        (200, r#"{"recordedId":1}"#),
        (500, "{}"),
        (200, r#"{"videoFiles":[]}"#),
        (200, r#"{"videoFiles":[{"id":3}]}"#),
        (500, "{}"),
    ]);
    c.epgstation_upload.epgstation_url = s.url.clone();
    let clock = Arc::new(FakeClock::default());
    assert!(execute(&c, &r, clock.clone(), &t.logger()).is_err());
    assert_eq!(clock.sleeps(), [30, 30]);
    assert!(r.filepath.exists());
    assert_eq!(s.finish().len(), 6);
}
#[test]
fn encode_get_exhaustion_stops_after_five() {
    let t = Temp::new();
    let r = t.recording();
    let mut c = config();
    c.epgstation_encode.run_encode = true;
    let mut responses = vec![(200, r#"{"recordedId":1}"#), (200, r#"{"recordedId":1}"#)];
    responses.extend(vec![(200, "{}"); 5]);
    let s = Server::new(responses);
    c.epgstation_upload.epgstation_url = s.url.clone();
    let clock = Arc::new(FakeClock::default());
    assert!(execute(&c, &r, clock.clone(), &t.logger()).is_err());
    assert_eq!(clock.sleeps(), [2, 30, 30, 30, 30]);
    assert_eq!(s.finish().len(), 7);
}
#[test]
fn deletion_missing_and_failure_logging() {
    let t = Temp::new();
    let c = config();
    let clock = FakeClock::default();
    delete_recording(
        &t.0.join("missing"),
        &c.epgstation_upload,
        &clock,
        &t.logger(),
    );
    // 既に存在しないファイルは delayDeleteTime を待たずにスキップする
    assert!(clock.sleeps().is_empty());
    let dir = t.0.join("directory.ts");
    fs::create_dir(&dir).unwrap();
    delete_recording(&dir, &c.epgstation_upload, &clock, &t.logger());
    assert!(dir.exists());
    assert_eq!(clock.sleeps(), [vec![2], vec![30; 10]].concat());
    let log = fs::read_to_string(t.0.join("test.log")).unwrap();
    assert!(log.contains("既に存在しない"));
    assert!(log.contains("10回試行"));
    assert!(!log.contains("削除しました"));
}
#[test]
fn readiness_cpu_boundaries_and_two_refreshes() {
    let t = Temp::new();
    for limit in [-1, 0, 1, 100, 101] {
        let mut c = config();
        c.epgstation_upload.cpu_usage_lower_limit = limit;
        let clock = FakeClock::default();
        let mut ready = Ready::default();
        wait_for_ready(
            &c.epgstation_upload,
            &clock,
            &mut ready,
            &t.logger(),
            Duration::ZERO,
        );
        assert_eq!(
            ready.refreshes,
            if (1..=100).contains(&limit) { 2 } else { 0 }
        );
        assert_eq!(
            clock.sleeps(),
            if (1..=100).contains(&limit) {
                vec![1]
            } else {
                vec![]
            }
        );
    }
}
#[test]
fn readiness_process_then_cpu_and_random_margin() {
    let t = Temp::new();
    let mut c = config();
    let u = &mut c.epgstation_upload;
    u.wait_recorded_process = true;
    u.wait_time_interval = 5;
    u.cpu_usage_lower_limit = 60;
    let clock = FakeClock::default();
    let mut ready = Ready {
        active_for: 2,
        usages: vec![60.0, 59.9],
        ..Ready::default()
    };
    wait_for_ready(u, &clock, &mut ready, &t.logger(), Duration::from_secs(3));
    assert_eq!(clock.sleeps(), [3, 5, 5, 1, 30, 1]);
    assert_eq!(ready.refreshes, 4);
    assert_eq!(ready.process_checks, 3);
}
#[test]
fn readiness_deadline_caps_all_waits() {
    let t = Temp::new();
    for (process, margin) in [(true, 0), (false, 0), (true, 120)] {
        let mut c = config();
        let u = &mut c.epgstation_upload;
        u.max_wait_minutes = 1;
        u.wait_recorded_process = process;
        u.cpu_usage_lower_limit = 60;
        let mut ready = Ready {
            active_for: usize::MAX,
            usages: vec![100.0; 10],
            ..Ready::default()
        };
        let clock = FakeClock::default();
        wait_for_ready(
            u,
            &clock,
            &mut ready,
            &t.logger(),
            Duration::from_secs(margin),
        );
        assert_eq!(clock.now(), Duration::from_secs(60));
    }
    assert!(
        fs::read_to_string(t.0.join("test.log"))
            .unwrap()
            .contains("待機上限")
    );
}
#[test]
fn rate_reader_respects_rate_without_real_sleep() {
    for rate in [0.0, 0.5, 2.0] {
        let clock = Arc::new(FakeClock::default());
        let bytes = vec![42; 1_000_000];
        let mut reader = RateReader::new(std::io::Cursor::new(bytes), rate, clock.clone());
        let mut total = 0;
        let mut b = [0; 8192];
        loop {
            let n = reader.read(&mut b).unwrap();
            if n == 0 {
                break;
            }
            total += n;
            if rate > 0.0 {
                assert!(clock.now().as_secs_f64() + 1e-9 >= total as f64 / (rate * 1_000_000.0));
            }
        }
        assert_eq!(total, 1_000_000);
        assert_eq!(
            clock.now(),
            if rate == 0.0 {
                Duration::ZERO
            } else {
                Duration::from_secs_f64(1.0 / rate)
            }
        );
    }
}
#[test]
fn program_read_failures_reach_log_and_continue_empty() {
    let t = Temp::new();
    let r = t.recording();
    let c = config();
    fs::write(format!("{}.program.txt", r.filepath.display()), "short").unwrap();
    assert_eq!(
        read_program(&r, &c.epgstation_upload, &t.logger()),
        (String::new(), String::new())
    );
    let log = fs::read_to_string(t.0.join("test.log")).unwrap();
    assert!(log.contains("index 4"));
    assert!(log.contains(&r.filepath.display().to_string()));
}

#[test]
fn workflow_injected_http_observes_wait_delete_encode_order() {
    struct ObservingHttp {
        path: PathBuf,
        clock: Arc<FakeClock>,
        calls: Mutex<Vec<&'static str>>,
    }
    impl Http for ObservingHttp {
        fn post_json(&self, url: &str, _: Value) -> anyhow::Result<Value> {
            if url.ends_with("/api/recorded") {
                assert_eq!(self.clock.sleeps(), [3, 1]);
                self.calls.lock().unwrap().push("recorded");
                Ok(json!({"recordedId":9}))
            } else {
                assert!(url.ends_with("/api/encode"));
                assert!(!self.path.exists());
                self.calls.lock().unwrap().push("encode");
                Ok(Value::Null)
            }
        }
        fn post_multipart(
            &self,
            _: &str,
            _: reqwest::blocking::multipart::Form,
        ) -> anyhow::Result<Value> {
            assert!(self.path.exists());
            assert_eq!(self.clock.sleeps(), [3, 1]);
            self.calls.lock().unwrap().push("upload");
            Ok(json!({"recordedId":9}))
        }
        fn get_json(&self, _: &str) -> anyhow::Result<Value> {
            assert!(!self.path.exists());
            assert_eq!(self.clock.sleeps(), [3, 1, 2]);
            self.calls.lock().unwrap().push("get");
            Ok(json!({"videoFiles":[{"id":2}]}))
        }
    }
    let t = Temp::new();
    let r = t.recording();
    let mut c = config();
    c.epgstation_encode.run_encode = true;
    c.epgstation_upload.cpu_usage_lower_limit = 60;
    let clock = Arc::new(FakeClock::default());
    let http = ObservingHttp {
        path: r.filepath.clone(),
        clock: clock.clone(),
        calls: Mutex::new(vec![]),
    };
    process_recording(
        &c,
        &r,
        &http,
        clock,
        &mut Ready::default(),
        &t.logger(),
        Duration::from_secs(3),
    )
    .unwrap();
    assert_eq!(
        *http.calls.lock().unwrap(),
        ["recorded", "upload", "get", "encode"]
    );
}

#[test]
fn zero_byte_recording_skips_all_http_and_keeps_file() {
    struct NoHttp;
    impl Http for NoHttp {
        fn post_multipart(
            &self,
            _: &str,
            _: reqwest::blocking::multipart::Form,
        ) -> anyhow::Result<Value> {
            panic!("HTTP must not be called")
        }
        fn post_json(&self, _: &str, _: Value) -> anyhow::Result<Value> {
            panic!("HTTP must not be called")
        }
        fn get_json(&self, _: &str) -> anyhow::Result<Value> {
            panic!("HTTP must not be called")
        }
    }
    let t = Temp::new();
    let r = t.recording();
    fs::write(&r.filepath, []).unwrap();
    let c = config();
    let clock = Arc::new(FakeClock::default());
    process_recording(
        &c,
        &r,
        &NoHttp,
        clock.clone(),
        &mut Ready::default(),
        &t.logger(),
        Duration::from_secs(10),
    )
    .unwrap();
    assert!(r.filepath.exists());
    assert_eq!(clock.sleep_calls(), 0);
    assert!(
        fs::read_to_string(t.0.join("test.log"))
            .unwrap()
            .contains("録画ファイルが 0 バイトのため登録をスキップ")
    );
}

#[test]
fn missing_recording_fails_before_http() {
    struct NoHttp;
    impl Http for NoHttp {
        fn post_multipart(
            &self,
            _: &str,
            _: reqwest::blocking::multipart::Form,
        ) -> anyhow::Result<Value> {
            panic!("HTTP must not be called")
        }
        fn post_json(&self, _: &str, _: Value) -> anyhow::Result<Value> {
            panic!("HTTP must not be called")
        }
        fn get_json(&self, _: &str) -> anyhow::Result<Value> {
            panic!("HTTP must not be called")
        }
    }
    let t = Temp::new();
    let r = t.recording();
    fs::remove_file(&r.filepath).unwrap();
    let c = config();
    let error = process_recording(
        &c,
        &r,
        &NoHttp,
        Arc::new(FakeClock::default()),
        &mut Ready::default(),
        &t.logger(),
        Duration::ZERO,
    )
    .unwrap_err();
    assert!(error.to_string().contains("録画ファイルを確認できません"));
}
#[test]
fn encode_accepts_non_json_success_body() {
    let t = Temp::new();
    let r = t.recording();
    let mut c = config();
    c.epgstation_encode.run_encode = true;
    let s = Server::new(vec![
        (200, r#"{"recordedId":1}"#),
        (200, r#"{"recordedId":1}"#),
        (200, r#"{"videoFiles":[{"id":2}]}"#),
        (200, "OK"),
    ]);
    c.epgstation_upload.epgstation_url = s.url.clone();
    execute(&c, &r, Arc::new(FakeClock::default()), &t.logger()).unwrap();
    assert_eq!(s.finish().len(), 4);
    let log = fs::read_to_string(t.0.join("test.log")).unwrap();
    assert!(log.contains("エンコード依頼: encodeId=不明 mode=HEVC_CMカット"));
}
#[test]
fn invalid_program_still_creates_recorded_with_empty_metadata() {
    let t = Temp::new();
    let r = t.recording();
    fs::remove_file(format!("{}.program.txt", r.filepath.display())).unwrap();
    let mut c = config();
    let s = Server::new(vec![
        (200, r#"{"recordedId":1}"#),
        (200, r#"{"recordedId":1}"#),
    ]);
    c.epgstation_upload.epgstation_url = s.url.clone();
    execute(&c, &r, Arc::new(FakeClock::default()), &t.logger()).unwrap();
    let requests = s.finish();
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["description"], "");
    assert_eq!(body["extended"], "");
    assert!(
        fs::read_to_string(t.0.join("test.log"))
            .unwrap()
            .contains("番組情報ファイルを開けません")
    );
}
#[test]
fn rate_reader_accounts_for_elapsed_time_and_rejects_unrepresentable_delay() {
    let clock = Arc::new(FakeClock::default());
    let mut reader = RateReader::new(&b"1234"[..], 1.0, clock.clone());
    clock.sleep(Duration::from_secs(1));
    let mut b = [0; 4];
    assert_eq!(reader.read(&mut b).unwrap(), 4);
    assert_eq!(clock.sleeps(), [1]);
    let mut reader = RateReader::new(&b"1"[..], f64::MIN_POSITIVE, clock);
    assert!(reader.read(&mut b).is_err());
}
#[test]
fn config_rejects_invalid_mode_rate_and_zero_attempts() {
    let t = Temp::new();
    let path = t.0.join("config.yml");
    for (key, value) in [
        ("uploadMode", serde_yaml::Value::from("bad")),
        ("limitRateMBps", serde_yaml::Value::from(-1)),
        ("uploadMaxRetry", serde_yaml::Value::from(0)),
    ] {
        let mut v: serde_yaml::Value =
            serde_yaml::from_str(include_str!("../config.yml.template")).unwrap();
        v["epgstationUpload"][key] = value;
        fs::write(&path, serde_yaml::to_string(&v).unwrap()).unwrap();
        assert!(load_config(&path).is_err());
    }
}

#[test]
fn cpu_refreshes_bracket_measurement_interval() {
    struct TimedCpu {
        clock: Arc<FakeClock>,
        samples: Vec<Duration>,
    }
    impl Readiness for TimedCpu {
        fn recording_active(&mut self) -> bool {
            false
        }
        fn refresh_cpu(&mut self) {
            self.samples.push(self.clock.now());
        }
        fn cpu_usage(&self) -> f32 {
            assert_eq!(self.samples.len(), 2);
            assert!(
                self.samples[1] - self.samples[0]
                    >= Duration::from_secs(1).max(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL)
            );
            0.0
        }
    }
    let t = Temp::new();
    let mut c = config();
    c.epgstation_upload.cpu_usage_lower_limit = 60;
    let clock = Arc::new(FakeClock::default());
    let mut cpu = TimedCpu {
        clock: clock.clone(),
        samples: vec![],
    };
    wait_for_ready(
        &c.epgstation_upload,
        clock.as_ref(),
        &mut cpu,
        &t.logger(),
        Duration::ZERO,
    );
    assert_eq!(cpu.samples, [Duration::ZERO, Duration::from_secs(1)]);
}

#[test]
fn cli_encode_http_failure_exits_nonzero_and_logs_at_executable_directory() {
    let t = Temp::new();
    let r = t.recording();
    // Use the actual template, then write a configuration beside the copied executable.
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(include_str!("../config.yml.template")).unwrap();
    let mut c = serde_json::to_value(yaml).unwrap();
    let s = Server::new(vec![
        (200, r#"{"recordedId":1}"#),
        (200, r#"{"recordedId":1}"#),
        (200, r#"{"videoFiles":[{"id":2}]}"#),
        (304, ""),
    ]);
    c["epgstationUpload"]["epgstationUrl"] = json!(s.url);
    c["epgstationUpload"]["recDetailsProgramFolder"] = Value::Null;
    c["epgstationUpload"]["waitRecordedProcess"] = json!(false);
    c["epgstationUpload"]["waitTimeRandomMargin"] = json!(0);
    c["epgstationUpload"]["cpuUsageLowerLimit"] = json!(0);
    c["epgstationUpload"]["deleteEDCBRecFile"] = json!(false);
    fs::write(t.0.join("config.yml"), serde_yaml::to_string(&c).unwrap()).unwrap();
    let executable = t.0.join("edcb-test");
    fs::copy(env!("CARGO_BIN_EXE_edcb_to_epgstation_ts"), &executable).unwrap();
    let mut command = std::process::Command::new(&executable);
    command
        .current_dir(std::env::temp_dir())
        .env("FILEPATH", &r.filepath)
        .env("FILENAME", "録画")
        .env("TITLE", "番組")
        .env("ONID10", "4")
        .env("SID10", "12");
    for (key, value) in [
        ("SDYYYY", "2025"),
        ("SDMM", "1"),
        ("SDDD", "2"),
        ("STHH", "3"),
        ("STMM", "4"),
        ("STSS", "5"),
        ("EDYYYY", "2025"),
        ("EDMM", "1"),
        ("EDDD", "2"),
        ("ETHH", "4"),
        ("ETMM", "5"),
        ("ETSS", "6"),
    ] {
        command.env(key, value);
    }
    let output = command.output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(r.filepath.exists());
    assert_eq!(s.finish().len(), 4);
    let log = fs::read_to_string(t.0.join("edcb_to_epgstation.log")).unwrap();
    assert!(log.contains("HTTP 304"));
    assert!(log.contains("[ERROR]"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("HTTP 304"));
}

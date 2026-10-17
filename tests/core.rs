use chrono::{FixedOffset, TimeZone};
use edcb_to_epgstation::{channel_id, decode, parse_program, paths, recording_from};
use std::collections::HashMap;
use std::path::PathBuf;
fn vars() -> HashMap<String, String> {
    [
        ("FILEPATH", "C:/rec/a.ts"),
        ("FILENAME", "a"),
        ("TITLE", "A\u{200b}B"),
        ("ONID10", "0004"),
        ("SID10", "12"),
        ("SDYYYY", "2025"),
        ("SDMM", "01"),
        ("SDDD", "02"),
        ("STHH", "03"),
        ("STMM", "04"),
        ("STSS", "05"),
        ("EDYYYY", "2025"),
        ("EDMM", "01"),
        ("EDDD", "02"),
        ("ETHH", "04"),
        ("ETMM", "05"),
        ("ETSS", "06"),
    ]
    .into_iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect()
}
#[test]
fn env_parse_channel_jst() {
    let m = vars();
    let r = recording_from(|k| m.get(k).cloned()).unwrap();
    assert_eq!(r.title, "A B");
    assert_eq!(r.sid, "00012");
    assert_eq!(channel_id(&r.onid, &r.sid).unwrap(), 400012);
    let expected = FixedOffset::east_opt(9 * 3600)
        .unwrap()
        .with_ymd_and_hms(2025, 1, 2, 3, 4, 5)
        .unwrap()
        .timestamp_millis();
    assert_eq!(r.start, expected);
}
#[test]
fn missing_env_lists_all() {
    let e = recording_from(|_| None).unwrap_err().to_string();
    for key in vars().keys() {
        assert!(e.contains(key), "missing {key}: {e}");
    }
}
#[test]
fn program_parse_crlf_and_malformed() {
    let p = "0\r\n1\r\n2\r\n3\r\nDESC\r\n詳細情報\r\nBODY";
    let (d, x) = parse_program(p, "DROP\r\n").unwrap();
    assert_eq!(d, "DESC");
    assert_eq!(x, "BODY\nDROP\n");
    assert!(parse_program("short", " ").is_err());
}
#[test]
fn encoding_aliases_and_bom() {
    assert_eq!(decode(b"\xef\xbb\xbfHello", "cp932").unwrap(), "Hello");
    assert_eq!(decode(&[0x82, 0xa0], "shift_jis").unwrap(), "あ");
}
#[test]
fn program_paths() {
    let m = vars();
    let r = recording_from(|k| m.get(k).cloned()).unwrap();
    assert_eq!(
        paths(&r, Some("C:/programs")).0,
        PathBuf::from("C:/programs/a.ts.program.txt")
    );
    assert_eq!(paths(&r, None).1, PathBuf::from("C:/rec/a.ts.err"));
}

#[test]
fn mixed_delimiters_follow_python_split() {
    for (first, second) in [
        ("詳細情報", "ジャンル : "),
        ("ジャンル : ", "詳細情報"),
        ("詳細情報", "詳細情報"),
    ] {
        let p = format!("0\n1\n2\n3\n概要\n{first}\n本文\n{second}\n対象外");
        assert_eq!(
            parse_program(&p, "drop").unwrap(),
            ("概要".into(), "本文\n\ndrop".into())
        );
    }
}
#[test]
fn program_encodings_and_invalid_formats() {
    let p = "0\n1\n2\n3\n概要\n詳細情報\n本文";
    for label in ["utf-8", "shift-jis", "shift_jis", "cp932", "sjis"] {
        let bytes = if label == "utf-8" {
            p.as_bytes().to_vec()
        } else {
            encoding_rs::SHIFT_JIS.encode(p).0.into_owned()
        };
        assert_eq!(
            parse_program(&decode(&bytes, label).unwrap(), "")
                .unwrap()
                .0,
            "概要"
        );
    }
    assert!(
        parse_program("0\n1\n2\n3\n概要", "")
            .unwrap_err()
            .to_string()
            .contains("区切り")
    );
    assert!(decode(&[0xff], "utf-8").is_err());
    assert!(decode(b"a", "unsupported").is_err());
}
#[test]
fn paths_with_trailing_separator_and_short_folder() {
    let r = recording_from(|k| vars().get(k).cloned()).unwrap();
    assert_eq!(paths(&r, Some("programs/")), paths(&r, Some("programs")));
    assert_eq!(paths(&r, Some("ab")), paths(&r, None));
    assert_eq!(paths(&r, None).0, PathBuf::from("C:/rec/a.ts.program.txt"));
    assert_eq!(
        paths(&r, Some("programs")).1,
        PathBuf::from("programs/a.ts.err")
    );
}
#[test]
fn jst_is_independent_of_process_timezone() {
    if std::env::var_os("EDCB_TZ_CHILD").is_some() {
        let r = recording_from(|k| vars().get(k).cloned()).unwrap();
        assert_eq!(r.start, 1_735_754_645_000);
        assert_eq!(r.end - r.start, 3_661_000);
        return;
    }
    for tz in ["UTC", "Asia/Tokyo", "America/New_York"] {
        assert!(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "jst_is_independent_of_process_timezone"])
                .env("EDCB_TZ_CHILD", "1")
                .env("TZ", tz)
                .status()
                .unwrap()
                .success()
        );
    }
}

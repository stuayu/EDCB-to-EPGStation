# EDCB-to-EPGStation
EDCBで録画が完了したらEPGStationに録画ファイルを登録するpythonスクリプト

## 表示例
![Image gallery](images/image1.png)
![Image single](images/image2.png)
## 使い方
EDCBの録画終了後のバッチファイルに `edcb_to_epgstation_ts.bat` を指定する  

config.yml.templateをconfig.ymlにコピーする。  
設定項目を編集する。こちらに必要事項を入力してください。

## 事前準備
exe化したので以下の作業は不要になりました。  
> pythonのインストール (3.10以上)とrequestsとpyyamlをインストールしてください。  
動かないことがあれば、自動で生成される test.log をご確認ください。
BUGなどあればご連絡ください

## 環境構築(ビルド)
```pwsh
pyenv install 3.12.4
pyenv local 3.12.4

poetry install --no-root
poetry run nuitka .\edcb_to_epgstation_ts.py --onefile --standalone --output-dir=dist --output-filename=edcb_to_epgstation_ts.exe
```

## うまく動作しないとき

powershell上で `edcb_to_epgstation_ts.bat` を実行してみてください。powershell上に問題が表示される場合があります。\
またログファイルを生成するため、確認して随時対応してください。

## ビルド
* python -m pyinstaller .\edcb_to_epgstation_ts.py --onefile --name edcb_to_epgstation_ts.exe
* python -m nuitka .\edcb_to_epgstation_ts.py --onefile --standalone --output-dir=dist --output-filename=edcb_to_epgstation_ts.exe
* poetry run nuitka .\edcb_to_epgstation_ts.py --onefile --standalone --output-dir=dist --output-filename=edcb_to_epgstation_ts.exe
## Rust 版のビルド

Rust 版は `edcb_to_epgstation_ts.exe` としてビルドします。Windows GNU ターゲットの標準ライブラリと MinGW GCC が必要です。macOS では `x86_64-w64-mingw32-gcc` を PATH に通して、次を実行します。

```sh
cargo build --release --target x86_64-pc-windows-gnu
```

Homebrew の rustc が Windows GNU の標準ライブラリを見つけられない場合は、`rustup target add x86_64-pc-windows-gnu` を実行したうえで、rustup 管理の rustc を `RUSTC` で指定します。

```sh
RUSTC="$(rustup which rustc)" cargo build --release --target x86_64-pc-windows-gnu
```

生成された `target/x86_64-pc-windows-gnu/release/edcb_to_epgstation_ts.exe` を `dist` に配置する。Rust 版は exe と同じフォルダの `config.yml` を読み込みます。配置・既存 exe の差し替えは手動で行ってください。

### Rust 版の追加設定

既存項目はそのまま使用し、以下は省略時に既定値を適用する。

- `uploadMode`: `upload`（標準。ファイル本体をストリーミング送信）または `path`（`localFilePath` を送信）
- `limitRateMBps`: `0`（無制限）。正数でアップロード速度の上限 MB/s
- `uploadMaxRetry`: `5`。アップロードの最大試行回数
- `maxWaitMinutes`: `720`。プロセス・CPU 待機の合計上限（分）
- `logLevel`: `info` または `debug`。標準は `info`

### Rust 版の動作と検証

- 録画日時は PC のタイムゾーンに関係なく JST として変換します。
- 番組情報フォルダの末尾の区切り文字は省略できます。未指定時は TS に隣接する `.program.txt` と `.err` を読みます。解析できない場合はファイル名と原因を警告ログに記録し、空の番組情報で登録を続行します。
- 起動後は recorded 作成、待機、upload、削除、encode の順で処理します。乱数待機・プロセス待機・CPU 待機の合計を `maxWaitMinutes` で制限します。CPU は1秒以上の間隔を空けた2回の更新結果で計測し、設定値未満になるまで待機します。`cpuUsageLowerLimit` が 0 以下または 100 超の場合は警告して CPU 待機を省略します。
- upload は HTTP 2xx かつ応答 JSON に `recordedId` キーがあれば成功です。`uploadMaxRetry` は初回を含む最大試行回数です。失敗時は30、60、120秒と待機し、待機間隔の上限は600秒です。全試行失敗時は非0終了し、録画ファイルを削除しません。
- JSON API のタイムアウトは60秒、upload は24時間、接続タイムアウトは全 API 共通で10秒です。encode POST は本文形式を問わず HTTP 2xx を成功として扱います。
- 削除は成功した upload の後にのみ実行します。削除不能時は10回まで試行し、各失敗後に30秒待機して最終失敗を error ログに残します。その後、設定が有効なら encode へ進みます。encode 失敗時には、先に削除したファイルは復元されません。`path` モードで削除を有効にする場合は、EPGStation 側が登録時にファイルを移動・コピーする運用か確認してください。
- ログは exe と同じフォルダの `edcb_to_epgstation.log` と標準エラーに出力します。時刻・PID・レベルを含み、起動時に5 MiBを超えていれば3世代までローテーションします。使用する環境変数のみ記録します。
- `uploadMode` の不正値、負または非有限の `limitRateMBps`、0 の `uploadMaxRetry` は設定エラーで非0終了します。

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release --target x86_64-pc-windows-gnu
```

テストはローカル HTTP サーバーと差し替え可能な HTTP・時計・CPU/プロセス判定を使用します。待機は偽時計を進めるため実時間の sleep は発生しません。EDCB・EPGStation の実機には接続しません。

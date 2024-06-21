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
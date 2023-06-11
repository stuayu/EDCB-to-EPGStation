# EDCB-to-EPGStation
EDCBで録画が完了したらEPGStationに録画ファイルを登録するpythonスクリプト

## 使い方
EDCBの録画終了後のバッチファイルに `edcb_to_epgstation_ts.bat` を指定する  
pythonファイルのコメントの箇所を自分の環境に合わせて変更してください

## 事前準備
pythonのインストール (3.10以上) と requiremwnts.txt 内のパッケージをインストールして下さい  
動かないことがあれば、自動で生成される test.log をご確認ください。(注意: logging.DEBUG 指定にすると解決しやすいかも)  
BUGなどあればご連絡ください

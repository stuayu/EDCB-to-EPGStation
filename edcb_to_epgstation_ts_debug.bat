@echo off

rem // ウインドウを非表示にする
rem _EDCBX_HIDE_

rem // パラメータを環境変数に渡す
rem // こうすることで Python 側でも環境変数を参照できる
rem _EDCBX_DIRECT_

rem デバッグ用に仮想変数
setlocal
set FILEPATH=F:\\EDCB\\202306111935_ダーウィンが来た!「相葉くんと生放送!出るか？激レア・ウサギ」_NHK総合1・東京.ts
set FILENAME=202306111935_ダーウィンが来た!「相葉くんと生放送!出るか？激レア・ウサギ」_NHK総合1・東京
set TITLE=ダーウィンが来た！「相葉くんと生放送！出るか？激レア・ウサギ」[字]
set ONID10=32736
set SID10=1024
set SDYYYY=2023
set EDYYYY=2023
set SDMM=06
set EDMM=06
set SDDD=11
set EDDD=11
set STHH=19
set ETHH=20
set STMM=35
set ETMM=00
set STSS=00
set ETSS=00


rem EDCBで録画した結果をEPGStationの録画済みとして処理する
python.exe %~dp0\edcb_to_epgstation_ts.py
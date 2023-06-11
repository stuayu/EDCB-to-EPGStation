@echo off

rem // ウインドウを非表示にする
rem _EDCBX_HIDE_

rem // パラメータを環境変数に渡す
rem // こうすることで Python 側でも環境変数を参照できる
rem _EDCBX_DIRECT_

rem EDCBで録画した結果をEPGStationの録画済みとして処理する
python.exe %~dp0\edcb_to_epgstation_ts.py
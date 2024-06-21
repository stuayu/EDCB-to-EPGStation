@echo off

rem // ウインドウを非表示にする
rem _EDCBX_HIDE_

rem // パラメータを環境変数に渡す
rem // こうすることで Python 側でも環境変数を参照できる
rem _EDCBX_DIRECT_

rem EDCBで録画した結果をEPGStationの録画済みとして処理する
rem ディレクトリ移動
cd %~dp0

%~dp0\dist\edcb_to_epgstation_ts.exe

exit
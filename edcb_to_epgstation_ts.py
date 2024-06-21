import os
import datetime
import re
import subprocess
import requests
from logging.handlers import RotatingFileHandler
from loguru import logger
import json
import time
import yaml
import sys
import psutil
import random
import setproctitle
from box import Box, ConfigBox

# ログを書き込む
handler = RotatingFileHandler(
    "edcb_to_epgstation.log", maxBytes=30000, backupCount=3, encoding="utf-8"
)
logger.add(handler, backtrace=True)


class ReadEnviron:
    """
    class: ReadEnviron

    EDCBの環境変数読み取り
    """

    # インスタンス変数
    tsFilePath = None
    tsProgramFilePath = None
    title = None
    originalNetworkId = None
    serviceId = None
    startTime = None
    endTime = None
    description = None
    extended = None
    epgstationUrl = None
    parentDirectoryName = None
    viewName = None
    fileType = None
    recDetailsProgramFolder = None
    textEncoding = None
    deleteEDCBRecFile: bool = False
    config = None

    def __init__(self) -> None:
        # インスタンス変数を初期化
        # タイムゾーンの生成
        try:
            logger.info("start init")
            JST = datetime.timezone(datetime.timedelta(hours=+9), "JST")
            logger.debug(os.environ)  # EDCBの環境変数を表示
            self.tsFilePath = os.environ.get("FILEPATH")  # 録画ファイルのパス
            self.tsProgramFilePath = (os.environ.get("FILEPATH") + ".program.txt")  # EDCBの録画詳細ファイルのパス
            self.title = (os.environ.get("TITLE").replace("\u200b", " ").replace("\u3000", "　"))  # タイトル
            self.originalNetworkId = os.environ.get("ONID10")  # NetWorkID
            self.serviceId = os.environ.get("SID10").zfill(5)  # サービスID(EPGStationは5桁ほしいので残りは0埋め)
            self.startTime = datetime.datetime(
                year=int(os.environ.get("SDYYYY")),
                month=int(os.environ.get("SDMM")),
                day=int(os.environ.get("SDDD")),
                hour=int(os.environ.get("STHH")),
                minute=int(os.environ.get("STMM")),
                second=int(os.environ.get("STSS")),
                tzinfo=JST,
            )
            self.endTime = datetime.datetime(
                year=int(os.environ.get("EDYYYY")),
                month=int(os.environ.get("EDMM")),
                day=int(os.environ.get("EDDD")),
                hour=int(os.environ.get("ETHH")),
                minute=int(os.environ.get("ETMM")),
                second=int(os.environ.get("ETSS")),
                tzinfo=JST,
            )

            # ----------- 以下はそれぞれの環境に合わせて設定を変えてください ---------------
            self.config = Box.from_yaml(filename="./config.yml", encoding="utf-8")
            logger.debug(self.config)

            self.epgstationUrl = self.config.epgstationUpload.epgstationUrl  # EPGStationのURL
            self.parentDirectoryName = self.config.epgstationUpload.parentDirectoryName  # EPGStationのストレージに表示されている名前
            self.viewName = self.config.epgstationUpload.viewName  # EPGStationの再生ボタンで表示する名前
            self.fileType = self.config.epgstationUpload.fileType  # ts か recorded で指定する
            self.recDetailsProgramFolder = self.config.epgstationUpload.recDetailsProgramFolder  # EDCBの録画情報保存フォルダを指定してください
            self.textEncoding = self.config.epgstationUpload.textEncoding  # EDCBの録画情報ファイルの文字コード (shift-jis or utf-8) を指定してください
            # EDCBの録画ファイルをEPGStationへのアップロードが成功したらEDCBの録画ファイルを削除する（削除しない: False 削除する: True）
            self.deleteEDCBRecFile = self.config.epgstationUpload.deleteEDCBRecFile
            self.delayDeleteTime = self.config.epgstationUpload.delayDeleteTime

            # -----------------------------------------------------------------------------
        except TypeError:
            logger.warning(f"環境変数から録画ファイルのパスが取得できません。EDCBから実行していることを再度確認してください。")
            sys.exit()
        except Exception as e:
            logger.error(e, stack_info=True)
            sys.exit()

    def readTsProgram(self):
        """
        番組情報ファイルから必要なデータを取り出す
        """
        logger.debug("start readTsProgram")

        # 録画情報保存フォルダの指定がない場合はTSファイルと同じフォルダにする
        if (
            self.recDetailsProgramFolder is None
            or len(self.recDetailsProgramFolder) < 3
        ):
            loadPath = self.tsProgramFilePath
        else:
            loadPath = (
                self.recDetailsProgramFolder
                + os.environ.get("FILENAME")
                + ".ts.program.txt"
            )
            dropLogPath = (
                self.recDetailsProgramFolder
                + os.environ.get("FILENAME")
                + ".ts.err"
            )

        # 文字コードを変更可能に
        with open(loadPath, mode="+r", encoding=self.textEncoding) as f:
            allData: str = f.read()
            allDataLine = allData.split("\n")
        try:
            with open(dropLogPath, mode="+r", encoding=self.textEncoding) as f:
                dropLogAllData: str = f.read()
        except:
            dropLogAllData = ""

        self.description = allDataLine[4]
        self.extended = re.split("詳細情報\n|ジャンル : \n", allData)[1]  # 録画詳細ファイルの解析
        # DropLogを末尾に追加する
        self.extended += "\n" + dropLogAllData
        return

    def createRecData(self):
        """
        EDCBの録画結果をEPGStationの録画結果一覧に登録する
        """
        if self.originalNetworkId is None:
            raise Exception("環境変数がありません。")
        logger.debug("start createRecData")
        epgstationRecordedApi = self.epgstationUrl + "/api/recorded"
        logger.debug("epgstationRecordedApi:" + epgstationRecordedApi)

        data = {
            "channelId": int(self.originalNetworkId + self.serviceId),
            "startAt": int(time.mktime(self.startTime.timetuple()) * 1000),
            "endAt": int(time.mktime(self.endTime.timetuple()) * 1000),
            "name": self.title,
            "description": self.description,
            "extended": self.extended,
        }

        headers = {"content-type": "application/json"}
        res = requests.post(
            url=epgstationRecordedApi, data=json.dumps(data), headers=headers
        )

        logger.debug("res.status_code:" + str(res.status_code))
        logger.debug("res.text:" + str(res.text))

        recordedId = int(res.json()["recordedId"])
        logger.info(recordedId)

        return recordedId

    def uploadTsVideoFile(self, recordedId: int):
        """EPGStationに録画データをアップロードする

        Args:
            recordedId (int): 録画済み番組のID
        """
        waitRecordedProcess = self.config.epgstationUpload.waitRecordedProcess
        waitTimeInterval = int(self.config.epgstationUpload.waitTimeInterval)
        waitTimeRandomMargin = int(self.config.epgstationUpload.waitTimeRandomMargin)
        cpuUsageLowerLimit = int(self.config.epgstationUpload.cpuUsageLowerLimit)

        # 乱数を生成し、チェック間隔をずらす。
        waitTimeRandomMargin = random.randint(0, waitTimeRandomMargin)
        logger.debug(f"waitRecordedProcess: {waitRecordedProcess}")
        logger.debug(f"waitTimeInterval: {waitTimeInterval}")
        logger.debug(f"waitTimeRandomMargin: {waitTimeRandomMargin}")
        logger.debug(f"cpuUsageLowerLimit: {cpuUsageLowerLimit}")
        logger.debug(f"waitTimeRandomMargin: {waitTimeRandomMargin}")

        # EpgDataCap_Bonのプロセスが起動中であればアップロード処理を待機する
        if waitRecordedProcess == True:
            logger.debug("EpgDataCap_Bonのプロセス確認が有効になっています")
            check = True
            while check:
                check = False
                for proc in psutil.process_iter():
                    if "EpgDataCap_Bon" in proc.name():
                        check = True
                        break

                if check == True:
                    time.sleep(waitTimeInterval)  # 次回のプロセス確認までのインターバル
                    continue

        check = True
        while check:
            check = False
            # CPU使用率の監視 計算結果は 51.049060 のようになるはず
            cpu_percent = psutil.cpu_percent(interval=1)

            # 設定したCPU使用率の下限値よりも高ければループ
            if int(cpu_percent) >= cpuUsageLowerLimit:
                time.sleep(30)
                check = True

        logger.debug("start uploadTsVideoFile")
        
        def upload():
            try:
                logger.debug("open file")
                result = subprocess.run(
                    [
                        "C:\\Windows\\System32\\curl.exe",
                        "limit-rate 100M",
                        "-X",
                        "POST",
                        f"{self.epgstationUrl}/api/videos/upload",
                        "-H",
                        "accept: application/json",
                        "-H",
                        "Content-Type: multipart/form-data",
                        "-F",
                        f"recordedId={recordedId}",
                        "-F",
                        f"parentDirectoryName={self.parentDirectoryName}",
                        "-F",
                        f"viewName={self.viewName}",
                        "-F",
                        f"fileType={self.fileType}",
                        "-F",
                        f"file=@{self.tsFilePath};type=text/plain",
                    ],
                    shell=True,
                    check=True,
                    capture_output=True,
                )
                logger.info(f"res: {result.stdout}")
                logger.debug(f"res: {result.stderr}")

                res = json.loads(result.stdout.decode(encoding="shift-jis"))
                logger.debug(res)
                return res

            except Exception as e:
                logger.error(e, stack_info=True)
                sys.exit()
        

        while True:
            res = upload()
            if res["code"] == 200:
                break
        
        # 正常にアップロードがされたら録画ファイルを自動で削除する
        if self.deleteEDCBRecFile == True:
            logger.debug("正常にアップロードが完了したため、EDCBから録画ファイルを削除します。")
            if self.delayDeleteTime > 1:
                time.sleep(self.delayDeleteTime)
            for i in range(10):
                try:
                    os.remove(self.tsFilePath)
                    break
                except PermissionError:
                    # サムネイル生成が動いておりファイルを使用中
                    time.sleep(30)
                except Exception as e:
                    logger.error(e, stack_info=True)
            logger.info(f"録画データ：{self.tsFilePath}は正常に削除されました。")

        return


class VideoEncode(ReadEnviron):
    # インスタンス変数を定義
    recordedId = 0  # 録画済み番組 id
    sourceVideoFileId = 0  # ビデオファイル id
    parentDir = None  # 親ディレクトリ名 config recorded の name, isSaveSameDirectory が false の場合は必須
    directory = None  # 親ディレクトリ以下のディレクトリ設定
    isSaveSameDirectory = (
        False  # ソースビデオファイルと同じ場所に保存するか(初期値: False)
    )
    mode = None  # エンコードプリセット名 config encode の name
    removeOriginal = False  # 元ファイルを削除するか(初期値: False)

    def __init__(self) -> None:
        super().__init__()  # ReadEnvironからインスタンス変数を継承
        pass

    def getSourceVideoFileId(self, recordedId: int):
        """録画済み番組 idからビデオファイル idを検索

        Args:
            recordedId (int): 録画済み番組 id
        """
        headers = {"content-type": "application/json"}
        for _ in range(5):
            try:
                res = requests.get(
                    self.epgstationUrl
                    + f"/api/recorded/{ str(recordedId) }?isHalfWidth=true",
                    headers=headers,
                )
                videoFileId = int(res.json()["videoFiles"][0]["id"])
                logger.debug("videoFileId:" + str(videoFileId))
                break
            except Exception as e:
                logger.error(e, stack_info=True)
                time.sleep(30)  # 30秒待機してリトライ
                pass

        self.recordedId = recordedId
        self.sourceVideoFileId = videoFileId

        return

    def addVideoEncode(self):
        """エンコードタスクに追加する

        Args:
            parentDir (str | None, optional): 親ディレクトリ名. Defaults to None.
            directory (str | None, optional): 親ディレクトリ以下のディレクトリ設定. Defaults to None.
            isSaveSameDirectory (bool, optional): ソースビデオファイルと同じ場所に保存するか. Defaults to False.
            mode (str | None, optional): エンコードプリセット名. Defaults to None.
            removeOriginal (bool, optional): 元ファイルを削除するか. Defaults to False.
        """
        parentDir = self.config.epgstationEncode.parentDir or ""
        directory = self.config.epgstationEncode.directory or ""
        isSaveSameDirectory = self.config.epgstationEncode.isSaveSameDirectory
        mode = self.config.epgstationEncode.mode
        removeOriginal = self.config.epgstationEncode.removeOriginal

        data = {
            "recordedId": self.recordedId,  # 必須
            "sourceVideoFileId": self.sourceVideoFileId,  # 必須
            "parentDir": parentDir,
            "directory": directory,
            "isSaveSameDirectory": isSaveSameDirectory,
            "mode": mode,  # 必須
            "removeOriginal": removeOriginal,  # 必須
        }

        logger.debug(str(data))
        headers = {"content-type": "application/json"}
        res = requests.post(
            url=self.epgstationUrl + "/api/encode",
            data=json.dumps(data),
            headers=headers,
        )

        logger.debug(json.dumps(data))
        logger.debug(str(res.status_code) + "\n" + res.text)

        return


if __name__ == "__main__":
    try:
        setproctitle.setproctitle("edcb_to_epgstation")
        start = ReadEnviron()  # インスタンス生成
        logger.info(f"プロセス名:{setproctitle.getproctitle()}")
        start.readTsProgram()
        recordedId = start.createRecData()
        start.uploadTsVideoFile(recordedId=recordedId)

        if start.config.epgstationEncode.runEncode == True:
            logger.info("エンコードを開始します。")
            pass
        else:
            sys.exit()
    except Exception as e:
        logger.exception(e)  # エラーが発生したらファイルに書き込み
        sys.exit()

    # エンコードタスクを実行
    try:
        encode = VideoEncode()
        encode.getSourceVideoFileId(recordedId=recordedId)
        encode.addVideoEncode()  # エンコードタスクに追加

    except Exception as e:
        logger.exception(e)
        sys.exit()

    sys.exit()

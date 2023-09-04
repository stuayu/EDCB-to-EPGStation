import os
import datetime
import re
import requests
import logging
import json
import time
import yaml
import sys
import psutil
import random

# filename="test.log"を　追加
# 動かないなどあれば logging.DEBUGに書き換えてください
# 追加パッケージのインストールを忘れずに
logging.basicConfig(level=logging.INFO,
format="%(asctime)s - %(levelname)s:%(name)s - %(message)s",
filename="test.log")

class ReadEnviron:
    """
    class: ReadEnviron
    
    EDCBの環境変数読み取り
    """
    #インスタンス変数
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
    deleteEDCBRecFile:bool = False
    config = None
    
    def __init__(self) -> None:
        # インスタンス変数を初期化
        # タイムゾーンの生成
        logging.info("start init")
        JST = datetime.timezone(datetime.timedelta(hours=+9), 'JST')
        logging.debug(os.environ) # EDCBの環境変数を表示
        self.tsFilePath = os.environ.get("FILEPATH") # 録画ファイルのパス
        self.tsProgramFilePath = os.environ.get("FILEPATH") + ".program.txt" # EDCBの録画詳細ファイルのパス
        self.title = os.environ.get("TITLE").replace('\u200b', ' ').replace('\u3000', '　') # タイトル
        self.originalNetworkId = os.environ.get("ONID10") # NetWorkID
        self.serviceId = os.environ.get("SID10").zfill(5) # サービスID(EPGStationは5桁ほしいので残りは0埋め)
        self.startTime = datetime.datetime(year=int(os.environ.get("SDYYYY")), month=int(os.environ.get("SDMM")), day=int(os.environ.get("SDDD")), hour=int(os.environ.get("STHH")), minute=int(os.environ.get("STMM")), second=int(os.environ.get("STSS")), tzinfo=JST)
        self.endTime = datetime.datetime(year=int(os.environ.get("EDYYYY")), month=int(os.environ.get("EDMM")), day=int(os.environ.get("EDDD")), hour=int(os.environ.get("ETHH")), minute=int(os.environ.get("ETMM")), second=int(os.environ.get("ETSS")), tzinfo=JST)
        
        # ----------- 以下はそれぞれの環境に合わせて設定を変えてください ---------------
        with open('config.yml', 'r', encoding="utf-8") as yml:
            self.config = yaml.safe_load(yml)
        
        self.epgstationUrl = self.config["epgstationUpload"]["epgstationUrl"] # EPGStationのURL
        self.parentDirectoryName = self.config["epgstationUpload"]["parentDirectoryName"] # EPGStationのストレージに表示されている名前
        self.viewName = self.config["epgstationUpload"]["viewName"] # EPGStationの再生ボタンで表示する名前
        self.fileType = self.config["epgstationUpload"]["fileType"] # ts か recorded で指定する
        self.recDetailsProgramFolder = self.config["epgstationUpload"]["recDetailsProgramFolder"] # EDCBの録画情報保存フォルダを指定してください
        self.textEncoding = self.config["epgstationUpload"]["textEncoding"] # EDCBの録画情報ファイルの文字コード (shift-jis or utf-8) を指定してください
        self.deleteEDCBRecFile = self.config["epgstationUpload"]["deleteEDCBRecFile"]  # EDCBの録画ファイルをEPGStationへのアップロードが成功したら
                                        # EDCBの録画ファイルを削除する（削除しない: False 削除する: True）
        
        # -----------------------------------------------------------------------------
        
    def readTsProgram(self):
        """
        番組情報ファイルから必要なデータを取り出す
        """
        logging.debug("start readTsProgram")
        
        # 録画情報保存フォルダの指定がない場合はTSファイルと同じフォルダにする
        if self.recDetailsProgramFolder is None or len(self.recDetailsProgramFolder) < 3:
            loadPath = self.tsProgramFilePath
        else:
            loadPath = self.recDetailsProgramFolder + os.environ.get("FILENAME") + ".ts.program.txt"
            
        # 文字コードを変更可能に
        with open(loadPath, mode="+r", encoding=self.textEncoding) as f:
            allData: str = f.read()
            allDataLine = allData.split("\n")
        
        self.description = allDataLine[4]
        self.extended = re.split("詳細情報\n|ジャンル : \n", allData)[1] # 録画詳細ファイルの解析
        return
        
    def createRecData(self):
        """
        EDCBの録画結果をEPGStationの録画結果一覧に登録する
        """
        logging.debug("start createRecData")
        epgstationRecordedApi = self.epgstationUrl + "/api/recorded"
        logging.debug("epgstationRecordedApi:" + epgstationRecordedApi)
        
        data = {'channelId': int(self.originalNetworkId + self.serviceId), 'startAt': int(time.mktime(self.startTime.timetuple()) * 1000), 'endAt': int(time.mktime(self.endTime.timetuple()) * 1000), 'name': self.title, 'description': self.description, 'extended': self.extended}
        
        headers = {'content-type': 'application/json'}
        res = requests.post(url=epgstationRecordedApi, data=json.dumps(data), headers= headers)
        
        logging.debug("res.status_code:"+ str(res.status_code))
        logging.debug("res.text:"+ str(res.text))
        
        recordedId = int(res.json()['recordedId'])
        logging.info(recordedId)
        
        return recordedId
        
    def uploadTsVideoFile(self, recordedId: int):
        """EPGStationに録画データをアップロードする

        Args:
            recordedId (int): 録画済み番組のID
        """
        waitRecordedProcess = bool(self.config["epgstationUpload"]["waitRecordedProcess"])
        waitTimeInterval = int(self.config["epgstationUpload"]["waitTimeInterval"])
        waitTimeRandomMargin = int(self.config["epgstationUpload"]["waitTimeRandomMargin"])
        cpuUsageLowerLimit = int(self.config["epgstationUpload"]["cpuUsageLowerLimit"])
        
        # 乱数を生成し、チェック間隔をずらす。
        waitTimeRandomMargin = random.randint(0, waitTimeRandomMargin)
        
        # EpgDataCap_Bonのプロセスが起動中であればアップロード処理を待機する
        if waitRecordedProcess == True:
            logging.debug("EpgDataCap_Bonのプロセス確認が有効になっています")
            check = True
            while check:
                check = False
                for proc in psutil.process_iter():
                    
                    if 'EpgDataCap_Bon' in proc.name():
                        check = True
                
                if check == True:
                    time.sleep(waitTimeInterval) # 次回のプロセス確認までのインターバル
                
                # CPU使用率の監視 計算結果は 51.049060 のようになるはず
                cpu_percent = psutil.cpu_percent(percpu=True)
                
                # 設定したCPU使用率の下限値よりも高ければループ
                if cpu_percent >= cpuUsageLowerLimit:
                    check = True

        logging.debug("start uploadTsVideoFile")
        
        headers = {'accept': 'application/json'}
        data = {'recordedId': recordedId, 'parentDirectoryName': self.parentDirectoryName, 'viewName': self.viewName, 'fileType': self.fileType}
        f = open(self.tsFilePath, 'rb')
        files = {'file': f}
        
        logging.debug(str(data))
        res = requests.post(url=self.epgstationUrl + "/api/videos/upload", headers=headers, data=data, files=files)
        
        logging.debug(str(res.status_code))
        
        f.close() # ファイルを閉じる
        
        # 正常にアップロードがされたら録画ファイルを自動で削除する
        if res.status_code == 200 and self.deleteEDCBRecFile == True:
            os.remove(self.tsFilePath)
            logging.info(f"録画データ：{self.tsFilePath}は正常に削除されました。")
        
        logging.debug(str(res.text))
        return

class VideoEncode(ReadEnviron):
    # インスタンス変数を定義
    recordedId = 0              # 録画済み番組 id
    sourceVideoFileId = 0       # ビデオファイル id
    parentDir = None            # 親ディレクトリ名 config recorded の name, isSaveSameDirectory が false の場合は必須
    directory = None            # 親ディレクトリ以下のディレクトリ設定
    isSaveSameDirectory = False # ソースビデオファイルと同じ場所に保存するか(初期値: False)
    mode = None                 # エンコードプリセット名 config encode の name
    removeOriginal = False      # 元ファイルを削除するか(初期値: False)
    
    def __init__(self) -> None:
        super().__init__()  # ReadEnvironからインスタンス変数を継承
        pass
    
    def getSourceVideoFileId(self, recordedId: int):
        """録画済み番組 idからビデオファイル idを検索

        Args:
            recordedId (int): 録画済み番組 id
        """
        headers = {'content-type': 'application/json'}
        res = requests.get(self.epgstationUrl + f"/api/recorded/{ str(recordedId) }?isHalfWidth=true", headers=headers)
        
        videoFileId = int(res.json()['videoFiles'][0]['id'])
        logging.debug("videoFileId:" + str(videoFileId))
        
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
        parentDir = self.config["epgstationEncode"]["parentDir"] or ""
        directory = self.config["epgstationEncode"]["directory"] or ""
        isSaveSameDirectory = self.config["epgstationEncode"]["isSaveSameDirectory"]
        mode = self.config["epgstationEncode"]["mode"]
        removeOriginal = self.config["epgstationEncode"]["removeOriginal"]
            
        data = {
            'recordedId': self.recordedId,                      # 必須
            'sourceVideoFileId': self.sourceVideoFileId,        # 必須
            'parentDir': parentDir,
            'directory': directory,
            'isSaveSameDirectory': isSaveSameDirectory,
            'mode': mode,                                       # 必須
            'removeOriginal': removeOriginal                    # 必須
        }
        
        logging.debug(str(data))
        headers = {'content-type': 'application/json'}
        res = requests.post(url=self.epgstationUrl + '/api/encode', data=json.dumps(data), headers= headers)
        
        logging.debug(json.dumps(data))
        logging.debug(str(res.status_code) + '\n' + res.text)
        
        return

if __name__ == '__main__':
    try:
        start = ReadEnviron() # インスタンス生成
        start.readTsProgram()
        recordedId = start.createRecData()
        start.uploadTsVideoFile(recordedId=recordedId)
        
        if start.config["epgstationEncode"]["runEncode"] == False:
            sys.exit()
        else:
            logging.info("エンコードを開始します。")
    except Exception as e:
        logging.exception(e) # エラーが発生したらファイルに書き込み
    
    # エンコードタスクを実行
    try:
        encode = VideoEncode()
        encode.getSourceVideoFileId(recordedId=recordedId)
        encode.addVideoEncode() # エンコードタスクに追加
    
    except Exception as e:
        logging.exception(e)

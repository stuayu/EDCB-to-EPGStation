import os
import datetime
import re
import requests
import logging
import json
import time

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
        
        self.epgstationUrl = "http://localhost:8888" # EPGStationのURL
        self.parentDirectoryName = "TS2" # EPGStationのストレージに表示されている名前
        self.viewName = "TS" # EPGStationの再生ボタンで表示する名前
        self.fileType = "ts" # ts か recorded で指定する
        
        # -----------------------------------------------------------------------------
        
    def readTsProgram(self):
        """
        番組情報ファイルから必要なデータを取り出す
        """
        logging.debug("start readTsProgram")
        with open(self.tsProgramFilePath, mode="+r", encoding="utf-8") as f:
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
        logging.debug("start uploadTsVideoFile")
        
        headers = {'accept': 'application/json'}
        data = {'recordedId': recordedId, 'parentDirectoryName': self.parentDirectoryName, 'viewName': self.viewName, 'fileType': self.fileType}
        files = {'file': (open(self.tsFilePath, 'rb'))}
        
        logging.debug(str(data))
        res = requests.post(url=self.epgstationUrl + "/api/videos/upload", headers=headers, data=data, files=files)
        
        logging.debug(str(res.status_code))
        logging.debug(str(res.text))
        return
    
if __name__ == '__main__':
    try:
        start = ReadEnviron() # インスタンス生成
        start.readTsProgram()
        recordedId = start.createRecData()
        start.uploadTsVideoFile(recordedId=recordedId)
    except Exception as e:
        logging.exception(e) # エラーが発生したらファイルに書き込み

@echo off

rem // �E�C���h�E���\���ɂ���
rem _EDCBX_HIDE_

rem // �p�����[�^�����ϐ��ɓn��
rem // �������邱�Ƃ� Python ���ł����ϐ����Q�Ƃł���
rem _EDCBX_DIRECT_

rem �f�o�b�O�p�ɉ��z�ϐ�
setlocal
set FILEPATH=F:\\EDCB\\202306111935_�_�[�E�B��������!�u���t����Ɛ�����!�o�邩�H�����A�E�E�T�M�v_NHK����1�E����.ts
set FILENAME=202306111935_�_�[�E�B��������!�u���t����Ɛ�����!�o�邩�H�����A�E�E�T�M�v_NHK����1�E����
set TITLE=�_�[�E�B���������I�u���t����Ɛ������I�o�邩�H�����A�E�E�T�M�v[��]
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


rem EDCB�Ř^�悵�����ʂ�EPGStation�̘^��ς݂Ƃ��ď�������
python.exe %~dp0\edcb_to_epgstation_ts.py
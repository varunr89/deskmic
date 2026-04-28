@echo off
REM deskmic-index-and-sync.cmd — Run index then push DB to Azure Blob
REM Called by Windows Task Scheduler (deskmic-reindex task)
REM Reads blob config from .env file next to deskmic.exe

setlocal enabledelayedexpansion

set "DESKMIC_DIR=%~dp0"
set "ENV_FILE=%DESKMIC_DIR%..\deskmic\.env"
if not exist "%ENV_FILE%" set "ENV_FILE=%DESKMIC_DIR%.env"

REM Parse .env file for blob vars
if exist "%ENV_FILE%" (
    for /f "usebackq tokens=1,* delims==" %%A in ("%ENV_FILE%") do (
        set "line=%%A"
        if not "!line:~0,1!"=="#" (
            set "%%A=%%B"
        )
    )
)

echo [%date% %time%] Starting index...
"%DESKMIC_DIR%deskmic.exe" index

if not defined DESKMIC_BLOB_ACCOUNT (
    echo [%date% %time%] No blob config found, skipping push.
    goto :eof
)
if not defined DESKMIC_BLOB_SAS_RW (
    echo [%date% %time%] No blob SAS token found, skipping push.
    goto :eof
)

echo [%date% %time%] Pushing DB to blob...
set "BLOB_URL=https://%DESKMIC_BLOB_ACCOUNT%.blob.core.windows.net/%DESKMIC_BLOB_CONTAINER%/deskmic-search.db?%DESKMIC_BLOB_SAS_RW%"

curl -sS -X PUT -H "x-ms-blob-type: BlockBlob" -H "x-ms-version: 2024-11-04" --data-binary "@%DESKMIC_OUTPUT_DIR%\deskmic-search.db" "%BLOB_URL%"

if %ERRORLEVEL% EQU 0 (
    echo [%date% %time%] Push complete.
) else (
    echo [%date% %time%] Push failed with error %ERRORLEVEL%
)

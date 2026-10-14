@echo off
rem ---------------------------------------------------------------------------
rem Probe the Blink/WebView2 behaviour for <img> EXIF orientation.
rem
rem WebView2 and Edge ship the same Blink engine, so a headless Edge run is a
rem faithful stand-in for the Tauri webview. The page (webview2-probe.html)
rem prints one line per fixture; we just dump the DOM and grep the result.
rem
rem Usage: scripts\probe-webview2-orientation.cmd
rem ---------------------------------------------------------------------------
setlocal

set EDGE="C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
if not exist %EDGE% set EDGE="C:\Program Files\Microsoft\Edge\Application\msedge.exe"
if not exist %EDGE% (
  echo EDGE_NOT_FOUND - install Edge or edit EDGE in this script
  exit /b 1
)

set ROOT=%~dp0..\
set PAGE=file:///%ROOT:\=/%scripts/webview2-probe.html
set PROFILE=%TEMP%\lumina-probe-profile

echo PAGE=%PAGE%
rem --allow-file-access-from-files: without it a file:// page gets an opaque
rem origin, canvas.getImageData() throws SecurityError, and we can only read
rem naturalWidth/Height. With it we can also inspect the rendered pixels.
%EDGE% --headless=new --disable-gpu --no-first-run --no-default-browser-check ^
  --allow-file-access-from-files --user-data-dir="%PROFILE%" ^
  --virtual-time-budget=8000 --dump-dom "%PAGE%" > "%TEMP%\lumina-probe-dom.txt" 2>&1

echo --- PROBE RESULT ---
findstr /C:"FIXTURE " "%TEMP%\lumina-probe-dom.txt"
echo --- (raw DOM kept at %TEMP%\lumina-probe-dom.txt) ---
exit /b 0

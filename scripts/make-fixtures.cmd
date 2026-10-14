@echo off
setlocal
set OUT=src-tauri\tests\fixtures
if not exist "%OUT%" md "%OUT%"

magick -size 96x64 gradient:red-blue "%OUT%\grad.png" || goto :fail
magick "%OUT%\grad.png" "%OUT%\grad.jpg" || goto :fail
magick "%OUT%\grad.jpg" "%OUT%\grad.bmp" || goto :fail
magick "%OUT%\grad.jpg" "%OUT%\grad.tiff" || goto :fail
magick "%OUT%\grad.jpg" "%OUT%\grad.webp" || goto :fail
magick "%OUT%\grad.png" -resize 96x96! -define icon:auto-resize=96,48,32,16 "%OUT%\grad.ico" || goto :fail

rem --- animated GIF (3 frames) ---
magick -size 48x48 xc:red -size 48x48 xc:green -size 48x48 xc:blue -delay 20 -loop 0 "%OUT%\anim.gif" || goto :fail
rem --- animated WebP (3 frames) ---
magick -size 48x48 xc:red -size 48x48 xc:green -size 48x48 xc:blue -delay 20 -loop 0 "%OUT%\anim.webp" || goto :fail
rem --- multi-page TIFF (3 pages, 8-bit continuous-tone RGB) ---
magick -size 48x48 gradient:red-blue -size 48x48 gradient:green-black -size 48x48 gradient:black-blue -depth 8 -compress LZW "%OUT%\multipage.tiff" || goto :fail
rem --- multi-page palette TIFF (8-bit indexed colour expansion path) ---
magick -size 48x48 gradient:red-blue -size 48x48 gradient:green-black -size 48x48 gradient:black-blue -colors 64 -type palette -depth 8 -compress none "%OUT%\palette.tiff" || goto :fail

rem --- AVIF (decoder evaluation) ---
magick "%OUT%\grad.jpg" "%OUT%\grad.avif" || goto :fail

rem --- EXIF fixture (needs Python + Pillow; skipped if python is absent) ---
where python >nul 2>nul && python scripts\make-exif.py

echo FIXTURES_OK
exit /b 0

:fail
echo FIXTURES_FAILED errorlevel=%errorlevel%
exit /b 1

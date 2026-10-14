"""Generate a real JPEG fixture carrying rich EXIF (camera / time / GPS)."""
from PIL import Image
from PIL.TiffImagePlugin import IFDRational as R

SRC = "src-tauri/tests/fixtures/grad.jpg"
DST = "src-tauri/tests/fixtures/photo.jpg"

img = Image.open(SRC).convert("RGB")

exif = Image.Exif()
exif[0x010F] = "LuminaCam"                 # Make
exif[0x0110] = "LC-1000"                   # Model
exif[0x0131] = "LuminaTest 1.0"            # Software
exif[0x013B] = "DevTeam"                   # Artist
exif[0x8298] = "Copyright Lumina"          # Copyright
exif[0x0132] = "2024:05:06 07:08:09"       # DateTime
exif[0x9003] = "2024:05:06 07:08:09"       # DateTimeOriginal
exif[0x0112] = 6                           # Orientation (rotate 90 CW)
exif[0x829D] = R(28, 10)                   # FNumber  f/2.8
exif[0x829A] = R(1, 250)                   # ExposureTime 1/250
exif[0x8827] = 200                         # ISOSpeedRatings
exif[0x920A] = R(35, 1)                    # FocalLength 35mm
exif[0xA434] = "LC-1000-Zoom 18-55"        # LensModel

gps = exif.get_ifd(0x8825)
gps[1] = "N"                               # GPSLatitudeRef
gps[2] = (R(40, 1), R(26, 1), R(46, 1))    # GPSLatitude  40 26 46
gps[3] = "E"                               # GPSLongitudeRef
gps[4] = (R(116, 1), R(23, 1), R(29, 1))   # GPSLongitude 116 23 29
gps[5] = 0                                 # GPSAltitudeRef (above sea level)
gps[6] = R(48, 1)                          # GPSAltitude 48 m
gps[7] = (R(12, 1), R(34, 1), R(56, 1))    # GPSTimeStamp
gps[29] = "2024:05:06"                     # GPSDateStamp

img.save(DST, exif=exif, quality=92)

# --- verify by reading back ---
back = Image.open(DST)
e = back.getexif()
print("Make         =", e.get(0x010F))
print("Model        =", e.get(0x0110))
print("DateTimeOrig =", e.get(0x9003))
print("Orientation  =", e.get(0x0112))
g = e.get_ifd(0x8825)
print("GPSLatRef    =", g.get(1), "GPSLat =", g.get(2))
print("GPSLonRef    =", g.get(3), "GPSLon =", g.get(4))
print("GPSAltRef    =", g.get(5), "GPSAlt =", g.get(6))
print("LensModel    =", e.get(0xA434))
print("EXIF_OK")

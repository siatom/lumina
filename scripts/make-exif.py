"""Generate the JPEG fixtures for Lumina.

Two families:

* `photo.jpg`          — rich EXIF (camera / lens / time / GPS), orientation 6.
* `orient{1,6,8}.jpg`  — orientation probes. The *stored* pixels are 96x64
  landscape with an unmistakable RED block in the stored top-left corner and
  dark blue everywhere else, so the applied orientation can be asserted by
  looking at which displayed corner ends up red:

      orientation 1 (none)          -> red at top-left
      orientation 6 (rotate 90 CW)  -> red at top-right
      orientation 8 (rotate 270 CW) -> red at bottom-left

Run: python scripts/make-exif.py
"""
from PIL import Image, ImageDraw
from PIL.TiffImagePlugin import IFDRational as R

FIX = "src-tauri/tests/fixtures"

# Stored (pre-orientation) geometry of the probe fixtures.
PROBE_W, PROBE_H = 96, 64
MARK_W, MARK_H = 24, 16
DARK = (32, 32, 64)
RED = (255, 0, 0)


def build_photo() -> None:
    """Rich-EXIF fixture, reused by the EXIF metadata tests."""
    img = Image.open(f"{FIX}/grad.jpg").convert("RGB")

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

    out = f"{FIX}/photo.jpg"
    img.save(out, exif=exif, quality=92)

    back = Image.open(out).getexif()
    print("[photo.jpg]")
    print("  Make/Model   =", back.get(0x010F), "/", back.get(0x0110))
    print("  DateTimeOrig =", back.get(0x9003))
    print("  Orientation  =", back.get(0x0112))
    g = back.get_ifd(0x8825)
    print("  GPS          =", g.get(1), g.get(2), "/", g.get(3), g.get(4), "alt", g.get(6))
    print("  LensModel    =", back.get(0xA434))


def build_orientation_probe(orientation: int) -> None:
    """Marker fixture whose stored top-left corner is red (JPEG + TIFF)."""
    img = Image.new("RGB", (PROBE_W, PROBE_H), DARK)
    ImageDraw.Draw(img).rectangle([0, 0, MARK_W - 1, MARK_H - 1], fill=RED)

    exif = Image.Exif()
    exif[0x0112] = orientation

    jpg = f"{FIX}/orient{orientation}.jpg"
    img.save(jpg, exif=exif, quality=95)

    # TIFF keeps Orientation (tag 274) as a first-class baseline tag, which is
    # the path exercised by read_for_display's TIFF -> PNG transcode.
    tif = f"{FIX}/orient{orientation}.tiff"
    img.save(tif, tiffinfo={274: orientation})

    back_jpg = Image.open(jpg)
    print(
        f"[orient{orientation}.jpg ] stored={back_jpg.size} orientation={back_jpg.getexif().get(0x0112)}"
    )
    if orientation != 1:
        print(f"[orient{orientation}.tiff] stored={Image.open(tif).size}")


if __name__ == "__main__":
    build_photo()
    for o in (1, 6, 8):
        build_orientation_probe(o)
    print("EXIF_OK")

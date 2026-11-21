# Picture fixtures

Tiny pictures for `src/picture.rs` tests. Made on 2026-11-21 from synthetic images (no third-party
content, so no licence applies; use them freely) with ImageMagick 7 and libjxl's `cjxl`:

```
magick -size 24x16 xc:'#ff0000' -fill '#0000ff' -draw 'rectangle 12,0 23,15' two.png
cjxl two.png two-24x16.jxl -d 0 -e 9                       # lossless: red left half, blue right half
magick -size 48x32 gradient:'#ff3030-#3050ff' grad.png
cjxl grad.png gradient-48x32.jxl -d 1 -e 9                 # lossy: red at the top to blue at the bottom
magick -size 16x16 gradient:white-black -colorspace Gray gray.png
cjxl gray.png gray-16x16.jxl -d 0                          # greyscale, white to black
magick -size 16x16 xc:none -fill '#ff0000' -draw 'rectangle 0,0 7,15' alpha.png
cjxl alpha.png alpha-16x16.jxl -d 0                        # alpha: left half red, right half transparent
```

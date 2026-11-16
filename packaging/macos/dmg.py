# dmgbuild's settings for the disk image dmg.sh builds: the app beside a link
# to Applications, in a window that shows only the two, ready to drag.
import os.path

app = defines["app"]  # noqa: F821, dmgbuild defines it
_name = os.path.basename(app)

format = "ULFO"
files = [app]
symlinks = {"Applications": "/Applications"}
icon = os.path.join(app, "Contents", "Resources", "live.kixel.studio.icns")

default_view = "icon-view"
window_rect = ((200, 200), (540, 340))
icon_size = 128
text_size = 13
icon_locations = {_name: (140, 150), "Applications": (400, 150)}

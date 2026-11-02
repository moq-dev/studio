# MoQ Studio

`moq-studio` is a native streaming studio for Linux, macOS, and Windows. It
composes cameras, screens, windows, images, colors, and broadcasts from a relay
into scenes on the GPU, mixes their audio, and publishes the result to a
[MoQ](https://github.com/moq-dev/moq) relay. It is early and not packaged yet,
so build it from source. The moq crates come from the `moq` submodule:

```bash
git clone --recurse-submodules https://github.com/moq-dev/studio
cd studio
cargo run --release -p moq-studio --features capture,openh264 -- \
    show.toml --connect https://cdn.moq.dev/anon --broadcast my-show
```

- `show.toml` is the scene collection. It is created on the first save.
- Without `--connect`, the studio composes and previews but publishes nothing,
  and broadcast sources stay offline.
- `capture` adds camera, display, window, microphone, and system-audio sources.
  On Linux it links ALSA; add `pipewire` for PipeWire cameras. `openh264`
  is the software H.264 fallback for a machine without a hardware encoder.
- On Linux, display and window sources capture X11 and XWayland only; a
  Wayland screen or window cannot be picked yet.
- The `--connect-*`, `--quic-*`, and `--log-level` flags are
  [moq-cli's](https://doc.moq.dev/bin/cli); see `moq-studio --help`.

## Panels

Every panel is a tab you can drag, split, or stack.

| Panel | Use it to |
| --- | --- |
| Preview, Program | See the scene you are editing and the one that is live. **Cut** puts the preview on program at once; **Fade** blends over the collection's fade time. |
| Scenes | Pick the scene in preview, add one, or remove one. |
| History | See every edit, and undo or redo back to any of them. |
| Items | Place, show, hide, and order what the preview scene or the top layer draws, back to front. Select an item to filter or copy it. |
| Sources | Add a source by hand, see whether each is live and where it is shown, clone one, and remove one. Select a source to filter it. |
| Devices | List this machine's cameras, displays, windows, and audio inputs, and add one as a source. |
| Filters | Add, order, and set the filters of the selected source or item, and copy or paste them. |
| Audio | Set each audio source's gain and mute, and watch its meter and the program's. |
| Output | See the relay connection, whether anyone is watching, and the encoder's work. |

A new source, from the Sources or Devices panel, is placed over whatever the
Items panel shows. The **top layer** is drawn over every scene, in preview and
on program, and stays on through a cut or fade: a downstream keyer for a logo
or a lower third.

Items showing the same source are **linked**: one capture, with the source's
filters, wherever it is shown. A **clone** is a new source showing another's
capture with filters of its own, so a camera can be shown faded in one scene
and plain in another while being opened once. An item's own filters apply
after its source's.

**Edit > Undo** (Ctrl+Z) and **Redo** (Ctrl+Shift+Z or Ctrl+Y) step through the
history, and a drag undoes as one step. **Copy** (Ctrl+C) takes the selected
item with the source it shows; **Paste** (Ctrl+V) adds it over whatever the
Items panel shows, in this studio or another. Pasted where the same source
exists under the same name, the item is linked to it; otherwise the source
comes along, renamed if its name is taken. Copied filters paste onto the
selected source or item, after its own. The clipboard holds text in the scene
file's format.

A broadcast source subscribes through the same relay connection, plays the
best video and audio its catalog lists, switches when the catalog changes, and
comes back on its own when the publisher restarts. Audio from every source is mixed into the program whether
or not its scene is live; mute a source to leave it out.

## Scene file

The collection is TOML, so it diffs and reviews like code. It names devices,
files, and broadcast paths, never a relay URL or token. Check its image paths
and device IDs before sharing it, since they can name your home directory or
hardware. A file from another format version fails to load rather than being
guessed at.

```toml
version = 1

[canvas]
width = 1920
height = 1080
framerate = 30

[transition]
fade = "300ms"

[sources.camera]
kind = "camera"

[sources.logo]
kind = "image"
path = "logo.png" # relative to the scene file

[sources.guest]
kind = "broadcast"
path = "room/guest"

[audio.guest]
gain = 0.5

[sources.camera-faded]
kind = "clone"
source = "camera"

[video.camera-faded]
filters = [{ kind = "opacity", amount = 0.5 }]

[[scenes]]
name = "Main"
items = [
    { source = "camera", rect = { x = 0, y = 0, width = 1920, height = 1080 } },
    { source = "guest", rect = { x = 1280, y = 720, width = 640, height = 360 }, filters = [{ kind = "opacity", amount = 0.8 }] },
]

[[scenes]]
name = "Break"
items = [
    { source = "camera-faded", rect = { x = 0, y = 0, width = 1920, height = 1080 } },
]

[[top]]
source = "logo"
rect = { x = 1600, y = 40, width = 280, height = 120 }
```

Source kinds are `camera`, `display`, and `microphone` (each with an optional
`id`, the default device without one), `window` (with an `id`), `system_audio`
(macOS), `image` (PNG or JPEG), `color` (`#rrggbb` or `#rrggbbaa`), and
`broadcast`, and `clone` (with the `source` it shows). The Devices panel fills
in ids. `[video.<source>]` lists a source's filters, and an item's `filters`
its own; the one filter so far is `opacity`, from 0 to 1, as in CSS. `[[top]]`
items make up the top layer. **File > Save** (Ctrl+S) writes the file and
**File > Reload** reads it back; a new canvas size or frame rate takes a
restart.

Saving also writes the last 100 edits beside the scene file, as
`show.history.toml` for `show.toml`, and opening the scene file restores them,
so undo reaches back past a restart. An edit that no longer matches the scene
file, such as after editing it by hand, is dropped with every older one, and
the status bar says how many. A history file that cannot be read stops the
studio, naming the file; delete it to start a new history. The history holds
whatever the scene file held, including removed sources, so check it too
before sharing.

## Output

The program publishes as one H.264 rendition at the canvas size and frame rate,
with Opus stereo audio, under the `--broadcast` path. Timestamps come from the
studio's own clock, so a source that stalls repeats its last picture instead of
holding up the stream.

The program encodes whether or not anyone watches, so a viewer who arrives
late can fetch the recent past. `--on-demand` copies and encodes the video only
while someone watches it, which saves that work but leaves nothing from before
the first viewer; audio encodes either way. A hardware encoder that reads
the GPU (NVENC on Linux, VideoToolbox on macOS, Media Foundation on Windows)
takes each frame there when it can read the GPU the studio draws on. When it
cannot, such as NVENC while the studio draws on a laptop's integrated GPU, the
studio passes over it for an encoder that reads each frame back to the CPU
(the opt-in `vaapi` or `openh264` features), which the Output panel counts.
Frames the encoder cannot keep up with are dropped rather than delaying the
stream.

Sources make the opposite trip. A frame already on the GPU is drawn where it
lies: PipeWire camera and VA-API decode DMA-BUFs on Linux, given a Vulkan
driver that imports them, and NV12 capture and VideoToolbox decode surfaces on
macOS. Every other source, every source on Windows or on a GPU that cannot
import, and every still image delivers its frames from the CPU, which the
compositor uploads.

## Development

Use the Nix dev shell (`direnv allow`, or `nix develop --command ...`) so
tooling matches CI, then:

```bash
just check   # Lint everything, including the quest tree
just test    # Run the tests
just ci      # Both, as CI runs them
just fix     # Auto-fix formatting
```

`rs/moq-scene` is the UI-free compositor and `rs/moq-studio` the app. See
[AGENTS.md](AGENTS.md) and [CONTRIBUTING.md](CONTRIBUTING.md), and the
[quest tree](quest/README.md) for planned work.

## License

Licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

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
| Items | Place, show, hide, and order the sources the preview scene draws, back to front. |
| Sources | Add a source by hand, see whether each and its own track are live, and remove one. |
| Devices | List this machine's cameras, displays, windows, and audio inputs, and add one as a source. |
| Audio | Set each audio source's gain and mute, and watch its meter and the program's. |
| Output | See the broadcast path, whether the program is encoding, and the encoder's work. |
| Health | See how the stream reaches viewers, track by track, and the cause of each problem. |

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

[[scenes]]
name = "Main"
items = [
    { source = "camera", rect = { x = 0, y = 0, width = 1920, height = 1080 } },
    { source = "logo", rect = { x = 1600, y = 40, width = 280, height = 120 } },
]
```

Source kinds are `camera`, `display`, and `microphone` (each with an optional
`id`, the default device without one), `window` (with an `id`), `system_audio`
(macOS), `image` (PNG or JPEG), `color` (`#rrggbb` or `#rrggbbaa`), and
`broadcast`. The Devices panel fills in ids. **File > Save** (Ctrl+S) writes
the file and **File > Reload** reads it back; a new canvas size or frame rate
takes a restart.

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

### Source tracks

Each camera, display, window, image, and color also publishes as its own track
in the same broadcast, so a viewer can compose the scene itself. The catalog's
`video` and `audio` sections stay the program's; the sources are listed under
the `dev.moq.studio` section's `sources`, by source name, each as a hang
catalog of its own on the program's clock. This listing is provisional until
the hang draft specifies source listings:

```json
{
  "video": { "renditions": { "0.avc3": { "codec": "avc3.42c01f", "...": "..." } } },
  "audio": { "renditions": { "audio": { "codec": "opus", "...": "..." } } },
  "dev.moq.studio": {
    "sources": {
      "camera": { "video": { "renditions": { "1.avc3": { "...": "..." } } } },
      "logo": { "binary": { "tracks": { "2.png": { "mode": "snapshot", "mime": "image/png" } } } }
    }
  }
}
```

A captured source encodes as H.264 for as long as it is in the collection,
whether or not anyone watches, so a late viewer can fetch its recent past; the
relay uploads only what someone subscribes to. A capture that changes size or
color space moves to a new track, which its catalog entry follows. Images and
colors publish once, as the image file and as a one-pixel PNG. Removing or
editing a source ends its track. Microphones and system audio do not publish
tracks of their own yet, and a broadcast source is already its publisher's.

Every captured source opens an encode session as it starts, and consumer GPUs
allow only a few hardware sessions at once. When the program encodes on
hardware, so do the sources, and a session the GPU refuses fails that source's
track with the encoder's reason in the Sources panel rather than falling back
to software. The source keeps drawing in the studio's own scenes. When the
program encodes in software, a source takes hardware where it can and software
otherwise.

### Source frames

Sources make the opposite trip. A frame already on the GPU is drawn where it
lies: PipeWire camera and VA-API decode DMA-BUFs on Linux, given a Vulkan
driver that imports them, and NV12 capture and VideoToolbox decode surfaces on
macOS. Every other source, every source on Windows or on a GPU that cannot
import, and every still image delivers its frames from the CPU, which the
compositor uploads.

## Health

The Health panel grades the stream every second on the thresholds moq's
`hang::health` shares with the rest of the stack, and lists each problem
with its cause in plain words: the relay is unreachable, the uplink is
congested or losing packets, the relay is far away, the encoder is behind or
stuck, a camera or broadcast source on program stalled, or viewers are late,
stalling, or cannot decode a track. Below that it shows:

- **Uplink**: the round trip to the relay, the bandwidth the connection
  estimates it can carry, the media every track writes, and the share lost and
  resent. Loss and headroom are graded only while someone watches, since the
  relay pulls nothing otherwise.
- **Watching**: the sessions subscribed to the broadcast, from the relay's
  `.stats` broadcasts. A relay shares those only with a connection allowed to
  read them, so most show "unknown". In a cluster, a relay forwarding the
  stream to another counts as one session.
- **Tracks**: each track's bitrate and frame rate, and, from viewers that
  report, the lowest bitrate any received, its worst grade, and the latency
  from the program clock to the slowest viewer. Latency compares the two
  hosts' wall clocks, so keep both synced.
- **Viewers**: each viewer that reports, with its round trip, latency, and
  playout delay. The studio reads 64 at most, a sample the grades rest on: it
  counts the rest, and a reader one frees goes to the next viewer to arrive.

The catalog asks viewers to report under `<broadcast>/viewers/`: a viewer
publishes `<broadcast>/viewers/<name>.echo`, as `moq export --echo <name>`
does. The relay decides who may publish there, as for any broadcast.

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

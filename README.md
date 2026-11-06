# MoQ Studio

`moq-studio` is a native streaming studio for Linux, macOS, and Windows. It
composes cameras, screens, windows, images, colors, web pages, HTML and CSS
overlays, and broadcasts from a relay into scenes on the GPU, mixes their audio, and publishes the result to a
[MoQ](https://github.com/moq-dev/moq) relay. It is early and not packaged yet,
so build it from source. The moq crates come from the `moq` submodule:

```bash
git clone --recurse-submodules https://github.com/moq-dev/studio
cd studio
cargo run --release -p moq-studio --features capture,openh264 -- \
    show.toml --connect https://cdn.moq.dev/anon --broadcast my-show
```

- `show.toml` is the scene collection. If it does not exist yet,
  [guided setup](#first-launch) proposes one and creates it. Changes are saved
  to it a second after you stop editing, with no save step.
- `--recordings <dir>` is where recordings are saved, by default beside
  `show.toml`.
- Without `--connect`, the studio composes, previews, and records but
  publishes nothing, and broadcast sources stay offline.
- `--failover <url>` publishes the same broadcast to a second relay under the
  same broadcast instance, so a viewer linked to both resumes mid-group from
  it when the first fails. Both relays must speak the one `--connect-version`,
  which must carry instances (`moq-lite-07-wip` today, which a relay
  accepts only when configured to), or the studio refuses to start; it also
  refuses a fixed `--connect-bind` port, since each relay's connection binds
  its own. Its credential rides its URL's `?jwt=`; `--connect-token` goes to
  the first relay only. The stream's tracks claim their bitrate on the first
  relay's link alone, so only that link is probed toward the stream's rate,
  and the video follows that link's estimate alone.
- Quitting while live ends every track after its last pictures and sound,
  then closes each relay session once the relay has them, waiting up to about
  a second, so viewers and recordings on the relay see the broadcast end
  rather than cut off.
- `capture` adds camera, display, window, microphone, and system-audio
  sources, and listening to a mix. On Linux it links ALSA; add `pipewire` for PipeWire cameras. `openh264`
  is the software H.264 fallback for a machine without a hardware encoder.
- `cef` adds browser sources, on Linux only for now. The build takes CEF from
  `CEF_PATH`, which the dev shell sets, or else downloads it, and the binary
  loads `libcef.so` from beside itself or from that `CEF_PATH`.
- On Linux, display and window sources capture X11 and XWayland only; a
  Wayland screen or window cannot be picked yet.
- On macOS, the studio is not an app bundle yet, so the camera, microphone,
  and Screen & System Audio Recording permissions are asked of, and granted
  to, the app that launched it, such as your terminal. Display, window, and
  system-audio sources need the last, granted in System Settings > Privacy &
  Security.
- The `--connect-*`, `--quic-*`, and `--log-level` flags are
  [moq-cli's](https://doc.moq.dev/bin/cli); see `moq-studio --help`.

## First launch

Opening a scene file that does not exist yet starts guided setup instead of
the studio. It runs three checks, and a check that fails shows its reason:

- **Devices:** the cameras, displays, and audio inputs this machine has.
- **Encoders:** for each canvas setup can propose (1080p and 720p at 60 or 30
  fps, and 360p at 30 fps), the encoder the program would pick, whether it
  reads the GPU, the bitrate it encodes at, and how long a frame takes over a
  short burst of moving frames. An encoder keeps up when a frame takes at most
  three quarters of the frame period, leaving the rest for sources and the
  compositor.
- **Uplink:** with `--connect`, setup publishes a probe at the `--broadcast`
  path for about six seconds and subscribes to it through the relay, so the
  relay pulls it. The congestion controller's send-rate estimate is the
  measurement. With `--failover`, only the `--connect` relay is measured,
  since only its link is probed toward the stream's rate. A connection that
  reports no estimate, such as WebSocket, fails the check.

Setup then proposes a scene named Main and a canvas, explaining each choice.
The scene has the first display filling the canvas, the first camera in the
bottom-right corner over it (or filling the canvas alone), and the first audio
input. Swap any of them for another device or leave it out. The canvas is the
largest one whose encoder keeps up and whose video and audio fit in two thirds
of the measured uplink, the most the Health panel grades healthy. Without a
measurement, it is the default 1080p at 30 fps. If no encoder keeps up, it is
the canvas closest to keeping up.

Nothing is written until you start the studio, so quitting during setup runs
it again on the next launch. **Start with this scene** and **Start with an
empty scene** both create the scene file at once, the second with the chosen
canvas and no sources. The proposed scene is the first edit in the history, so
undoing it leaves that empty scene.

Setup writes an ordinary scene file. Change its sources and scenes in their
panels, and its canvas in the Output panel. The Health panel shows how the
uplink holds up once you are live.

## Panels

Every panel is a tab you can drag, split, or stack.

| Panel | Use it to |
| --- | --- |
| Preview, Program | See the scene you are editing and the one that is live, and drag items in the preview to place them. With more than one canvas, pick the canvas they show, and that the Items panel edits, beside the preview. **Cut** puts the preview on program at once; **Transition** takes it there by the transition picked beside it, over the time set there, or for a stinger by the media source and cut point set there (see [Transitions](#transitions)). **Interact** passes the mouse over the preview to the browser source under it instead, and the keyboard to the one last clicked. |
| Scenes | Pick the scene in preview, and add, rename (double-click), reorder, or remove one. A scene on program, or still leaving it in a transition, cannot be removed, even by an undo, until program cuts away from it. |
| History | See every edit, and undo or redo back to any of them. |
| Items | Place, show, hide, and order what the preview scene or the top layer draws on the canvas shown, back to front. Select an item to filter or copy it. |
| Sources | Add a source by hand, see whether each and its own track are live and where it is shown, clone one, and remove one. Select a source to filter it. A camera or broadcast without a new picture, or a microphone or system audio without new sound, for a second reads as stalled. A browser source shows whether its page is copied on the GPU or, as a fallback, uploaded from memory (CPU), in place of a track, and **Reload** restarts its page. A media file reads as ended once it plays to its end without looping, **Play** plays it from its start, **Stop** stops it at once, blank and silent, until played again, and **Reload** reads the file again. |
| Devices | List this machine's cameras, displays, windows, and audio inputs, and add one as a source. |
| Guests | Invite a guest, show their join link as a QR code, see who joined, and take a guest out of every scene. |
| Filters | Crop the selected source or item, add, order, and set its filters, set an item's blend mode, and copy or paste filters. |
| Keyframes | Animate the selected item: add, time, ease, and remove its keyframes, and set how far each moves, resizes, crops, and filters it (see [Keyframes](#keyframes)). **Play** plays the preview scene's keyframes again. |
| Audio | Add, rename (double-click), and remove mixes, pick the one on program and the one to listen to and on which output, and set each audio source's gain, mute, delay, and the mixes it feeds, with a meter on every mix and source. |
| Output | Start and stop recording, pick the mix it records, set the canvas size and frame rate, add, resize, and remove further canvases, and see the broadcast path, whether the program is encoding, the encoder sessions open, and each canvas's renditions' encoder work. |
| Health | See how the stream reaches viewers, relay by relay and track by track, and the cause of each problem, and which recordings a crash left unfinished were repaired at launch. |
| Replay | Replay the last moments of the program or a source in a replay source, slowed down if you like, or save them as a clip. |

A new source, from the Sources or Devices panel, is placed over whatever the
Items panel shows. The **top layer** is drawn over every scene, in preview and
on program, and stays on through a cut or transition: a downstream keyer for a
logo or a lower third.

### Canvases

Beside the main canvas, the studio can draw further canvases at once, such as
a 1080x1920 portrait one for a vertical stream beside a 1920x1080 landscape
main canvas. Add one by name in the Output panel. Each scene has its own
layout on each canvas, and each canvas its own top layer; a scene without a
layout on a canvas shows only that canvas's top layer there. Every canvas
shares the main canvas's frame rate, and the canvases are linked: one scene is
in preview and one on program on all of them, and a cut or transition takes
every canvas together. An item's keyframes play on every canvas from when its
scene goes on, as on the main one. A source captures or decodes once however many canvases
show it, and a canvas draws only the items it places.

Each further canvas publishes a broadcast of its own beneath the main one, at
`<--broadcast>/<canvas name>`, with renditions and encoders of its own (see
[Output](#output)). Its catalog lists the main broadcast's audio rather than
encoding it again. Removing a canvas ends its broadcast, and removes its top
layer and every scene's layout on it.

### Transitions

| Transition | What it does |
| --- | --- |
| Fade | The incoming scene fades in over the outgoing one, at an even pace. |
| Slide | The incoming scene pushes the outgoing one off the canvas, both moving left, right, up, or down. |
| Wipe | An edge sweeps across the canvas left, right, up, or down, uncovering the incoming scene behind it. |
| Move | Each item showing a source both scenes place glides from its place in the outgoing scene to its place in the incoming one, resizing, recropping, and mixing its filters' settings on the way. If its blend mode or kinds of filter differ, it changes look halfway. The rest of the outgoing scene fades out and the rest of the incoming scene fades in. Items stack as the outgoing scene has them until halfway, then as the incoming one does. A source placed more than once pairs up in order, back to front. |
| Stinger | A media source's clip plays over the program from its start, and the program switches scenes under it at the cut point, the time into the clip when it covers the canvas. It lasts as long as the clip. |

A slide, wipe, or move eases in and out. A transition is drawn on the program
clock, so the stream and a recording show the same frames of it, and a second
**Transition** while one runs starts from the scene it was heading to. A
transition set to 0 ms is a cut.

A stinger's clip fills the canvas, cropped evenly to the canvas's shape rather
than stretched, through its source's crop and filters, so a clip without an
alpha channel can be keyed. The cut point counts from the clip's first picture
or sound, however long the file takes to start decoding. Its sound joins the
mixes as its source's does. Its source waits blank and silent for the first
stinger rather than playing once when it opens, and rests ended between
stingers, which the Health panel does not count unless a scene on air places
it. A clip played to its end holds its last picture, as any media file does,
and a looping one stops, blank and silent, after one pass. A
clip that cannot play, or that is played again from the Sources panel or
reloaded while a stinger plays it, cuts at once; one stopped there ends the
stinger at once. A **Cut**, or another kind of transition, while a stinger
runs stops its clip, sound included; another stinger plays the clip again from
its start.

### Keyframes

An item's keyframes animate it: an item can fly in, fade in, or blur into
focus, and a camera can push in. Each keyframe is a time after the item's
scene goes on and how far the item is from where it rests then: moved and
resized by canvas pixels, cropped further, and drawn through filters after its
own. Between two keyframes the item eases from one to the next, linearly or
easing in, out, or (by default) both. Filters of the same kinds, in order,
mix their settings, and others change halfway. A keyframe's filters cannot be
a LUT or a mask.

Keyframes play once, from when the item's scene goes on program, and hold the
last keyframe; before the first, the item holds the first. A cut or transition
starts them again, even to the scene already on program, while a scene leaving
in a transition carries on with its own. The preview plays them from when the
scene went into preview; pick the scene again, or press **Play**, to watch them
again. They are drawn on the program clock, so the stream, a recording, and a
replay of the program show the same frames. Top-layer items have none, since
the top layer never goes on.

Dragging an item in the preview grabs it where its keyframes have it and moves
where it rests, so its whole animation moves with it.

In the preview, click an item of whatever the Items panel shows to select it,
drag it to move it, and drag its edges or corners to resize it. A drag snaps
edges and centers to the canvas's edges and center, to the other items shown,
and to the grid while **Grid** is on, drawing a guide through what it lines up
with. A moved item also snaps to the spacing of its neighbors, marking the
equal spaces. Hold Ctrl (Cmd on macOS) to drag without snapping, or turn
**Snap** off. The snap and grid settings last until the studio quits.

Items showing the same source are **linked**: one capture, with the source's
filters, wherever it is shown. A **clone** is a new source showing another's
capture with filters of its own, so a camera can be shown faded in one scene
and plain in another while being opened once. An item's own filters apply
after its source's.

**Edit > Undo** (Ctrl+Z) and **Redo** (Ctrl+Shift+Z or Ctrl+Y) step through the
history, and a drag undoes as one step; on macOS each shortcut uses Cmd in
place of Ctrl. **Copy** (Ctrl+C) takes the selected
item with the source it shows; **Paste** (Ctrl+V) adds it over whatever the
Items panel shows, in this studio or another. Pasted where the same source
exists under the same name, the item is linked to it; otherwise the source
comes along, renamed if its name is taken. Copied filters paste onto the
selected source or item, after its own. The clipboard holds text in the scene
file's format.

A broadcast source subscribes through the same relay connections. It plays its
broadcast through the first relay while that relay has a route to it, and
through the failover otherwise, so a publisher restarted behind only the
failover replaces the first relay's copy once that copy ends. It plays the
best video and audio its catalog lists, switches when the catalog changes, and
comes back on its own when the publisher restarts.

A broadcast source, a guest's included, plays on its publisher's timestamps,
so its sound and picture stay in sync through network jitter and stalls. Its
audio plays as far behind what arrives as the network's jitter needs, measured
as it goes, the same delay `moq play --delay auto` holds, up to 2 s. Each
picture waits for the sound stamped with it, and the canvas shows the last one
meanwhile. A sound stall longer than that delay pauses the picture too and
starts both again together; a picture stall leaves the sound playing. A burst
after a stall skips ahead to the delay. Without audio, the
picture plays 100 ms behind. Sources shows how far behind each broadcast
plays.

### Audio mixes

Audio from every source is mixed whether or not its scene is live. A
collection has one or more **mixes**, each the sum of the sources it is fed:
for example, a stream mix that leaves out music you may not broadcast, and a
monitor mix for your headphones. A source feeds every mix, a new one included,
until you untick that mix in its row of the Audio panel; mute a source to take
it out of every mix. **On program** picks the mix the broadcast carries, and
switching it, or editing a mix, never drops or repeats audio. A recording
holds the mix on program unless the Output panel picks another, such as one
keeping music the stream leaves out; that mix never reaches the relay, and
switching it while recording never drops or repeats audio either. The mix on
program cannot be removed; removing the one a recording picks returns the
recording to the mix on program.

**Listen** plays a mix to an audio output, from the `capture` feature's
build: the system default, or the one picked under **Listen on**, which the
studio remembers for you rather than in the scene file. Use headphones: a
microphone that hears the speakers feeds the mix back into itself.

A source's **delay** holds it back in every mix, up to 2 s, so a microphone
lines up with a camera that lags it. A broadcast's picture already follows its
sound, so its delay holds the sound back from its own picture: use it only for
a publisher whose sound leads their picture.

Each mix is advanced on the program clock, and each output reads its mix on
its own, so an output that falls behind skips ahead in its own audio rather
than holding up the mixes or another output.

A browser source renders a web page, such as an overlay, alert box, or chat
widget, at the canvas frame rate (CEF caps it at 60) over a transparent
background, and its sound joins the mixes. Every page shares one
browser profile, kept in `$XDG_DATA_HOME/moq-studio/browser`
(`~/.local/share/moq-studio/browser` by default) rather than beside the scene
file, so cookies and logins, such as an alert service's, survive a reload and
a restart of the studio. One studio at a time can use it: a second one's
browser sources fail saying so. Each page renders in a process of its own, so
a page that crashes fails only its own source, with the reason, while the
stream carries on. A crash of the browser itself fails every browser source.
Either shows at once, since the browser's processes dump no core.
**Reload** in Sources restarts a page, failed or not. A page's dropdown
lists, such as a `<select>`'s, draw over it and take clicks under
**Interact**, but it opens no other windows (`window.open` and new-tab links
do nothing). Copy and Cut there put the page's selected text on the clipboard,
and nothing from a password field. Except for a page copied on the GPU through
an X display, the text goes as plain text that the page's own copy handlers
cannot change.

An overlay is a title, lower third, or other graphic written as HTML and CSS,
drawn by the studio itself (through [Blitz](https://github.com/DioxusLabs/blitz))
rather than a browser, on every platform. Its CSS animations and transitions
run on the program clock, so they land on the same frame in the program and in
a recording or replay of it, whatever the studio's load. It runs no scripts
and never touches the network. Its text is set in the bundled Noto Sans,
regular, bold, or italic, unless it loads a face of its own, and never in the
machine's fonts, so it looks the same on every machine.

An overlay's own images and fonts, such as a logo or a brand face, come from
`data:` URLs or from files in the scene file's directory, named relative to
it (`url(brand/logo.png)`), never from the web or anywhere else on disk. The
studio reads them all as the overlay starts, so its first frame has them, and
a file changed on disk changes nothing on air until the overlay starts again,
as when it is edited. Images are PNG or JPEG, at most 4096 pixels a side;
fonts are TrueType, OpenType, WOFF, or WOFF2, loaded with `@font-face`:

```css
@font-face { font-family: Brand; src: url(brand/face.woff2); font-weight: bold }
.title { font-family: Brand; background: url(brand/logo.png) no-repeat }
```

An overlay outside what overlays draw fails to start, naming the first
element, attribute, rule, property, or asset it refused:

- Elements: text and the boxes around it (`div`, `span`, `p`, headings,
  lists, tables, and the like), `img`, plus `style`, `title`, and `meta`. Not
  `script`, `link`, `iframe`, `svg`, `canvas`, or form controls.
- Attributes: `id`, `class`, `style`, `lang`, `dir`, `title`, `hidden`,
  table and list numbering ones, `data-*` and `aria-*`, and an `img`'s `src`
  and `alt`.
- CSS rules: style rules, nested ones included, `@media`, `@keyframes`, and
  `@font-face` with `font-family`, `src`, `font-weight`, `font-style`, and
  `font-display`. Not `@import`, nor `local()` or `image-set()`.
- CSS properties: box layout, flexbox, grid, positioning, margins and
  padding, colors, opacity, gradients, borders and their radii, `box-shadow`,
  outlines, transforms, `clip-path`, fonts and text layout, text decoration,
  `content`, and animations and transitions, by their usual shorthands too.
  Not yet `filter`, `backdrop-filter`, `mix-blend-mode`, `text-shadow`,
  `mask`, `text-overflow`, or multi-column layout; the list grows as Blitz
  draws more.

An overlay's animation starts when the overlay does: when the studio opens
it, and again whenever it is edited. One that no preview or program shows
costs nothing until it is shown again, where its animation has carried on.
Overlays publish no track of their own, so viewers see them only in the
program.

### Media files

A `media` source plays a video or audio file, such as an intro clip or a music
bed, on the program clock, and its sound joins the mixes like any other
source's. It reads fragmented MP4, MKV or WebM, MPEG-TS, and FLV, as moq's
importers do, and fails to start on a codec this machine cannot decode. A
regular MP4 fails to start with a remux to fragmented MP4 that makes it readable:
`ffmpeg -i in.mp4 -c copy -movflags frag_keyframe+empty_moov out.mp4`. The
whole file is read into memory, compressed, when the source starts, so it
plays from its start at once and loops without reading the disk again; keep it
to clips and music, not hours of video.

```toml
[sources.intro]
kind = "media"
path = "intro.mkv"   # relative to the scene file, like an image's
restart = true       # play from the start each time it goes on program

[sources.music]
kind = "media"
path = "music.webm"
loop = true          # start over at the end
```

Without `loop`, it plays once, then holds its last picture, silent, and reads
as ended in Sources and, while the program shows or plays it, in Health.
**Play** in Sources plays it from its start again, as does going on program
with `restart`, in a scene on air or the top layer, when it was not on program
before. Either clears its picture at once, so the end of the last play never
shows on program. **Stop** in Sources stops it at once: blank, silent, sound
already queued included, and reading as ended until played again. A file that
cannot be read or decoded fails, with the reason.
The source a [stinger](#transitions) plays waits, blank and silent, for the
first stinger instead of playing as it opens. Its pictures and its sound each have a track of their own (see [Source
tracks](#source-tracks)), its pictures on one timeline across plays. Each is
used apart, as a camera's and a microphone's are: the pictures while a scene on
air shows it, the sound while the mix on program plays it, and either while
read. So a music bed that no scene shows encodes its sound alone. A file with
no sound has no strip in the Audio panel.

## Guests

Guests and phones join a room the studio owns on its relay, from a web page,
and their camera and screen appear as broadcast sources. The page plays the
show back to them, without their own voice. The room follows
[moq-room](https://github.com/moq-dev/moq/tree/main/rs/moq-room): a guest
named `alice` publishes `alice/camera.hang` and `alice/screen.hang` under it.
Guests need a relay that verifies tokens, so run your own with
[`moq auth serve`](https://doc.moq.dev/bin/cli) behind it, and a key that can
sign only for the room. The relay must be built from moq's `main` and accept
moq-lite-07 (`--listen-version moq-lite-07-wip`, beside the versions your
other clients use), the version that renews a guest's token without
reconnecting. Its wire is still a work in progress, so the join page offers
nothing else, and a guest cannot join through a relay without it.

```bash
moq auth generate --algorithm ES256 --out room.jwk --public-dir keys \
    --root show/guests --publish '**' --subscribe '**'
moq-studio show.toml --connect 'https://relay.example.com/show?jwt=...' \
    --room guests --room-key room.jwk --join-page https://join.example.com/
```

- `--room` is a path under the `--connect` URL's, here `show/guests` on the
  relay. The studio's own token must let it subscribe there. A guest's browser
  dials the same relay, so `--connect` must be `https://`, or `http://` only to
  this machine or the local network.
- `--room-key` signs each guest's token. The studio refuses a key whose scope
  reaches outside the room, so a stolen key cannot sign for the rest of the
  relay. Put its public half in the directory `moq auth serve --key-dir` reads.
- `--join-page` is where the join page (`js/join`, `bun run --cwd js/join
  build`) is hosted. Browsers open the camera only on an `https://` page or
  `localhost`, so the studio refuses any other. It defaults to `https://join.studio.moq.dev/`, which this
  repository deploys from `main`, but that domain is not live yet, so for now
  host the page yourself and pass its URL.
- `--guest-expiry` (default `4h`, at least `1m`) is how long a link works,
  and each token the studio renews it with. While a guest is in the room, the
  studio signs them a fresh token every half of it, over a session of its own
  in the room that the room key signs for, and the join page presents it
  without reconnecting. A guest's token may read only their own renewals and
  return feed, at `.renew/<name>/<generation>` and `.return/<name>/<generation>`
  in the room, and the program at `.program`, but anything else the relay lets
  subscribe there can read every guest's, so grant that only to the studio. A guest who
  leaves needs a link, or a renewed token, that has not expired to come back.
  The Guests panel says when renewal is failing.
- **Upgrading from a studio without renewal:** generate a new room key, drop
  the old one's public half from `--key-dir`, then restart the relay (or wait
  out the old links' `--guest-expiry`) before starting the upgraded studio.
  Links from before let their holder subscribe to the whole room, renewals
  included, so they must die with the old key, and a relay keeps a session
  it already admitted until its token expires.
- With `--failover`, a link still names only the `--connect` relay: the join
  page dials one relay, and a guest's token is rooted at the room there, so
  guests drop while that relay is down. The studio follows the room through
  both relays, like any broadcast source, so a guest the failover routes at
  the same path, as a relay in the same cluster does, keeps reaching it.
  Anyone the failover lets publish in the room joins as a guest, so guard the
  room there as on the `--connect` relay.

**Invite** in the Guests panel makes a link for one guest name and shows it as
a QR code; **Copy link** puts it on the clipboard instead. The guest's token is
in the link's fragment, which the browser never sends to a server, and lets
them publish only under their own name, so they cannot appear as someone else,
and read only their own renewals, their return feed, and the program.
Treat the link like a password: anyone holding it can join as that guest
before it expires, and is then renewed for as long as they stay, until you
remove them. A guest's broadcasts become sources named after them (`alice`,
`alice screen`) when they first appear, muted and in no scene: place them from
the Items panel and unmute them in Audio to put them on air. That includes a
guest invited before a restart whose link still works. A guest's source is
saved to the scene file but is not your edit: it stays out of History, so undo
never removes it and a guest joining never loses what you could redo. A guest
who reconnects after you removed their source comes back under a new name
(`alice 2`), since undoing the removal brings back the old one.
**Remove from scenes** is your edit, so it undoes: it takes a guest, any clone
of their sources, and any mask reading them out of every scene and the top
layer and mutes them. A signed token cannot be taken back, so it also stops
renewing theirs and ends their return feed, and they are disconnected within
`--guest-expiry`. That
holds across a restart, whichever scene file you open: removed guests are
kept beside the room key, as `room.guests.toml` for `room.jwk`, so its
directory must be writable. Undo puts them back in the scenes but not on
renewal; **New link** lets them back in and renews them again, under a new
generation, so a link from before the removal cannot read their renewals.

While a guest is in the room, the join page plays their return feed: the
program's video, and the mix on program without that guest's microphone (a
mix-minus), so they follow the show without hearing themselves back. Ask
guests to wear headphones, so the show does not reach their microphone. The
studio publishes it over its session in the room, never with its own
credentials: the program once at `.program`, and each guest's feed at
`.return/<name>/<generation>`. The video is the program's own two sizes,
and a guest's player picks one for their link; this session uploads the size
a guest watches a second time, and a guest watching the half size makes it
encode as any viewer does. Each guest in the room costs one 64 kbps Opus
encode of their mix-minus. A guest's
screen share stays in their feed, since only their camera's microphone is
taken out.

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

[canvases.portrait] # a further canvas, at the main one's frame rate
width = 1080
height = 1920
top = [{ source = "logo", rect = { x = 780, y = 40, width = 280, height = 120 } }]

[transition]
kind = "slide"      # fade, slide, wipe, move, or stinger
duration = "300ms"  # all but a stinger
direction = "left"  # slide and wipe: left, right, up, or down
# A stinger names a media source and its cut point instead:
# kind = "stinger"
# source = "sting"
# cut = "500ms"

[sources.camera]
kind = "camera"

[sources.logo]
kind = "image"
path = "logo.png" # relative to the scene file

[sources.guest]
kind = "broadcast"
path = "room/guest"

[sources.alerts]
kind = "browser"
url = "https://example.com/alerts"
width = 800 # the page's own size, whatever its items stretch it to
height = 600

[sources.name]
kind = "overlay"
width = 1920 # the overlay's own size, like a page's
height = 1080
html = '''
<style>
  .third { position: absolute; left: 80px; bottom: 120px; padding: 16px 32px;
    background: linear-gradient(90deg, #1e3a8a, #2563eb); border-radius: 12px;
    font: bold 48px sans-serif; color: white; animation: in 400ms ease-out both; }
  @keyframes in { from { transform: translateX(-40px); opacity: 0 } }
</style>
<div class="third">Ada Lovelace</div>
'''

[mixes]
names = ["Stream", "Headphones"]
program = "Stream" # the mix the broadcast carries

[audio.guest]
gain = 0.5
exclude = ["Headphones"] # feeds every other mix
delay = "80ms"

[sources.camera-faded]
kind = "clone"
source = "camera"

[video.camera-faded]
filters = [{ kind = "opacity", amount = 0.5 }]

[sources.grade]
kind = "lut"
path = "teal-orange.cube"

[video.camera]
crop = { left = 0.1, right = 0.1 } # trim 10% of the picture off each side
filters = [
    { kind = "chroma_key", color = "#00ff00", similarity = 0.4, smoothness = 0.08, spill = 0.1 },
    { kind = "lut", source = "grade", amount = 0.8 },
]

[[scenes]]
name = "Main"
items = [
    { source = "camera", rect = { x = 0, y = 0, width = 1920, height = 1080 } },
    { source = "guest", rect = { x = 1280, y = 720, width = 640, height = 360 }, filters = [{ kind = "drop_shadow", x = 8, y = 8, blur = 12, color = "#00000080" }] },
    { source = "logo", rect = { x = 0, y = 0, width = 1920, height = 1080 }, blend = "multiply", filters = [{ kind = "opacity", amount = 0.3 }] },
]
canvases.portrait = [ # the same scene laid out on the portrait canvas
    { source = "camera", rect = { x = 0, y = 0, width = 1080, height = 1920 }, crop = { left = 0.34, right = 0.34 } },
    { source = "guest", rect = { x = 0, y = 1312, width = 1080, height = 608 } },
]

[[scenes]]
name = "Break"
items = [
    { source = "camera-faded", rect = { x = 0, y = 0, width = 1920, height = 1080 } },
    # Flies in from the right, blurred, over half a second, landing where it rests.
    { source = "guest", rect = { x = 1280, y = 720, width = 640, height = 360 }, keyframes = [
        { at = "0s", x = 700, filters = [{ kind = "blur", radius = 12 }] },
        { at = "500ms", ease = "out", filters = [{ kind = "blur", radius = 0 }] },
    ] },
]

[[top]]
source = "logo"
rect = { x = 1600, y = 40, width = 280, height = 120 }
```

Source kinds are `camera`, `display`, and `microphone` (each with an optional
`id`, the default device without one), `window` (with an `id`), `system_audio`
(macOS), `image` (PNG or JPEG), `media` (a video or audio file; see
[Media files](#media-files)), `color` (`#rrggbb` or `#rrggbbaa`),
`broadcast`, `browser` (a `url`, and a `width` and `height` up to 4096),
`overlay` (its `html`, and a `width` and `height` up to 4096), `clone` (with the `source` it shows), `lut` (a 3D Adobe or Resolve `.cube`
file, which no item places, but which grades what a `lut` filter names it in),
and `replay`, where the Replay panel plays replays. The Devices panel fills in
ids. `[[top]]` items make up the top layer.

`[canvases.<name>]` adds a further canvas, with its `width` and `height` and
its own `top` layer, and a scene's `canvases.<name>` lists its items on that
canvas; leave it out, rather than empty, for a scene that shows nothing there.
A canvas name is one segment of its broadcast path, so it holds no `/`.

`[mixes]` names every mix, in the order the Audio panel lists them, the one
on `program`, and the one a `recording` holds, the mix on program without it;
without `[mixes]`, the collection has one mix, `Program`.
`[audio.<source>]` sets a source's linear `gain`, `muted`, the mixes it is
left out of (`exclude`), and its `delay`.

`[video.<source>]` sets a source's `crop` and `filters`, wherever it is shown,
and an item's `crop`, `filters`, and `blend` its own. A `crop` trims a fraction
(0 to 1) of the picture off its `left`, `top`, `right`, and `bottom`, opposite
sides together trimming less than the whole, and stretches the rest over the
item; an item's crop trims what its source's
leaves. Filters apply in order, the source's then the item's, with lengths in
canvas pixels:

| Filter | Settings |
| --- | --- |
| `opacity`, `grayscale`, `sepia`, `invert` | `amount`, 0 to 1, as in CSS |
| `brightness`, `contrast`, `saturate` | `amount`, 0 or more, 1 leaving the picture as it is, as in CSS |
| `hue_rotate` | `degrees`, as in CSS |
| `blur` | `radius`, the standard deviation; the edges fade out, as in CSS |
| `drop_shadow` | `x`, `y`, `blur` (the standard deviation), and `color`, as in CSS |
| `chroma_key` | `color` keyed out, and `similarity`, `smoothness`, and `spill` from 0 to 1 |
| `luma_key` | `min` and `max` luma kept, `min` at most `max`, and `smoothness`, all from 0 to 1 |
| `lut` | the `lut` `source` to grade through, and `amount` from 0 to 1 |
| `mask` | a `source` whose picture, stretched over the item, keeps what it covers: by its alpha, or `mode = "luminance"` |

An item's `blend` mixes it with what is under it by a CSS `mix-blend-mode`:
`multiply`, `screen`, `overlay`, `darken`, `lighten`, `color_dodge`,
`color_burn`, `hard_light`, `soft_light`, `difference`, `exclusion`, `hue`,
`saturation`, `color`, or `luminosity`. Removing a source removes the filters
that name it.

An item's `keyframes`, in order of time, each have an `at`, how long after the
scene goes on, and optionally an `ease` into it from the one before (`linear`,
`in`, `out`, or `in_out`, the default), `x`, `y`, `width`, and `height` added
to the item's `rect`, a `crop` of what the item's crop leaves, and `filters`
applied after the item's own.

The studio writes the file a second after the last change, so a burst such as a
slider drag is one write, and once more on quit. Edits that never pause for a
second are written five seconds after the first, so a crash loses at most the
last five seconds of edits. Each write replaces the whole file at once, never
leaving half of it, and a collection that has not changed is not rewritten, so
a hand-written file keeps its comments and layout until the next edit in the
studio. A failed write shows in the status bar and
is retried every second; until one succeeds, a crash loses every edit since the
last write.

The file can be edited by hand, or by another studio, while the studio runs.
The studio looks at it every second and before each write. With no edits of
its own unsaved, it reloads the change, as **File > Reload** does, which
replaces even unsaved edits. If both sides changed, or the change cannot be
loaded, the studio saves nothing until you pick **Keep the studio's**, which
writes its version over the file, or **Take the file's**; quitting before you
pick saves nothing. A file deleted while the studio runs is written again. One
overwrite remains possible: a change saved in the instant between the studio's
last look and its write replacing the file.

The last 100 edits are written the same way beside the scene file, as
`show.history.toml` for `show.toml`, after the scene file, and opening the
scene file restores them, so undo reaches back past a restart. An undo or redo
is saved like any other edit. An edit that no longer matches the scene file,
such as after editing it by hand, is dropped with every older one, and the
status bar says how many. A history file that cannot be read stops the
studio, naming the file; delete it to start a new history. The history holds
whatever the scene file held, including removed sources, so check it too
before sharing.

## Output

The program publishes as two H.264 renditions, at the canvas size and at half
of it, at the canvas frame rate, with the mix on program as Opus stereo audio,
under the `--broadcast` path. A canvas under 360 lines has no half size.
Timestamps come from the studio's own clock, so a source that stalls repeats
its last picture instead of holding up the stream.

Each rendition follows its share of the bandwidth the first relay's connection
estimates: a closing uplink lowers its bitrate at once, and an opening one
raises it back over a few seconds, up to 0.07 bits per pixel (about 4.4 Mb/s
for 1080p at 30 fps). The half size is funded first. When the share left for
the canvas size cannot fund it over the half size, the catalog lists it as
disabled and viewers move to the half size until the uplink recovers. A viewer
joining a track whose newest group has grown heavy asks for a keyframe, and
the encoder cuts one, so later viewers start near it.

A new canvas size or frame rate, from the Output panel, an undo, or a reload,
takes effect live. A track never changes what it carries, so the old
renditions' tracks end and new ones are listed in their place in a catalog
update, on the same timeline, and viewers switch to them; the video pauses
while the new encoders open. Items keep their place in canvas pixels. A canvas
the encoder cannot take stops the video, saying why, until the canvas changes
again. At startup, it stops the studio when live; without `--connect`, the
studio starts without video.

The canvas size encodes whether or not anyone watches, so a viewer who arrives
late can fetch the recent past. The half size encodes only while someone
watches it. `--on-demand` copies and encodes the canvas size only while someone
watches it too, which saves that work but leaves nothing from before the first
viewer; audio encodes either way. A recording has an encoder of its own (see
Recording). A hardware encoder that reads
the GPU (NVENC on Linux, VideoToolbox on macOS, Media Foundation on Windows)
takes each frame there when it can read the GPU the studio draws on. When it
cannot, such as NVENC while the studio draws on a laptop's integrated GPU, the
studio passes over it for an encoder that reads each frame back to the CPU
(the opt-in `vaapi` or `openh264` features), which the Output panel counts.
Frames the encoder cannot keep up with are dropped rather than delaying the
stream.

### Source tracks

Each camera, display, window, image, media file, color, microphone, and system
audio also has a track of its own, so the studio can replay or record it on its
own. These tracks stay in the studio: the program is all it publishes to a relay. They are listed in a local
catalog under the `dev.moq.studio` section's `sources`, by source name, each
as a hang catalog of its own on the program's clock.

A captured source encodes its track as H.264 only while it is used: while it
is shown on program, so a replay of it has its recent past, or while a
recording reads its track. Otherwise it holds no encoder. A display or window
that goes into use while its screen stands still, and so delivers nothing new,
restarts its capture, so its track starts with the picture as it is. A capture that
changes size or color space moves to a new track, which its catalog entry
follows. Images and colors keep their picture once, as the image file and as a
one-pixel PNG. Removing or editing a source ends its track. A track carries the
plain capture, before any filter, and a clone has none, since it shows its
source's capture. A browser source or an overlay has no track of its own, and a
broadcast source is already its publisher's.

A microphone, system audio, or media file's sound encodes its track as Opus,
from the same sound the mixes hear, only while it is used: while the mix on program plays it
(unmuted and not left out of it), so a replay of it has its recent past, or
while a recording reads its track. The track carries the sound as captured,
before its gain, mute, and delay, which stay the mixes'. A buffer the capture
lost, or a stretch the source was out of use, is a gap on its track. Opus
encodes on the CPU, about 1% of one Apple M4 core for a stereo source and
half that for mono, and holds no encode session.

Every encoder holds an encode session, and consumer GPUs allow only a few
hardware sessions at once: 8 on a GeForce card, while a base Apple M4 has one
encode engine for them all. The Output panel counts the sessions open:

| Encoder | Sessions |
|---|---|
| The program's canvas size, live | 1, or none without `--connect` |
| Its half size | 1 while someone watches it |
| Each further canvas's canvas size and half size | as the program's |
| The recording's own canvas size | 1 while recording |
| Each captured source or media file | 1 while on program or recorded |

So a live scene showing a camera, a display, and a window holds 4 sessions, 5
with the half size watched, and 6 while recording too. A portrait canvas beside
it adds 1 more, 2 with its half size watched. When the program
encodes on hardware, so do the sources, and a
session the GPU refuses fails that source's track with the encoder's reason in
the Sources panel rather than falling back to software. The source keeps
drawing in the studio's own scenes. The program opens its session before any
source, even with `--on-demand`, so the sources past the cap fail rather than
the program; so does each canvas in the scene file at startup. A canvas added
later, past the cap, draws without video and says why in the Output panel. When the program encodes in software, a source takes hardware
where it can and software otherwise.

### Recording

**Start recording** in the Output panel saves the program, at the canvas size,
to `<scene file name> <UTC start time>.mp4` in the `--recordings` directory,
live or not. The recording has an encoder of its own, opened only while it
records, at full quality whatever the uplink grants the live stream, as OBS's
advanced output mode records apart from streaming. It costs one more encode
session and one more copy of each program frame while recording. With
**each source too**, every camera, display, window, microphone, and system
audio records its own track into a file of its own beside it,
`<scene file name> <UTC start time> <source>.mp4`, holding the capture before
any filter, or the sound before its gain and mute; a source being recorded
encodes whether or not it is on program.
Each broadcast source, a guest's included, records its broadcast as it arrives
from the relay, its largest video and its audio copied without a re-encode, on
its publisher's timestamps, into a file named after it while it is live. A
guest who leaves finishes their file, and one who joins again goes on in a new
one, numbered after the first, since each join is a new broadcast with a
timeline of its own. A source added while recording
starts its file then, and one removed finishes its file then. A new canvas
size or frame rate, or a capture that changes size or color space, moves the
video to a new track, so the program or that source goes on in a new file,
numbered after the first, since players show only one video track. A file
starts with up to 2 s from before it opened, so a recording starts a little
early, and a track that goes on into the next file, such as the program
audio, repeats up to 2 s of the last. The
program and local sources are read from the studio's own broadcasts, never what
the relay serves, so a recording starts and stops without touching the live
stream. The mix it holds is
encoded a second time for it while recording: the mix on program, so the file
holds what viewers hear, or the one the Output panel picks, which viewers never
can. Without `--connect`, only the
recording's encoder encodes the program.

Each file plays while it grows, so a crash leaves a playable fragmented MP4,
and becomes a regular MP4 when the recording stops or the studio quits.
Quitting first lets the recording's encoders publish what they still hold, and
the program's file keeps it. A file nothing reached before the recording
stopped is removed.

While a file is written, a marker `<name>.mp4.unfinished` sits beside it. A
crash, a power loss, or a write that failed leaves the marker, and the next
launch repairs that file without asking: it writes `<name>.repaired.mp4`
beside it, numbered if a file already has that name, a regular MP4 of
everything the file holds up to its last complete fragment, leaves the
original untouched, and removes the marker, so the file is repaired
only once. The repair runs in the background and appears at the top of the
Health panel and in the log. A file another studio is still recording into the
same directory is left alone.

### Source frames

Sources make the opposite trip. A frame already on the GPU is drawn where it
lies: PipeWire camera and VA-API decode DMA-BUFs on Linux, given a Vulkan
driver that imports them, and NV12 capture and VideoToolbox decode surfaces on
macOS. A browser source's page is copied on the GPU from the shared texture CEF
paints, given the same Vulkan DMA-BUF import and a display to paint through:
the Wayland one when `$WAYLAND_DISPLAY` is set, needing no XWayland, or else
the X one in `$DISPLAY`. Every other source, every source on Windows or on a
GPU that cannot import, and every still image delivers its frames from the
CPU, which the compositor uploads. A browser source takes that path with no
display at all too, and there renders its page in software, WebGL included, so
a WebGL page runs slower.

## Replay

Add a `replay` source and place it like any other, such as full screen in a
scene of its own. In the Replay panel, pick the program or a source, a length
up to 30 seconds, and a speed of 100%, 50%, or 25%, then **Replay** plays that
many seconds up to the click in the replay source, which is transparent before
and after. **Cut** to its scene to put it on program. Sound replays only at
100%, through the replay source's strip in the Audio panel, so a replay of a
microphone or system audio, which has only sound, plays only at 100%. A new
replay cuts
off the one playing, and a stretch the source was offline or paused replays
as the gap it was.

**Save clip** writes the same seconds, in real time, beside the scene file as a
regular MP4: `show-clip-1.mp4` for `show.toml`, then `show-clip-2.mp4`, and so
on. Nothing is re-encoded, so a clip starts at the keyframe before those
seconds, up to one keyframe interval earlier. It shows nothing after them, so
video with B-frames can end early: the pictures shown before the end that refer
to one shown after it are left out, along with everything decoded after that
one. That is usually a fraction of a second, but it can reach back as far as
the keyframe for a slow or deeply reordered source. The panel says where once
it is written, shortly after the click.

Nothing records in the background for this. The program's tracks keep their
last 30 seconds for late viewers anyway, and each source's track while it is on
program (a picture shown in a scene on air, or a sound played by the mix on
program) or recorded, and a replay copies from those, so a replay
goes back at most that far. A replay of the program needs `--connect`; a local
source keeps its track without a relay. With `--on-demand`, the program has
no past from before someone watched it, and a source has none from before it
was last on program or recorded; a replay of nothing is refused, saying why. A
broadcast source replays from what the studio received of it, and only over a
relay connection whose protocol version can address a track by time; an older
one refuses with the reason in the panel. An image, a color, or a web page has
no past to replay: a page publishes no track of its own, so replay the program
to show it. A replay source on program encodes each replay as its own track
while it plays, like a camera.

## Health

The Health panel grades the stream every second on the thresholds moq's
`hang::health` shares with the rest of the stack, and lists each problem
with its cause in plain words: a relay is unreachable, the uplink is
congested or losing packets, the relay is far away, the encoder is behind or
stuck, a source stalled (a camera or broadcast the program draws, or a
microphone or system audio the mix on program plays: unmuted and not left
out of it), a source the program shows or plays failed or, as a media file
that does not loop, played to its end, or viewers are late, stalling, or cannot
decode a track. A relay lost while the other serves is
degraded, since viewers have nothing left to fall back to; with none serving
it is unhealthy. Below that it shows:

- **Relays**: each relay's state, and the protocol its session negotiated.
- **Uplink**: the round trip to the first connected relay, the bandwidth the
  connection estimates it can carry, the media the program's tracks write, and
  the share lost and resent. Loss and headroom are graded only while someone
  watches, since the relay pulls nothing otherwise. Source tracks never reach
  the relay, so they are left out of the media.
- **Watching**: the sessions subscribed to the broadcast, from the relays'
  `.stats` broadcasts. A relay shares those only with a connection allowed to
  read them, so most show "unknown". In a cluster, a relay forwarding the
  stream to another counts as one session. Two relays whose stats use the
  same node name publish them at the same path, so only one of them counts:
  the first relay's while it has them.
- **Tracks**: each track's share of the connection's bandwidth (**Granted**,
  or "off" for a rendition the share cannot fund), its bitrate and frame rate,
  and, from viewers that report, the lowest bitrate any received, its worst
  grade, and the latency from the program clock to the slowest viewer. The
  program's tracks come first, then each source's own tracks, grouped by
  source; a still image or
  color carries no media to grade. Latency compares the two hosts' wall
  clocks, so keep both synced.
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
just join    # Serve the join page locally
```

`rs/moq-scene` is the UI-free compositor, `rs/moq-studio` the app, and
`js/join` the join page, built on `@moq/room`, `@moq/publish`, and
`@moq/watch`. Like the
Rust crates, those come from the `moq` submodule rather than npm, so
`bun install` needs it checked out, and Nix to build `@moq/libav`. See
[AGENTS.md](AGENTS.md) and [CONTRIBUTING.md](CONTRIBUTING.md), and the
[quest tree](quest/README.md) for planned work.

## License

Licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

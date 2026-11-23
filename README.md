# kixel.live Studio

`moq-studio` is a native streaming studio for Linux, macOS, and Windows. It
composes cameras, screens, windows, images, colors, web pages, HTML and CSS
overlays, and broadcasts from a relay into scenes on the GPU, mixes their audio, and publishes the result to a
[MoQ](https://github.com/moq-dev/moq) relay. It is early.

On Linux, each [release](https://github.com/moq-dev/studio/releases) ships a
Flatpak bundle for x86_64 and aarch64:

```bash
flatpak install --user moq-studio-x86_64.flatpak
flatpak run live.kixel.studio
```

Its menu entry opens [your shows](#your-shows) and saves recordings to your
Videos folder. The sandbox reaches
cameras through the camera portal, which asks once, and files under your home
directory. The virtual camera is its PipeWire node only, since the sandbox
has no `/dev/video*`.

On macOS 13 or later, each release ships the app for Apple silicon
(`moq-studio-macos-arm64.dmg`) and Intel (`moq-studio-macos-x86_64.dmg`),
signed and notarized. Open the disk image and drag **kixel.live Studio** to
Applications. Opened from Finder or the Dock, it opens
[your shows](#your-shows). macOS asks you to grant it the camera, microphone, and Screen &
System Audio Recording permissions, the last in System Settings > Privacy &
Security. Run it from a terminal with the flags below as
`"/Applications/kixel.live Studio.app/Contents/MacOS/moq-studio"`.

Elsewhere, build it from source. The moq crates come from the `moq`
submodule:

```bash
git clone --recurse-submodules https://github.com/moq-dev/studio
cd studio
cargo run --release -p moq-studio --features capture,openh264
```

- With no arguments, the window reopens the last show, or on
  [first launch](#first-launch) creates one in the app's storage, and opens
  straight into the studio. Name a show to open it instead:
  `moq-studio show.toml`. A show is its scene file, created as a new show if
  it does not exist yet. Changes are saved to it a second after you stop
  editing, with no save step.
- Nothing is published until you [go live](#going-live) from the window, and
  you end the stream from it too. Until then the studio composes, previews,
  and records, and broadcast sources and guests play through the show's
  relays.
- Where a show streams is in Settings, Stream (see
  [Stream settings](#stream-settings)), kept beside the show. A flag sets one
  for a single run, over the show's own, and is never saved: `--connect` (the
  relay) with `--connect-token`, `--failover`, `--broadcast`, `--on-demand`,
  `--recordings <dir>`, where recordings and clips go (by default beside the
  show, or your Videos folder for a show in the app's storage), and the guest
  flags.
- A failover relay publishes the same broadcast to a second relay under the
  same broadcast instance, so a viewer linked to both resumes mid-group from
  it when the first fails. Both relays must speak the one `--connect-version`,
  which must carry instances (`moq-lite-07-wip` today, which a relay
  accepts only when configured to), or going live with both refuses; it also
  refuses a fixed `--connect-bind` port, since each relay's connection binds
  its own. Each relay's token is its own: `--connect-token` goes to the
  `--connect` relay only. The stream's tracks claim their bitrate on the first
  relay's link alone, so only that link is probed toward the stream's rate,
  and the video follows that link's estimate alone.
- Quitting while live ends the stream as **END STREAM** does: every track
  ends after its last pictures and sound, then each relay session closes once
  the relay has them, waiting up to about a second, so viewers and recordings
  on the relay see the broadcast end rather than cut off.
- `capture` adds camera, display, window, microphone, and system-audio
  sources, and listening to an audio scene. On Linux it links ALSA; add `pipewire` for
  PipeWire cameras and the [virtual camera](#virtual-camera)'s PipeWire node.
  `openh264` is the software H.264 fallback for a machine without a hardware
  encoder.
- `release` is what the packages build: `capture`, `openh264`, and the
  hardware codecs. The Flatpak adds `pipewire`.
- `cef` adds browser sources, on Linux and macOS. The build takes CEF from
  `CEF_PATH`, which the dev shell sets on Linux, or else downloads it. On
  Linux the binary loads `libcef.so` from beside itself or from that
  `CEF_PATH`. On macOS browser sources run only from the app bundle, which
  holds CEF's framework and its helper apps: build it with
  `packaging/macos/bundle.sh`, which the macOS packages ship with `cef`.
- On Linux, display and window sources capture X11 and XWayland by id. With
  `pipewire`, a display or window can also be picked in the desktop's
  screen-sharing picker, the only way to share a Wayland screen; see
  [Picked displays and windows](#picked-displays-and-windows).
- On macOS, a build from source is not an app bundle, so the camera,
  microphone, and Screen & System Audio Recording permissions are asked of,
  and granted to, the app that launched it, such as your terminal. Display,
  window, and system-audio sources need the last, granted in System Settings >
  Privacy & Security.
- The `--connect-*`, `--quic-*`, and `--log-level` flags are
  [moq-cli's](https://doc.moq.dev/bin/cli); see `moq-studio --help`.
- `moq-studio --watch <url>` opens no scene file and plays a studio broadcast
  instead, as a viewer sees it; see [Watching](#watching).

## First launch

The first launch opens no dialog and runs no setup screen: the window opens on
a new show, **My Show**, already saved in the app's storage
(`~/.local/share/moq-studio/shows` on Linux,
`~/Library/Application Support/moq-studio/shows` on macOS,
`%APPDATA%\moq-studio\data\shows` on Windows), with one empty scene. While it
opens, the studio picks its canvas: 1080p at 60 fps when a hardware encoder
keeps up with it over a short burst of moving frames (three quarters of each
frame's period at most, leaving the rest for sources and the compositor), and
1080p at 30 fps otherwise. If the system has a microphone, the show hears it
as **Mic** in the sound. Its recordings go to your Videos folder. Change the
canvas in Settings, Show, and where recordings go in Settings, Recording.

While the scene has no layers, the picture shows the quick start: one click
on **Camera** (naming the camera it adds), **Screen**, **Media** (an image or
video file), or **Web / Text** (a web address, or a text to edit) adds the
scene's first layer, and the picture becomes the scene. Anything dropped or
pasted on the picture lands there too (see
[Dropping and pasting](#dropping-and-pasting)).

The go-live sheet times the encoder again before you go live, and offers a
smaller canvas if it cannot keep up (see [Going live](#going-live)).

## Your shows

Every later launch reopens the last show you had open. If it is gone, the
studio creates a new one, and if it cannot be opened, the studio creates a new
one and says why. The show's name in the title bar opens its menu, which
lists the other shows you have opened, the latest first, with when each was
last live under the pointer, and:

- **New show** creates another show in the app's storage, named **My Show 2**
  and so on, and opens it.
- **Open show file…** opens any scene file, where it is.
- **Rename show…** renames the show's scene file, its history, and its
  stream settings, and opens it again. Its recordings and clips keep their
  names, and REVIEW still lists them: the stream settings note the show's
  earlier names (`earlier`), which no new or renamed show takes.
- **Export show copy…** saves the scenes as a scene file of its own, to share
  or keep under version control. In another folder, each file a source reads
  is named from the root, so the copy reads the same files; an overlay that
  loads files from beside the show cannot be copied there. Its history and
  stream settings stay with the show.

Each but the export ends the open show as quitting does (saving it, finishing
a recording, and letting go of its relays, sources, and keys) before opening
the next, so they are dimmed while the stream is on air or going on or off:
end the stream first. A show that cannot be saved, because its scene file
changed outside the studio, stays open and says so, and one that cannot open
brings back the show you left, saying why. The list is yours, kept with the
app's settings rather than beside any show.

## The window

The studio opens to its **LIVE** page: the program as large as the window
allows, in a neutral frame tagged **OFF AIR** until you go live, then a blue
one tagged **ON AIR**. The title bar and the strip along the bottom stay
on every page, so the scenes, the sound, and the panic key never leave the
window while you are on air.

Blue, the studio's accent, means on air (and marks what is picked or switched
on), amber anything cued, off air, muted, or degraded, and red an error, such
as a failure or a clipping meter. An icon-only button names itself, and its
key, under the pointer.

The **title bar** is the window's own: drag it to move the window,
double-click it to maximize, and drag the window's edges to resize it. On
macOS the system's buttons and edges stay. It holds, from the left:

- The show's name. Its menu has the scene file's path, [your
  shows](#your-shows) with New show, Open show file, Rename, and Export a
  copy, **Reload the scene file**, **Copy the selected layer**, **Paste**, and
  **Open the multiview** (see [Multiview](#multiview)).
- Undo and redo, each naming under the pointer the edit it would step, and
  the history (the clock), which lists every edit, newest first: pick one to
  undo or redo back to just after it.
- The pages: **LIVE**, **BUILD**, **MIX**, and **REVIEW**.
- The health dot, the stream's worst grade at a glance. Click it for the
  worst problem first, then each destination, who is watching, and the
  encoder, and **Details** for the [Health](#health) window. Off air it reads
  **Ready** once the show has a relay, and **No relay** until then. While a
  capture a live scene shows has [gone dark](#a-capture-gone-dark), or a
  camera or broadcast it shows [stopped sending pictures](#a-picture-that-stalls),
  and those are among the worst problems, it counts them, as **1 layer dark**,
  **1 layer stalled**, or **2 layers dark or stalled**.
- Record, which starts or stops a [recording](#recording) from any page, with
  the picks in Settings, Recording. Its dot pulses while recording, with
  how long it has been recording beside it.
- The live control: **GO LIVE** off air, which opens the
  [go-live sheet](#going-live); on air, **LIVE** and how long the stream has
  run, which reads **END STREAM** under the pointer.
- Search, which opens the [command palette](#command-palette).
- The gear, which opens [Settings](#settings).
- Minimize, maximize, and close, except on macOS.

The **strip** holds:

- A **pad** for each scene, its picture redrawn a few times a second, with its
  name, its hotkey, an ON AIR tag on the scene on program (before going live,
  a **FIRST** tag in amber, as the stream opens on it), and a NEXT tag on the
  scene cued or open off air. Click a pad to take its scene on air by the
  show's transition (see [Transitions](#transitions)), or to cue it in
  [Studio mode](#off-air-and-studio-mode); clicking the scene already on air
  leaves nothing cued. A scene that cannot go on air, of another size or
  showing the program, has a dimmed pad that says why under the pointer.
  Right-click a pad to take its scene, cue it in Studio mode, **Edit off
  air**, mark it the safe scene, rename it, duplicate it, move it left or
  right, or remove it. **Duplicate** adds a scene after it showing the same
  sources, linked, so an edit to a source's look shows in both; **Duplicate
  with its own copies** gives each of its layers a copy of its source to edit
  apart (a scene or the program drawn in it stays one picture). A pad ringed
  in red says what just failed on it under the pointer. The last pad, **+**,
  adds an empty scene without taking it, and the switch after it turns
  [Studio mode](#off-air-and-studio-mode) on and off.
- The **panic key**, amber and named for the safe scene: it mutes every
  microphone and every guest (their camera's sound and their screen's), hides
  every always-on-top layer, whatever it shows, in one edit undo takes back,
  and cuts at once to the scene marked safe, if one is. Other remote feeds and
  media stay on. Mark one from its pad's menu, BUILD's inspector with no layer
  selected, or the scene graph. The scissors beside it save a replay clip (see
  [Replay](#replay)), and the flag drops a marker (see [Markers](#markers)).
- A row for each sound the stream carries, with its meter and mute, and the
  **STREAM** meter of the audio scene on program, with **Listen** (the
  headphones) to hear it (see [Audio scenes](#audio-scenes)).

| Page | Use it to |
| --- | --- |
| LIVE | Watch the program while on air, and TAKE or AUTO a cued scene. |
| BUILD | Edit the scene on program, or the one open off air, shown in the middle, with its layers and what can be added on the left and the inspector on the right (see [BUILD](#build)). |
| MIX | Ride each sound's fader and mute it, open its strip for its pan and delay, and send each sound and audio scene into audio scenes, from dots under the strips or as a matrix under **Routes**. Add, rename, reorder, and remove audio scenes, put one on the stream, and listen to one on the output you pick (see [Audio scenes](#audio-scenes)). The program shows small in the strip's place for the sound. |
| REVIEW | Pick a stretch of the last 30 seconds of the program or a source from a strip of its pictures, and replay it in a replay source, slowed down if you like, or save it as a clip (see [Replay](#replay)). Beside it, this session's markers, with a clip around each (see [Markers](#markers)), over this show's recordings and clips as cards with their length and size, one repaired after a crash marked so, and **Open the folder**. The program's card is the live picture. |

The layout follows the program's shape and the window's size, giving the
picture the most room it can. A landscape program sits above the strip; a
portrait one moves the pages to a sidebar on the left and the scenes and sound
to columns beside the picture; and a narrow window stacks the scenes, the
panic key, and the sound under the picture. The window is at least 640x600.

A message about what the last action did shows over the bottom of the page
for a few seconds, an error a little longer. A conflict with an outside edit
to the scene file, or a failed save, shows over its top until it is settled.
An error with a place shows there instead: on the layer's row and in the
inspector, on a scene's pad, or in Add, for ten seconds or until the next
edit there succeeds.

### BUILD

BUILD edits the scene on air as viewers see it: every change is live at once,
unless a scene is open [off air](#off-air-and-studio-mode). What a scene draws is a stack of **layers**, each placing one source.

**Layers**, on the left, lists them front first in two groups: **Always on
top**, drawn over every scene on program and staying on through a cut or
transition (a downstream keyer for a logo or a lower third), and **This
scene**, the edited scene's own: the one on program, or the one open off air. Each row has an icon for the source's
kind, its name, what it shows (and **linked ×3** when three layers show the
source), and its eye, which shows or hides it. A lock shows on a locked row,
and on the selected or pointed-at one to lock it. A dot says a source is not
live, such as starting, stalled, offline, ended, or failed, with the words on
hover, and an error on the layer replaces what it shows, in red. A capture
[gone dark](#a-capture-gone-dark), or a camera or broadcast whose picture
[stalled](#a-picture-that-stalls), has an amber badge instead, which opens
what happened and the fix. Click a row
to select its layer, drag it up or down its group to restack it, and
right-click it for its menu: hide or show, lock or unlock, interact with a web
page, bring it to the front or forward or send it backward or to the back,
keep it always on top or move it into this scene, duplicate it (linked, or as
its own copy), make it independent, copy it, or remove it. **Add a layer**
(A) turns to Add.

**Add** lists what can be placed: what a drop or paste makes (see
[Dropping and pasting](#dropping-and-pasting)), the kind a drag over the
window holds lit; the sources no layer shows, no audio scene
hears, and nothing reads, each with **Add**, to place it in the edited
scene, or the bin, to remove it for good; this machine's cameras, displays,
windows, and audio inputs; any source by hand; and, with a room, guests:
invite one, show their join link as a QR code in a sheet over the window, see
who joined, and take a guest out of every scene. A new source with a picture
is placed in front of the edited scene, and selected.

On the **picture**, click a layer to select it, drag it to move it, and drag
its edges or corners to resize it (see [Placing layers](#placing-layers)). A
locked layer stays put: clicks pass through it to what is under it. A small
toolbar beside the selected layer offers **Crop**, after which dragging an edge
trims the picture without scaling it and dragging inside moves the picture
under the frame, with the whole picture outlined; **Fit**, as large as fits on
the canvas, keeping its shape; **Fill**, covering the canvas, keeping its shape
and cropping the rest evenly; **Shape**, square, rounded, circle, or arch;
**Cut out**, removing the background of a source that can lose it;
**Interact**, on a web page; and **...**, the row's menu. Right-click a layer
for the same menu, or the picture elsewhere to add a layer, paste, or set
**Snap**, **Grid**, and the grid's spacing. Escape ends cropping, then the
selection.

A layer in a [slot](#slots-and-roles) is outlined on the picture, dashed, with
a chip naming its role, or **Guest** or its slot, and a tag naming the source
it shows. It has no handles, nor Fit and Fill, since its slot places it: drag
it onto another slot, or drag a row of Layers onto a slot, to swap the two
layers' places. A row in a slot says **Main slot** or **Tile** before what it
shows. The row's menu puts a layer in the main slot or a tile, or sets it
free where it is drawn.

**Interact** passes the mouse and keyboard to the selected layer's web page
alone, even where another layer is over it, while the picture says so; Escape
or **Stop** ends it.

The **inspector**, on the right, follows the selection. For a layer, its tab
is named for it, with these sections:

| Section | Use it to |
| --- | --- |
| Place | Pick its [slot](#slots-and-roles), or none; type or drag its position and size, and its crop, in percent off each edge, fit or fill it, lock it, and switch on **Keep out of the way** (see [Keeping out of the way](#keeping-out-of-the-way)). A layer in a slot has no position or size of its own to type, and keeps out of the way only when set free. |
| Shape | Pick its outline, square, rounded, circle, or arch, give it a border inside that outline and the border's color, and give it a soft shadow, which follows the outline and is the layer's first drop shadow filter. |
| Look | Remove its source's background, which every layer showing the source loses (make it independent to cut out here alone); grade its color through a LUT source; add, order, set, and remove its filters, copy them, or paste copied filters after its own; and set its blend mode. |
| Shown in | See how many places show its source, **Linked in 3 scenes**, and **Make independent**, which points this layer at a copy of the source of its own, keeping its look, so editing that look no longer edits the others. A copy says what it copies. |
| Source | See what the source is and whether it and its own track are live. A camera or broadcast without a new picture, or a microphone or system audio without new sound, for a second reads as stalled; a broadcast says how far behind it plays, and a web page whether it is copied on the GPU or, as a fallback, uploaded from memory, saying why. A media file reads as ended once it plays to its end without looping: **Play** plays it from its start, **Stop** stops it at once, blank and silent, until played again, and **Reload** reads the file again; **Reload** restarts a web page, and opens a picked display or window again, as after the picker was cancelled or sharing stopped. Publish it independently, then whether viewers may switch it off and its priority for them (see [Published independently](#published-independently)), and set its own crop and filters, before the layer's, on every layer showing it. **Warn when it goes dark**, on by default for a camera, display, or window, watches it for [going dark](#a-capture-gone-dark). For a role, pick the source filling it. |

Its **Animate** tab holds the layer's keyframes (see [Keyframes](#keyframes)).
With no layer selected, the inspector shows the edited scene: rename it,
see its size, mark it **Safe**, switch on **Publish** and pick the audio scene
it carries (see [Published scenes](#published-scenes)), arrange its slots and
fill their roles (see [Slots and roles](#slots-and-roles)), and pick the show's
transition and how long it takes, or for a stinger its media source and cut
point (see [Transitions](#transitions)). A scene on program, or still leaving
it in a transition, cannot be removed, even by an undo, until program cuts
away from it. In a narrow window, the inspector is a tab beside Layers and
Add.

### Off air and Studio mode

**Edit off air**, in a pad's menu, opens its scene on BUILD without taking it:
the frame turns amber and reads OFF AIR, and the program shrinks to a corner
beside a banner. **Take it live** puts the scene on air by the show's
transition; **Back to live** goes back to editing the scene on air. Any scene
opens off air, even one that cannot go on air. One viewers see anyway, still
leaving program in a transition, published or recorded on its own, or shown
by a scene they see through a scene source, reads NOT ON PROGRAM instead, and
the banner says how they see it, since an edit reaches them. Off air, Layers
lists the scene's own layers alone: the always-on-top ones are drawn on
program, so editing them, or keeping a layer always on top, waits for **Back
to live**.

**Studio mode**, off by default, is for an operator running a show: with its
switch at the end of the scene strip on, a pad or a scene's hotkey cues its
scene instead of taking it. A NEXT card docks on the program, on LIVE and on
BUILD, with the cued scene as it will go on air, its keyframes and overlays
started over. **TAKE** puts it on air at once and **AUTO** by the show's
transition; **Edit** opens it off air, from where **Back to live** leaves it
cued; and the close clears the cue. On BUILD the card moves to the opposite
corner while the selected layer is under it, so it hides neither the layer
nor its toolbar, even mid-drag. Studio mode is kept per user, with
the [hotkeys](#hotkeys), not in the scene file.

### Command palette

**Ctrl+K** (**Cmd+K** on macOS), or the search button in the title bar, opens
a search box over any page that runs anything the studio can do. Type a few
letters of what you want, pick a row with the arrows or the pointer, and
press Enter or click it; Escape, a click outside it, or Ctrl+K again closes
it. It lists:

- Every action a [hotkey](#hotkeys) can do, by what it is for, each with the
  key bound to it: going live, ending the stream, the panic key, recording,
  Studio mode, TAKE and AUTO, taking or cueing each scene, muting or
  unmuting each sound, and replay and clips. One that cannot run now stays
  listed, dimmed, saying why, such as TAKE with nothing cued or a scene that
  cannot go on program. **Go live** opens the go-live sheet, as GO LIVE does;
  the rest act at once, as their keys do.
- Selecting each layer of the scene BUILD edits, by its source's name.
- Adding a camera, screen, window, or input: the first time, **Find cameras,
  screens, windows, and inputs to add** lists them, then each is a row of its
  own, added as a layer as the Add tab adds it. **Add a source** opens the
  Add tab on any kind of source, to fill in by hand.
- The pages, each Settings pane, and each setting by its name, such as
  **Frame rate** or **Room key**, which opens its pane at that setting.
- Undo and redo, naming the edit, the multiview, the stream's health in
  detail, and the scene graph.

A row is found by the words of its name, what it is for, and a few more it
answers to, such as `fps` for the frame rate or `mic` for a mute, so `mute
desk` finds **Mute `desk`**.

### Multiview

**Open the multiview**, in the show's menu, opens a window for another
monitor with every scene and the sources you pick live in one grid, as large
as the window allows: the scene on air framed in blue and tagged ON AIR, the
one cued (or, off air, the one the stream opens on) framed in amber, and a
blue dot on each source the live picture shows. A source
[gone dark](#a-capture-gone-dark) or [stalled](#a-picture-that-stalls), and
each scene showing it, carries the amber badge, and the source's picture says
so. Over the grid are the show's
name, a clock in this computer's time zone, the stream's health, and how long
it has been live. Click a scene's tile to take it, or cue it in Studio mode,
as its pad does; its hotkey shows on the tile and works with the multiview in
front. The multiview draws on its own, so it stays live while the studio's
window is minimized or behind a game.

Sources that move on their own (cameras, screens, windows, guests and other
broadcasts, web pages, video files, and replays) are on it until you leave
one out, from its tile's right-click menu or the eye in the corner, where you
can also add a still, a color, or a copy. The tiles on air and cued are
redrawn at the program's frame rate, and the rest ten times a second.

**F11**, the square in the corner, or a double-click on the header goes full
screen on the monitor the window is on, and **Escape** comes back. Whether
it is open, where it was, whether it was full screen, and each show's picks
are kept per user, with the [hotkeys](#hotkeys), not in the scene file, so it
opens where you left it next time. Wayland does not let an app place its own
window, so there the desktop picks where it opens.

### Going live

**GO LIVE** opens a sheet that says which scene the stream opens on, then:

- **Where it goes:** the show's relay and its failover relay, each with a
  switch and its state, such as *Connected, 18 ms away*. A relay switched off
  is left out of this stream. A show going out in both formats says each
  relay carries both broadcasts.
- **Checks**, filled in as they finish: whether each camera has a picture,
  whether a microphone's level moves, whether the encoder keeps up with the
  canvas (timed over a short burst of moving frames, keeping up when a frame
  takes at most three quarters of its period), and whether the connection to
  the first relay switched on carries the program (the congestion
  controller's send-rate estimate while a probe publishes at the broadcast's
  path for about six seconds, which a connection without one, such as
  WebSocket, fails; published scenes and sources share what is left). A show
  going out in both formats times both encoders at once, each its own
  picture, so the encoder check passes when each keeps up beside the other, and
  needs two videos' bitrate. If a second encoder cannot open, such as a GPU
  encoder at its session limit, the encoder check fails and says why. A failed
  check is one sentence and its fix,
  such as **Send 720p instead**, which changes the canvas, or **Check again**.
- **GO LIVE TO** the number of destinations switched on. A failed check never
  stops you; only having none switched on does.

Going live dials each relay, failing with why if none answers within 15
seconds, then opens the program's encoders and announces its broadcast. Each
time on air is a broadcast of its own. On air, the live control is the clock;
under the pointer it reads **END STREAM**, and holding it down for a moment
ends the stream, handing the relays its tail first, as quitting does. The
**Go live** and **End stream** hotkeys do the same at once, going live to the
relays switched on without the sheet.

Off air, the program has no past to replay, and a replay of it says so.

### Stream settings

Settings, Stream holds where the show streams, saved in `show.stream.toml`
beside `show.toml` rather than in it, since a scene file is meant to be
shared:

- **Relay**, the one the stream goes to, and its **Token**.
- **Failover relay** and its token: a second relay the same broadcast goes
  to, which needs one `--connect-version` with broadcast instances.
- **Broadcast name**, the path on the relays, `studio` until set.
- **Encode only while someone watches** (on demand), which encodes the
  canvas size only while someone watches (see [Output](#output)).

Tokens are kept in the system's credential store (the Secret Service on Linux,
the login keychain on macOS, the Credential Manager on Windows), one per relay
address, never in a file: a `?jwt=` typed into an address moves there when
saved. Without a credential store, a relay that needs no token still
connects, and saving a token says why it cannot. A setting a flag sets for
this run says so, and is not editable there. A change applies the next time
you go live. A new relay or token is followed for broadcast sources and
guests at once off air, and once the stream ends on air. The recordings
folder (Settings, Recording) and the guests' room (Settings, Guests) are kept
in the same file, as `recordings`, `room`, `room_key`, `join_page`,
`guest_expiry`, and `guests_appear`, beside `earlier`, the names a renamed
show had, whose recordings are still its own.

### Settings

The gear opens Settings, one window over the pages: the categories down the
left, and the one picked on the right. Each line of help names what the
setting costs. Escape or a click outside closes it.

| Pane | Use it to |
| --- | --- |
| Show | Pick the format, landscape or portrait, which swaps the canvas's sides; **Both at once**, which sends a portrait picture beside a landscape one (see [Both formats at once](#both-formats-at-once)); the canvas size as 720p, 1080p, 1440p, or 4K, with the upload each wants, for both pictures while Both at once is on, or **Custom** for exact numbers; the frame rate as 30, 60, or **Custom**; and, under **Scene sizes**, each scene's own size (see [Published scenes](#published-scenes)). Saved in the scene file, and undone like any edit. |
| Stream | Set the relay, its token, the failover relay, the broadcast's name, and on demand (see [Stream settings](#stream-settings)). |
| Recording | Change or open the recordings folder, and pick the picture and the audio scene a recording holds, and whether each source records a file of its own too (see [Recording](#recording)). |
| Hotkeys | Bind keys, by what they are for: going live, ending the stream, the panic key, and recording; Studio mode, TAKE, and AUTO; each scene; each sound's mute; and replay, clips, and markers (see [Hotkeys](#hotkeys)). |
| Guests | Set the room guests join, its key, the join page, how long a link lasts, and whether guests appear on their own (see [Guests](#guests)). |
| Remote | Pair a phone as a remote by its QR code, see which are connected, and remove one (see [Phone remote](#phone-remote)). |
| Advanced | Publish scenes as broadcasts of their own and sources independently, turn the virtual camera of the program or of each published scene on, and open the [scene graph](#scene-graph). |

On air, a new canvas size or frame rate, or a published scene's new size,
asks before it applies, since the video pauses while the new encoders open.
The encoders themselves, the sessions open, and each one's work are in the
[Health](#health) window.

### Scene graph

Settings, Advanced, opens the scene graph in a window of its own: the collection as nodes, each source
with a picture or sound, each scene and the always-on-top layers, and each
audio scene.
A wire runs from each source to every scene or layer placing it, and from
each sound and audio scene to every audio scene holding it; one into a scene
or clone source shows what it draws, and follows from that source. A wire
whose layers are all hidden, or whose sound is muted, is dimmed.

Drag from a node's right edge onto a scene's, the always-on-top layers', or an
audio scene's left edge to wire them: a picture is placed filling the scene,
and a sound or audio scene is sent into the audio scene. Click a wire, then
**Remove**, to take every layer of that source out of the scene, or stop the
send. Each sound's node has its strip's mute, gain, pan, and delay and a waveform
of its last few seconds, each audio scene's node **On program**, **Listen**,
and the waveform of its mix, and each scene's node **Safe**, **Publish**, and the audio
scene it carries. Clicking a scene takes it on air. Every edit lands in the
history, as from anywhere else.

### Published scenes

Every scene has a size, the program's unless **Scene sizes**, in Settings,
Show, gives it its own, such as 1080x1920 for a portrait scene beside a
1920x1080 program. Every scene is drawn at the program's frame rate. Only a
scene of the program's size goes on program: its pad refuses another, saying
why, and an edit resizing the scene on air is refused.

Switch on **Publish** for a scene, in BUILD's inspector with no layer selected,
in the scene graph, or with the **+** of **Publish a scene as its own
broadcast** in Settings, Advanced, to put it live as a broadcast
of its own beneath the program's, at `<broadcast>/<scene name>`, at its own
size, with renditions and encoders of its own (see [Output](#output)), while
the program is on air. A published scene is never switched: it shows what its layers
show, whatever is on program. For a portrait stream that follows the program,
publish a portrait scene drawing the program through a program source (see
[Scene sources](#scene-sources)), with any overlay of its own placed beside
it; the always-on-top layers go over the program alone. It carries the audio scene
on program, or the one picked beside its **Publish** box, encoded as audio of
its own, so picking another never changes the track its viewers play.
Unticking it, or removing the scene, ends
its broadcast. A published scene's name is one segment of its broadcast path,
so it holds no `/`, and a `\` only before another `\`. A scene named `viewers`, where viewers report back,
does not publish while live.

A source captures or decodes once however many scenes show it. A published
scene is drawn only while one of its encoders takes the frame, so with
on demand, or off air, one nobody watches costs no drawing. A
layer's keyframes play in a published scene from when it was published, picked
for recording, or the studio started, whichever came first: a recording and a
broadcast of one scene share one picture.

### Scene sources

A **scene source** draws another scene as one picture, which a layer places,
scales, crops, filters, and blends like any other: a lower third built once
and shown in every scene, or a whole layout shrunk into a corner. Add one in
BUILD's Add as kind **Scene**, picking the scene it draws, or
**Program**, which shows whatever is on air, a transition under way included,
on the same frame, such as for a portrait scene that follows the program,
cropped.

A nested scene is drawn at its own size, transparent where it draws
nothing, once per frame however many layers show it, and only while something
drawn shows it. Its layers' keyframes play from when the scene showing it goes
on, as that scene's own layers' do, so an always-on-top layer, which never
goes on, cannot show a scene with keyframes. A media file or overlay in a nested scene
set to restart starts over when the scene showing it goes on program.

A scene cannot draw itself, even through other scenes, so an edit that would
is refused. A scene showing the program cannot go on program, nor can an
always-on-top layer show it, since either would draw the program into itself:
its pad refuses such a scene, and an edit making the scene on
air show the program is refused. Renaming a scene renames what its scene
sources draw, and removing one removes them, with the layers showing them.

### Transitions

| Transition | What it does |
| --- | --- |
| Fade | The incoming scene fades in over the outgoing one, at an even pace. |
| Slide | The incoming scene pushes the outgoing one off the canvas, both moving left, right, up, or down. |
| Wipe | An edge sweeps across the canvas left, right, up, or down, uncovering the incoming scene behind it. |
| Move | Each layer showing a source both scenes place glides from its place in the outgoing scene to its place in the incoming one, resizing, recropping, and mixing its filters' settings on the way. If its blend mode or kinds of filter differ, it changes look halfway. The rest of the outgoing scene fades out and the rest of the incoming scene fades in. Layers stack as the outgoing scene has them until halfway, then as the incoming one does. A source placed more than once pairs up in order, back to front. |
| Stinger | A media source's clip plays over the program from its start, and the program switches scenes under it at the cut point, the time into the clip when it covers the canvas. It lasts as long as the clip. |

A slide, wipe, or move eases in and out. A transition is drawn on the program
clock, so the stream and a recording show the same frames of it, and taking
another scene while one runs starts from the scene it was heading to. A
transition set to 0 ms is a cut.

A stinger's clip fills the canvas, cropped evenly to the canvas's shape rather
than stretched, through its source's crop and filters, so a clip without an
alpha channel can be keyed. The cut point counts from the clip's first picture
or sound, however long the file takes to start decoding. Its sound joins the
mixes as its source's does. Its source waits blank and silent for the first
stinger rather than playing once when it opens, and rests ended between
stingers, which the Health window does not count unless a scene on air places
it. A clip played to its end holds its last picture, as any media file does,
and a looping one stops, blank and silent, after one pass. A
clip that cannot play, or that is played again from the inspector or
reloaded while a stinger plays it, cuts at once; one stopped there ends the
stinger at once. A cut, such as the panic key's, or another kind of
transition, while a stinger runs stops its clip, sound included; another stinger plays the clip again from
its start.

### Keyframes

A layer's keyframes animate it: a layer can fly in, fade in, or blur into
focus, and a camera can push in. Each keyframe is a time after the layer's
scene goes on and how far the layer is from where it rests then: moved and
resized by canvas pixels, cropped further, and drawn through filters after its
own. Between two keyframes the layer eases from one to the next, linearly or
easing in, out, or (by default) both. Filters of the same kinds, in order,
mix their settings, and others change halfway. A keyframe's filters cannot be
a LUT or a mask. Set them in the inspector's **Animate** tab: add, time, ease,
and remove keyframes, and set how far each moves, resizes, crops, and filters
the layer.

Keyframes play once, from when the layer's scene goes on program, and hold the
last keyframe; before the first, the layer holds the first. A cut or transition
starts them again, while a scene leaving in a transition carries on with its
own. Press **Play** in the Animate tab to play the scene on program's again
from now, or a scene open off air's in its own picture alone. They are drawn
on the program clock, so the stream, a recording, and a replay of the program
show the same frames. Always-on-top layers have none, since they never go on.

Dragging a layer on BUILD grabs it where its keyframes have it and moves
where it rests, so its whole animation moves with it.

### Placing layers

On BUILD, click a layer of the scene on program, or an always-on-top one, to
select it, drag it to move it, and drag its edges or corners to resize it. A
drag snaps edges and centers to the canvas's edges and center, to the other
layers shown, and to the grid while **Grid** is on, drawing a guide through
what it lines up with. A moved layer also snaps to the spacing of its
neighbors, marking the equal spaces. Hold Ctrl (Cmd on macOS) to drag without
snapping, or turn **Snap** off, in the picture's right-click menu. Resizing a
camera, display, window, image, media file, broadcast, or replay keeps its
shape, and resizing anything else does not; hold Shift to flip that. Click the
picture, then the arrow keys nudge the selected layer a pixel, or ten with
Shift held, and a held key undoes as one step; a locked layer stays put. The
snap and grid settings are kept per user, with the [hotkeys](#hotkeys), not in
the scene file.

Layers showing the same source are **linked**: one capture, with the source's
look, wherever it is shown. A **clone** is a new source showing another's
capture with a look of its own, so a camera can be shown faded in one scene
and plain in another while being opened once: **Make independent** and
**Duplicate as its own copy** make one. A layer's own crop and filters apply
after its source's.

Undo (Ctrl+Z) and redo (Ctrl+Shift+Z or Ctrl+Y), in the title bar, step
through the history, and a drag undoes as one step; on macOS each shortcut
uses Cmd in place of Ctrl. **Copy** (Ctrl+C) takes the selected layer with the
source it shows; **Paste** (Ctrl+V) adds it in front of the selected layer's
group, or the scene on program, in this studio or another. Pasted where the
same source exists under the same name, the layer is linked to it; otherwise
the source comes along, renamed if its name is taken. Copied filters paste
onto the selected layer, after its own. The clipboard holds text in the scene
file's format.

### Dropping and pasting

Drag a file from a file manager onto BUILD's picture and it becomes the kind
of layer it holds, where you let go: a PNG or JPEG becomes an image layer, and
a video or sound file a media layer, each at its own size, no larger than
half the canvas, in front of the edited scene, and selected. A sound alone
has no picture, so it is only sent to the audio scene on program, as anything
with sound is. A `.cube` file grades the layer it is dropped on, in place of
any grade it had. While the drag is over the window, the picture outlines
where the layer will land and names what it will become, and Add lights that
kind. A file dropped on an image or media layer of its own kind asks whether
to replace that layer's file, which every layer showing its source shows, or
add a layer. Dropped anywhere else in the window, or on another page, it lands
in the middle of the edited scene.

A file the studio cannot play says why where it was dropped, and how to fix
it where it can: a regular MP4 names the remux that makes it readable. The
file is read before it is placed, in the background, so a large video holds
up nothing while it is read.

A dropped file outside the scene file's directory is copied there, since
sources name files relative to it: `logo.png`, or `logo 2.png` beside a
different `logo.png`, while a copy already there with the same contents is
reused. One already under that directory is named where it is.

**Paste** (Ctrl+V, or the picture's right-click menu) does the same at the
pointer, and the title bar's **Paste** at the scene's center: files copied in a file manager; a picture, such as a screenshot,
saved as `Pasted picture.png` beside the scene file; and a web address, as a
web page layer the size of the canvas, shown at half that size.

The window hears drops on Windows, macOS, and X11. On Wayland it hears none
yet, so paste instead; a web address dragged from a browser is not heard on
any platform, so paste it too.

A broadcast source subscribes through the show's relays, on air or off,
over sessions of their own that only subscribe. It plays its broadcast
through the first relay while that relay has a route to it, and
through the failover otherwise, so a publisher restarted behind only the
failover replaces the first relay's copy once that copy ends. It plays the
best video and audio its catalog lists, switches when the catalog changes, and
comes back on its own when the publisher restarts. While it is offline, or
once a new stream replaces the one it played, it shows nothing until the new
stream's first picture, rather than freezing on the last one. A broadcast
without video but with an image, such as a studio's image or color published
independently, shows that image, and never reads as stalled, since it sends
a picture only when it changes.

A broadcast source, a guest's included, plays on its publisher's timestamps,
so its sound and picture stay in sync through network jitter. It plays as far
behind what arrives as the network's jitter needs, measured as it goes as
`moq play --delay auto` does, plus 50 ms to hand its sound to the mix, up to
2 s in all. When that delay moves, the sound speeds up or slows down by about
a tenth until it is back on schedule, as `moq play`'s does, rather than
skipping or falling silent: a move of 100 ms takes about a second. A move
earlier by more than 200 ms skips, and one later by more than about 130 ms
falls silent until the sound is due. Sound and picture each keep that
schedule on their own: a sound stall leaves the picture playing, and the
sound comes back on the clock, skipping what it missed by more than 200 ms
and speeding up through the rest; a picture stall leaves the sound playing,
with the canvas showing the last picture meanwhile. Without audio, the picture plays 100 ms behind. The inspector's Source section shows how
far behind each broadcast plays.

### Slots and roles

A scene can lay out some of its layers by **slots** rather than by hand: one
**main** slot, the largest, and **tiles** beside it, in the order of their
layers. The scene's **arrangement** places them, each slot 16:9,
inside a margin, each picture covering its slot, cropped evenly to fit:

| Arrangement | Places |
| --- | --- |
| Side column | The main slot on the left, the tiles stacked in a column on the right. |
| Bottom row | The main slot on top, the tiles in a row under it. |
| Grid | Every slot the same size, in as square a grid as fits them. |
| Picture in picture | The main slot fills the canvas, the tiles small, stacked up from its bottom-right corner. |

A layer in a slot closes up when it has nothing to show: when it is hidden,
when its source is offline or failed, or when it shows a role nothing fills.
The others grow into its room, and they glide there over 0.4 s on program, as
a Move transition does, so a guest joining or leaving moves the rest rather
than cutting them. Layers set free, by their own `rect`, stay where they are,
so a scene can mix both: a backdrop behind the slots and a logo over them.

A **role** is a source that names another, once for the whole show:
`Main`, the main picture, or `Camera`, the host's camera. A layer showing a
role shows its source, through that source's crop and filters, and filling
the role with another source, from the scene inspector's Slots or the
layer's Source section, changes it in every scene at once. A role filled by
nothing draws nothing, and its slot closes up. Removing the source filling a
role leaves the role unfilled. A role takes no look of its own, and is not
cloned or published; its layers take their own.

The scene inspector's **Slots** section picks the arrangement, lists the
slots with the role or source in each and what fills each role, and adds a
**Main slot** or **Camera slot**, making the role if the show has none. Add, under
**Not in any scene**, puts a guest into the scene's next tile when it has
slots.

**Appear on their own**, in Settings, Guests, has a guest whose camera comes
online take the next tile of the scene on program, and be heard on program,
as an edit you can undo. Up to four guests still in the room take tiles that
way, and more wait under Add. One who leaves keeps their tile, closed until
they are back, and frees their place: the next guest takes a new tile, or
once four tiles hold guests, a tile the studio gave one who has left. They
are no longer heard on program unless another layer shows them, and wait
under Add when back, as they appeared once already. A tile you gave a guest
yourself stays theirs, as does a copy of one, and the scene file marks the
studio's own with `auto = true`. A guest appears on their own once per link,
across restarts: after that, where they are shown is yours, so undoing their
appearance or removing their layer keeps them off when they reconnect, until
a new link. Since an appearance is your edit, it drops what you could redo.
Off, every guest waits under Add, as before.
### Keeping out of the way

A scene's layer with **Keep out of the way** on, in the inspector's Place
section, moves itself off what matters in the picture under it, such as a
facecam off a game's minimap or a slide's text. A few times a second the
studio draws the layers under it, scaled far down, and reduces them on the
GPU to a grid: where the picture moves, where it is dense with edges that hold
still, as text is, and what holds still while the rest moves, as a HUD does.
Each can be switched off under **Stays clear of**. Nothing is uploaded: the
choice is made on this machine, from the picture alone.

It moves to one of eight places, the canvas's corners and edge middles,
inside a margin of a twentieth of the canvas's shorter side, that its size
fits in, taking the one that covers the least of what matters, a corner over
a middle that covers as much. It chooses a second after its scene goes on,
about two seconds after the content under it changes kind (another layout, or
another app), and otherwise moves only once its place has been the worst of them for
10 seconds, never for a single busy moment. It moves only to a place that
covers clearly less, so over a picture where nothing matters it stays where it
rests, even outside the eight places. What it covers is where its keyframes
draw it, and it never goes where they would draw it off the canvas. It glides
to a new place as a move transition eases, in 0.4 s unless **Glide** says otherwise. Its rect in the
scene file gives its size, and where it rests until it first moves.

While it is selected, BUILD shows what it saw: what matters glowing, tagged
**MOVING**, **TEXT**, or **HUD**, the places it may go, dashed, and where it
last moved from, with what it left there and an arrow to where it went. Drag
it, nudge it, or type its X or Y, and it stays where it was put until the
content changes, as it does where a redo or a reload of the scene file
moves it, or an undo moves it anywhere but back before such a move. Undo
that move and it goes back to where it was, such as the corner it had
glided to. A resize keeps it free to move. Switched off, it stays where it
is drawn, and undoing that puts it back keeping clear where it was. Only a
scene's layer set free of its [slots](#slots-and-roles) keeps out of the
way, and not one showing a source
[published independently](#published-independently), which viewers place
themselves; putting one in a slot switches it off. While the show goes out in
[both formats at once](#both-formats-at-once), it moves only in the landscape
picture: in the portrait one it stays at the place set there, or fitted from
where its rect rests.

### Both formats at once

A landscape show can go out in portrait too, for phones, beside the
landscape picture for desks and TVs: switch on **Both at once** in Settings,
Show. Each scene of the show's size then has two arrangements of the same
layers, and the studio draws, encodes, and sends both pictures. A scene of its
own size, such as a published portrait scene, stays one picture, in its own
shape, the same in both down to the scenes it shows, so it cannot go on
program meanwhile. A scene given the show's size as
its own follows the show's size instead when you turn it on.

Turning it on is one edit you can undo, and starts each portrait arrangement
from the landscape one. The largest picture, covering a quarter of the canvas
or more, is the main one: it is cropped to the top 60% of the portrait canvas,
with the largest camera stacked under it, cropped to fill the rest, or fitted
there keeping its outline if it has a round or rounded shape. Without a
camera, the main picture covers the whole portrait canvas. A backdrop covering
the landscape canvas covers the portrait one, cropped, but a web page or
overlay covering it, mostly see-through, is fitted whole and is never the main
picture. What sits on the main picture, such as a lower third or a logo, keeps
its place on it. A layer set anywhere else keeps its place on the turned
canvas, scaled to fit. A scene laid out by slots arranges them in a row in
portrait, or in a grid or inset if that is how it arranges them in landscape,
and picks its own portrait arrangement in BUILD's scene inspector. A layer
showing another scene of the show's size, or the program, shows that scene's
portrait arrangement in the portrait picture, tall as the portrait canvas is,
so its place there is shaped for it: one covering the landscape canvas, such
as a scene of overlays, covers the portrait canvas whole rather than being the
main picture, and one set anywhere else keeps its place, as wide and as much
taller as its picture turned. Giving that scene a size of its own in Settings,
or the show's back, changes the shape of its picture there, so each layer
showing it is fitted inside the place it had in the portrait picture, in the
same edit.

LIVE shows both pictures side by side, each in its tally frame, with its
format, size, and what its video sends under it. BUILD edits one picture at a
time: **Landscape** or **Portrait** under the picture picks which, and the other
shows small beside it, where a click edits it instead. A drag, Fit, Fill, the
inspector's X, Y, W, H, and crop all place the layer in the picture edited,
and leave its place in the other alone, even one fitted there from its
landscape place. A layer in a slot has one crop in both.
Each scene's pad shows the pair, and Studio mode's NEXT card shows both
pictures of the scene cued.

A layer is shown in both pictures, or only in one: **Format** in its
inspector, and the chip under the pictures naming the selected layer, picks
**Both**, **Landscape**, or **Portrait**. A layer shown only in landscape keeps
no portrait place. Switching **Both at once** off removes every layer shown
only in portrait and forgets every portrait place and arrangement, as one
edit.

Only a landscape canvas turns: picking Portrait in Settings, Show, switches
Both off. The portrait picture goes out as its own broadcast, at
`<broadcast>/portrait` (see [Output](#output)), so a published scene cannot
be named `portrait` meanwhile.

### Audio scenes

Audio from every source is mixed whether or not its scene is live, and cuts
never change it. A collection has one or more **audio scenes**, each the sum
of the sounds it holds and of the audio scenes it holds: for example a "Mics"
audio scene holding every microphone, which both a "Stream" audio scene (Mics,
without music you may not broadcast) and a "Headphones" one (Mics and the
music) hold. A sound an audio scene reaches more than one way plays in it
once, and an audio scene never holds itself, even through others. Add,
rename, reorder, and remove audio scenes on the MIX page.

The MIX page is a mixing desk. Each sound has a strip: its name, a meter, a
fader for its gain in decibels (unity is marked, and a double click returns
to it), its gain to read or type, and its mute, which turns amber while it
is on. These apply in every audio scene holding the sound. Click a sound's
name to open its strip to its gain, pan (a double click centers it), and
delay, and the audio scenes it is heard in; the other strips narrow beside it
rather than hide. Each audio scene has a strip after the sounds with
**Listen** and its meter, on the same scale, which reads the audio scene as
**Listen** plays it, published sources included. A meter's top is full scale,
where its last segment turns red: the sound clips.

Under each strip is a dot for each audio scene, marked with its initial:
filled where the strip is sent into it, the accent's color for the one the
stream carries; ringed where it is heard there only through another audio
scene; and faint where it is not heard. A click sends it in or takes it out.
An audio scene shows a dash under its own strip and under any it holds, which
it cannot be sent back into. A sound [published independently](#published-independently)
keeps its dots and sends: they say where people in the studio hear it, while
viewers hear it from its own broadcast and the audio the studio encodes
leaves it out. **Routes** shows the same sends as a matrix, a
row for each sound and audio scene and a column for each audio scene, with
what each one holds at its foot.

A source added from BUILD's Add, or pasted, is sent to the
audio scene on program, so it is heard at once. The audio scene on program is
tagged **Stream hears this**; **Put on stream** on another's strip puts that
one on program instead, and switching it, or editing an audio scene, never
drops or repeats audio. **+** after the audio scenes adds one, holding
nothing. Right-click an audio scene's strip, or its column under **Routes**,
to rename it, move it left or right, or remove it. A recording holds the
audio scene on program unless Settings, Recording picks another, such as one keeping music the stream leaves out, which is tagged
**In the recording**, and each published scene carries the one on program
unless BUILD's inspector picks another beside its **Publish** switch; switching
either while it runs never drops or repeats audio either. The audio scene on
program cannot be removed; removing one a recording or published scene picks
returns it to the one on program.

**Listen** on an audio scene's strip plays it to an audio output, from the
`capture` feature's build: the system default, or the one picked from the tag
on the strip while it plays, which the studio remembers for you rather than
in the scene file. Use headphones: a microphone that hears the speakers feeds
its audio scene back into itself.

A source's **delay** holds it back in every audio scene, up to 2 s, so a microphone
lines up with a camera that lags it. A broadcast's picture already follows its
sound, so its delay holds the sound back from its own picture: use it only for
a publisher whose sound leads their picture.

Each audio scene is mixed on the program clock, and each output reads its
audio scene on its own, so an output that falls behind skips ahead in its own
audio rather than holding up the mixing or another output.

A browser source renders a web page, such as an overlay, alert box, or chat
widget, at the canvas frame rate (CEF caps it at 60) over a transparent
background, and its sound joins the audio scenes that hold it. Every page shares one
browser profile, kept in `$XDG_DATA_HOME/moq-studio/browser`
(`~/.local/share/moq-studio/browser` by default) on Linux and in
`~/Library/Application Support/moq-studio/browser` on macOS, rather than
beside the scene file, so cookies and logins, such as an alert service's,
survive a reload and a restart of the studio. They are not encrypted with the
desktop's keyring or keychain, so a locked keyring never holds a page back and
no keychain prompt interrupts one, and anything that can read your home
directory can read them. One studio at a time can use the profile: a
second one's browser sources fail saying so. Each page renders in a process of its own, so
a page that crashes fails only its own source, with the reason, while the
stream carries on. A crash of the browser itself fails every browser source.
Either shows at once, since the browser's processes dump no core. Each page
renders in Chromium's sandbox, so a page that exploits a browser bug still
cannot read your files, keys, or stream credentials.
The browser requests only what its pages load, never asking Google for
updates, the time, or autofill suggestions on its own, so pages are not
spellchecked, which would need a dictionary from Google. Nor does it run
browser extensions, even ones your distribution installs for every Chromium.
**Reload** in Sources restarts a page, failed or not. A page's dropdown
lists, such as a `<select>`'s, draw over it and take clicks under
**Interact**, but it opens no other windows (`window.open` and new-tab links
do nothing). Copy and Cut there put the page's selected text on the clipboard,
and nothing from a password field. On Linux, except for a page copied on the
GPU through an X display, the text goes as plain text that the page's own copy
handlers cannot change; on macOS the page copies as it would in a browser.

On Linux the sandbox needs unprivileged user namespaces, and a studio run by
a user other than root. Where either is missing, browser sources fail saying
so rather than render a page unsandboxed. Ubuntu refuses them, since
23.10, to any program an AppArmor profile does not allow; as root, allow the
studio, naming the `moq-studio` you run:

```bash
cat >/etc/apparmor.d/moq-studio <<'EOF'
abi <abi/4.0>,
include <tunables/global>

profile moq-studio /path/to/moq-studio flags=(unconfined) {
  userns,
}
EOF
apparmor_parser -r /etc/apparmor.d/moq-studio
```

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
studio reads them all as the overlay opens, so its first frame has them, and
a file changed on disk changes nothing on air until the overlay opens again,
as when it is edited; a restart on program reuses what it read. Images are PNG or JPEG, at most 4096 pixels a side;
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
it, and again whenever it is edited. It also starts over each time a scene
showing it goes on program, as a layer's keyframes do, so a lower third plays
its entrance at every cut.
Set `restart = false` (**Restart on program**, off, in the add form) for one
that runs on across cuts, such as a clock or a ticker. One that program does
not show costs nothing until it is shown again, where its animation has
carried on unless it restarts.
Overlays publish no track of their own, so viewers see them only in the
program.

### Media files

A `media` source plays a video or audio file, such as an intro clip or a music
bed, on the program clock, and its sound joins the audio scenes like any other
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
as ended on its layer and in the inspector and, while the program or a published scene shows or
plays it, in Health.
**Play** in the inspector plays it from its start again, as does going on program
with `restart`, in a scene on air or always on top, when it was not on program
before. Either clears its picture at once, so the end of the last play never
shows on program. **Stop** in the inspector stops it at once: blank, silent, sound
already queued included, and reading as ended until played again. A file that
cannot be read or decoded fails, with the reason.
The source a [stinger](#transitions) plays waits, blank and silent, for the
first stinger instead of playing as it opens. Its pictures and its sound each have a track of their own (see [Source
tracks](#source-tracks)), its pictures on one timeline across plays. Each is
used apart, as a camera's and a microphone's are: the pictures while a scene on
air shows it, the sound while the audio scene on program plays it, and either while
read. So a music bed that no scene shows encodes its sound alone. A file with
no sound has no strip on the MIX page.

### Picked displays and windows

On Linux with the `pipewire` feature, a `picked_display` or `picked_window`
source opens the desktop's screen-sharing picker (xdg-desktop-portal) the
first time it opens, and shows whichever display or window you choose there.
It is the way to share a screen or window on a Wayland desktop, where the
studio cannot list them itself. BUILD's Add, among this machine's devices,
and the quick start's Screen offer them as **Picked display** and **Picked
window**, first on a Wayland desktop, and Add's form for any source as **Display (system
picker)** and **Window (system picker)**.

The scene file records only that the source is picked, not what you picked,
so it stays portable. What you picked is the portal's grant, kept per machine
in `$XDG_STATE_HOME/moq-studio/portal.toml` (`~/.local/state` by default),
readable by you alone, under the scene file's path and the source's name. So
the source reopens without the picker, across restarts, on a desktop that
keeps grants. The picker shows once again when the grant cannot be used: on
another machine, after the scene file moves or the source is renamed, when
the desktop refuses the grant, or after you stop sharing from the desktop,
which revokes it. Cancelling the picker or stopping sharing fails the source
until its **Reload**, in the inspector, asks again. Removing a source
forgets its grant. Studios open at once share the file safely.

## Guests

Guests and phones join a room the studio owns on its relay, from a web page,
and their camera and screen appear as broadcast sources. The page plays the
show back to them, without their own voice. The room follows
[`@moq/room`](https://github.com/moq-dev/moq/tree/main/js/room): a guest
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

Settings, Guests, sets the room, its key, the join page, and how long a link
lasts, and keeps them in the show's [stream settings](#stream-settings) file as
`room`, `room_key`, `join_page`, and `guest_expiry`. It refuses a room guests
could not join, saying why, before it saves. A saved room is joined at once
off air; on air, the guests already in the old room stay until the stream
ends, and the new room is joined then. The guest flags set the room for one
run instead, and the pane shows a field a flag sets as the flag's. A room
that fails to load no longer stops the show from opening: BUILD's Add says
why under Guests, with a button to the pane.

- `--room` is a path under the relay's URL, here `show/guests` on the
  relay. The studio's own token must let it subscribe there. A guest's browser
  dials the same relay, so it must be `https://`, or `http://` only to
  this machine or the local network.
- `--room-key` signs each guest's token. The studio refuses a key whose scope
  reaches outside the room, so a stolen key cannot sign for the rest of the
  relay. Put its public half in the directory `moq auth serve --key-dir` reads.
- `--join-page` is where the join page (`js/join`, `bun run --cwd js/join
  build`) is hosted. Browsers open the camera only on an `https://` page or
  `localhost`, so the studio refuses any other. It defaults to
  `https://join.studio.moq.dev/`, which this repository deploys from `main`.
- `--guest-expiry` (default `4h`, at least `1m`) is how long a link works,
  and each token the studio renews it with. While a guest is in the room, the
  studio signs them a fresh token every half of it, over a session of its own
  in the room that the room key signs for, and the join page presents it
  without reconnecting. A guest's token may read only their own renewals and
  return feed, at `.renew/<name>/<generation>` and `.return/<name>/<generation>`
  in the room, and the program at `.program`, but anything else the relay lets
  subscribe there can read every guest's, so grant that only to the studio.
  The same session publishes each [phone remote](#phone-remote)'s desk under
  `.remote` and reads their presses under `.press`. A guest who
  leaves needs a link, or a renewed token, that has not expired to come back.
  BUILD's Add says, under Guests, when renewal is failing.
- **Upgrading from a studio without renewal:** generate a new room key, drop
  the old one's public half from `--key-dir`, then restart the relay (or wait
  out the old links' `--guest-expiry`) before starting the upgraded studio.
  Links from before let their holder subscribe to the whole room, renewals
  included, so they must die with the old key, and a relay keeps a session
  it already admitted until its token expires.
- With a failover relay, a link still names only the first relay: the join
  page dials one relay, and a guest's token is rooted at the room there, so
  guests drop while that relay is down. The studio follows the room through
  both relays, like any broadcast source, so a guest the failover routes at
  the same path, as a relay in the same cluster does, keeps reaching it.
  Anyone the failover lets publish in the room joins as a guest, so guard the
  room there as on the first relay.

**Invite**, under Guests in BUILD's Add, makes a link for one guest name and shows it as
a QR code; **Copy link** puts it on the clipboard instead. The guest's token is
in the link's fragment, which the browser never sends to a server, and lets
them publish only under their own name, so they cannot appear as someone else,
and read only their own renewals, their return feed, and the program.
Treat the link like a password: anyone holding it can join as that guest
before it expires, and is then renewed for as long as they stay, until you
remove them. A guest's broadcasts become sources named after them (`alice`,
`alice screen`) when they first appear, in no scene or audio scene: place
them from BUILD's Add and send them to the audio scene on program on MIX
to put them on air, have them [appear on their own](#slots-and-roles) in
the next tile of the scene on program, or switch on **Published
independently** to have viewers fetch them from the guest instead of through
the program (see [Published independently](#published-independently)). That
includes a
guest invited before a restart whose link still works. A guest's source is
saved to the scene file but is not your edit: it stays out of History, so undo
never removes it and a guest joining never loses what you could redo, unless
they [appear on their own](#slots-and-roles). A guest
who reconnects after you removed their source comes back under a new name
(`alice 2`), since undoing the removal brings back the old one.
**Remove from scenes** is your edit, so it undoes: it takes a guest, any clone
of their sources, and any mask reading them out of every scene and the top
layer and mutes them. A signed token cannot be taken back, so it also stops
renewing theirs and ends their return feed, and they are disconnected within
`--guest-expiry`. That
holds across a restart, whichever scene file you open: removed guests, and
those who appeared on their own, are kept beside the room key, as
`room.guests.toml` for `room.jwk`, so its directory must be writable. Undo puts them back in the scenes but not on
renewal; **New link** lets them back in and renews them again, under a new
generation, so a link from before the removal cannot read their renewals.

While a guest is in the room, the join page plays their return feed: the
program's video, and the audio scene on program without that guest's microphone (a
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

## Phone remote

A phone paired as a remote drives the show from wherever it reaches the
relay, with nothing installed: its page shows the scene pads with their
tally (ON AIR, FIRST, or NEXT, as the studio's own pads), TAKE and AUTO in
Studio mode, each sound's mute, and the live clock, and the whole page is
framed in the accent while the program is on air. A pad the studio cannot
take now is greyed out. Each press shows in the studio's status
bar, as a key's does, and on the phone.

Settings, Remote, pairs one: name it, **Pair a phone**, and scan the QR code
with the phone's camera, or **Copy link** to send it. A remote joins the
[guests' room](#guests), so it needs the room, its key, and the join page set
there first; its page is `remote.html` beside the join page, so a join page
in a directory ends with `/` (`https://example.com/join/`), and the link
lasts as long as a guest's (`--guest-expiry`). The pane lists each phone
paired, whether it is connected, and its link, with **Show link** while that
link still works, **New link**, and **Remove**. Treat the link like a
password: anyone holding it can drive the show until you remove it.

- A remote presses only what is safe to press by mistake from a pocket: a
  scene's pad, TAKE, AUTO, and the mute of a source with sound. Going live,
  ending the stream, the panic key, recording, and replay stay on the studio.
- A remote's token, signed by the room key like a guest's, may publish only
  its presses, at `.press/<name>/<generation>` in the room, and read only the
  desk the studio publishes it at `.remote/<name>/<generation>`, which carries
  its renewed tokens. Remote names are their own, apart from guests'.
- The studio reads a remote's newest press only, at most one a program frame,
  and refuses one that is not a single action of at most 4 KiB, so a page
  pressing faster costs the studio nothing more. The page sends its next
  press once the desk says how the last went, and the desk catches up with
  what changed at the studio four times a second. A press the phone made
  before the studio restarted or rejoined the room is not done again, since
  it was most likely done before: the page asks you to check it first.
- **Remove** stops reading its presses at once, and stops renewing its token,
  so it is disconnected within `--guest-expiry`, and no press of it is done
  once Remove says it is removed; **New link** pairs it again
  under a new generation, which a link from before cannot reach. Pairings and
  removals outlast a restart, kept beside the room key as
  `room.remotes.toml` for `room.jwk`. While the studio moves to another room,
  or rejoins this one with new settings, pairing, removing, and inviting
  guests are refused until it has joined, so the next room misses none.

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

[sources.Main] # a role: the show's main picture, wherever a scene shows it
kind = "role"
source = "camera" # the source filling it; none draws nothing

[sources.alerts]
kind = "browser"
url = "https://example.com/alerts"
width = 800 # the page's own size, whatever its layers stretch it to
height = 600

[sources.name]
kind = "overlay"
width = 1920 # the overlay's own size, like a page's
height = 1080
restart = true # start over each time it goes on program, the default
html = '''
<style>
  .third { position: absolute; left: 80px; bottom: 120px; padding: 16px 32px;
    background: linear-gradient(90deg, #1e3a8a, #2563eb); border-radius: 12px;
    font: bold 48px sans-serif; color: white; animation: in 400ms ease-out both; }
  @keyframes in { from { transform: translateX(-40px); opacity: 0 } }
</style>
<div class="third">Ada Lovelace</div>
'''

[sources.music]
kind = "media"
path = "music.webm"
loop = true

[audio.guest]
gain = 0.5
pan = -0.5
delay = "80ms"

[sources.camera-faded]
kind = "clone"
source = "camera"

[sources.main-view]
kind = "scene"
scene = "Main" # draws the Main scene as one picture

[sources.on-air]
kind = "program" # whatever the program shows

[video.camera-faded]
filters = [{ kind = "opacity", amount = 0.5 }]

[sources.grade]
kind = "lut"
path = "teal-orange.cube"

[video.camera]
background = "remove" # cut the person out, leaving the rest transparent
crop = { left = 0.1, right = 0.1 } # trim 10% of the picture off each side
filters = [
    { kind = "chroma_key", color = "#00ff00", similarity = 0.4, smoothness = 0.08, spill = 0.1 },
    { kind = "lut", source = "grade", amount = 0.8 },
]

[[scenes]]
name = "Main"
items = [
    { source = "camera", rect = { x = 0, y = 0, width = 1920, height = 1080 }, locked = true },
    { source = "guest", rect = { x = 1280, y = 720, width = 640, height = 360 }, shape = { corners = [0.12, 0.12, 0.12, 0.12], border = 4 }, filters = [{ kind = "drop_shadow", x = 8, y = 8, blur = 12, color = "#00000080" }] },
    { source = "logo", rect = { x = 0, y = 0, width = 1920, height = 1080 }, blend = "multiply", filters = [{ kind = "opacity", amount = 0.3 }] },
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
    { source = "main-view", rect = { x = 80, y = 80, width = 640, height = 360 } },
]

[[scenes]]
name = "Guests"
arrangement = "column" # column (the default), row, grid, or inset
items = [
    { source = "Main", slot = "main" }, # a slot takes no rect
    { source = "guest", slot = "tile" },
    { source = "logo", rect = { x = 1600, y = 40, width = 280, height = 120 } },
]

[[scenes]]
name = "Game"
portrait = { arrangement = "row" } # its slots in portrait, while the show goes out in both
items = [
    # Its place in the portrait picture, while the show goes out in both.
    { source = "game", rect = { x = 0, y = 0, width = 1920, height = 1080 }, portrait = { rect = { x = 0, y = 0, width = 1080, height = 1152 }, crop = { left = 0.2, right = 0.2 } } },
    { source = "logo", rect = { x = 1600, y = 40, width = 280, height = 120 }, format = "landscape" }, # shown only in landscape
]

[[scenes]]
name = "Portrait"
size = { width = 1080, height = 1920 } # its own size; the program's without one
published = true # live at <broadcast>/Portrait
recorded = true  # recordings hold this scene rather than the program
items = [
    { source = "on-air", rect = { x = 0, y = 0, width = 1080, height = 1920 }, crop = { left = 0.34, right = 0.34 } },
    { source = "logo", rect = { x = 780, y = 40, width = 280, height = 120 } },
]

[[audio_scenes]]
name = "Voices"
sounds = ["guest"]

[[audio_scenes]]
name = "Stream"
program = true # the audio scene the broadcast carries
scenes = ["Voices"]

[[audio_scenes]]
name = "Headphones"
recorded = true # recordings hold it rather than the one on program
scenes = ["Voices"]
sounds = ["music"]

[[top]]
source = "logo"
rect = { x = 1600, y = 40, width = 280, height = 120 }
```

Source kinds are `camera`, `display`, and `microphone` (each with an optional
`id`, the default device without one), `window` (with an `id`),
`picked_display` and `picked_window` (no fields; see
[Picked displays and windows](#picked-displays-and-windows)), `system_audio`
(macOS), `image` (PNG or JPEG), `media` (a video or audio file; see
[Media files](#media-files)), `color` (`#rrggbb` or `#rrggbbaa`),
`broadcast`, `browser` (a `url`, and a `width` and `height` up to 4096),
`overlay` (its `html`, a `width` and `height` up to 4096, and whether it `restart`s, true by default), `clone` (with the `source` it shows), `role` (with the `source` filling it, if any; see [Slots and roles](#slots-and-roles)), `lut` (a 3D Adobe or Resolve `.cube`
file, which no layer places, but which grades what a `lut` filter names it in),
`replay`, where REVIEW plays replays, `scene` (with the `scene` it
draws), and `program` (see [Scene sources](#scene-sources)). BUILD's Add fills in
ids.

A scene's `size` gives it a `width` and `height` of its own; without one it is
the program's. `published = true` puts it live as its own broadcast (see
[Published scenes](#published-scenes)), so its name is one path segment, and
`recorded = true` has recordings hold it rather than the program, on one scene
at most. `safe = true` marks the scene the panic key cuts to, on one scene at
most, which must be able to go on program.

A scene's `audio` names the audio scene it carries while published, the
one on program without it, and its `arrangement` how it places its slots:
`column`, the default, `row`, `grid`, or `inset` (see
[Slots and roles](#slots-and-roles)).

`[[audio_scenes]]` lists the audio scenes, in the order MIX lists
them, each with its `name`, the sources whose `sounds` it holds, and the other
audio `scenes` it holds. Exactly one is on `program = true`, and
`recorded = true` has recordings hold one rather than the one on program.
Without `[[audio_scenes]]`, the collection has one audio scene, `Program`, on
program and holding nothing. `[audio.<source>]` sets a source's linear `gain`,
`muted` (which the strip's mute and the panic key set), `pan` (`-1.0` hard left to `1.0` hard right, centered by
default), and `delay`, in every audio scene holding it.

`[published.<source>]` publishes a source
[published independently](#published-independently), even empty: its
`priority` (0 by default) ranks its tracks against the program's for
viewers, and `toggle = true` lets each viewer switch it off.
Scene files from before named them in a `published = [...]` list, which no
longer loads: write a `[published.<name>]` table for each. A history saved
before then that holds a publish edit fails to load the same way, naming
`show.history.toml` beside the scene file: delete it to start a new history,
so undo no longer reaches the edits before. A history without one loads.

`portrait = true`, a top-level key, sends a portrait picture beside the
landscape one (see [Both formats at once](#both-formats-at-once)). Meanwhile,
in a scene of the show's size or `[[top]]`, a layer's `format`, `landscape`
or `portrait`, shows it only in that picture, both without one, and its
`portrait`, a `rect` and `crop`, places it in the portrait picture, where it
sits on the landscape canvas, fitted to the turned one, without one. A layer
in a slot, or shown only in landscape, takes no `portrait` place, and a
scene's `portrait = { arrangement = "..." }` arranges its slots there, `row`
without one.

Each entry of a scene's `items` is one of its layers, back to front, and each
`[[top]]` an always-on-top layer: the `source` it shows and its `rect` on the
canvas, in pixels, or, in a scene, its `slot`, `main` (one per scene) or
`tile`, in place of a `rect`, and optionally `visible = false`, `locked = true`, which
the picture's pointer passes through, and its `crop`, `filters`, `blend`,
`shape`, `keyframes`, and `keep_clear`.

A scene's layer with `keep_clear = {}` [keeps out of the
way](#keeping-out-of-the-way) of motion, text, and a HUD; `motion = false`,
`text = false`, or `hud = false` in it ignores one, and `glide` sets how long
a move takes, `"400ms"` unless it says otherwise, up to `"5s"`.

A layer's `shape` cuts it to an outline, before its filters, so a drop shadow
follows it: `corners`, how round each corner is (top-left, top-right,
bottom-right, then bottom-left) as a fraction of the layer's shorter side,
from 0, square, to 0.5, and a `border` that many canvas pixels wide inside the
outline, white unless its `color` says otherwise. A layer showing a source
published independently takes no shape.

`[video.<source>]` sets a source's `background`, `crop`, and `filters`,
wherever it is shown, and a layer's `crop`, `filters`, and `blend` its own.
`expect_dark = true` on a camera, display, or window says it is meant to go
dark at times, so the studio never flags it [gone dark](#a-capture-gone-dark).
`background = "remove"` cuts the person out of a camera, display, window,
image, media, broadcast, browser, or replay source's picture before its crop
and filters, leaving the rest transparent. A segmentation model finds them in
each picture on the CPU, so each picture shows only once its cut-out is
ready, a frame or more late, and a model that falls behind skips pictures
rather than queue them. It is MediaPipe's selfie segmentation model, made for
one or two people near the camera. A clone shows its source's cut-out, so it
cannot remove a background of its own. A `crop` trims a fraction
(0 to 1) of the picture off its `left`, `top`, `right`, and `bottom`, opposite
sides together trimming less than the whole, and stretches the rest over the
layer; a layer's crop trims what its source's
leaves. Filters apply in order, the source's then the layer's, with lengths in
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
| `mask` | a `source` whose picture, stretched over the layer, keeps what it covers: by its alpha, or `mode = "luminance"` |

A layer's `blend` mixes it with what is under it by a CSS `mix-blend-mode`:
`multiply`, `screen`, `overlay`, `darken`, `lighten`, `color_dodge`,
`color_burn`, `hard_light`, `soft_light`, `difference`, `exclusion`, `hue`,
`saturation`, `color`, or `luminosity`. Removing a source removes the filters
that name it.

A layer's `keyframes`, in order of time, each have an `at`, how long after the
scene goes on, and optionally an `ease` into it from the one before (`linear`,
`in`, `out`, or `in_out`, the default), `x`, `y`, `width`, and `height` added
to the layer's `rect`, a `crop` of what the layer's crop leaves, and `filters`
applied after the layer's own.

The studio writes the file a second after the last change, so a burst such as a
slider drag is one write, and once more on quit. Edits that never pause for a
second are written five seconds after the first, so a crash loses at most the
last five seconds of edits. Each write replaces the whole file at once, never
leaving half of it, and a collection that has not changed is not rewritten, so
a hand-written file keeps its comments and layout until the next edit in the
studio. A failed write shows over the page and
is retried every second; until one succeeds, a crash loses every edit since the
last write.

The file can be edited by hand, or by another studio, while the studio runs.
The studio looks at it every second and before each write. With no edits of
its own unsaved, it reloads the change, as **Reload the scene file** does, which
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
studio says how many as it opens. A history file that cannot be read stops the
studio, naming the file; delete it to start a new history. The history holds
whatever the scene file held, including removed sources, so check it too
before sharing.

## Output

The program publishes as two H.264 renditions, at the canvas size and at half
of it, at the canvas frame rate, with the audio scene on program as Opus stereo audio,
under the broadcast's path, while on air. A canvas under 360 lines has no
half size. Each published scene publishes the same way at
`<broadcast>/<scene name>`, at
its own size and half of it, its tracks sharing the connection's bandwidth
with the program's, and its own encode of the audio scene it carries. A show
going out in both formats publishes its portrait picture the same way at
`<broadcast>/portrait`, at the canvas turned and half of it, with its own
encode of the audio scene on program. The sources published independently
are drawn into it, with no [composition](#composition) of their own. Each relay carries both broadcasts,
and GO LIVE's checks time the encoder and size the connection for both.
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

A new canvas size or frame rate, from Settings, an undo, or a reload,
takes effect live. A track never changes what it carries, so the old
renditions' tracks end and new ones are listed in their place in a catalog
update, on the same timeline, and viewers switch to them; the video pauses
while the new encoders open. Items keep their place in canvas pixels. A canvas
the encoder cannot take stops the video, saying why, until the canvas changes
again. Going live with one fails, saying why.

The canvas size encodes whether or not anyone watches, so a viewer who arrives
late can fetch the recent past. The half size encodes only while someone
watches it. On demand copies and encodes the canvas size only while someone
watches it too, which saves that work but leaves nothing from before the first
viewer; audio encodes either way. A recording has an encoder of its own (see
Recording). A hardware encoder that reads
the GPU (NVENC on Linux, VideoToolbox on macOS, Media Foundation on Windows)
takes each frame there when it can read the GPU the studio draws on. When it
cannot, such as NVENC while the studio draws on a laptop's integrated GPU, the
studio passes over it for an encoder that reads each frame back to the CPU
(the opt-in `vaapi` or `openh264` features), which the Health window counts.
Frames the encoder cannot keep up with are dropped rather than delaying the
stream.

### Source tracks

Each camera, display, window, image, media file, color, microphone, and system
audio also has a track of its own, so the studio can replay or record it on its
own. These tracks stay in the studio unless a source is [published independently](#published-independently): otherwise the program and its composition are all it publishes to a relay. They are listed in a local
catalog under the `live.kixel.studio` section's `sources`, by source name, each
as a hang catalog of its own on the program's clock.

A captured source encodes its track as H.264 only while it is used: while it
is shown on program or in a published or recorded scene being drawn, so a
replay of it has its recent past, or while a recording reads its track. Otherwise it holds no encoder. A display or window
that goes into use while its screen stands still, and so delivers nothing new,
restarts its capture, so its track starts with the picture as it is. A capture that
changes size or color space moves to a new track, which its catalog entry
follows. Images and colors keep their picture once, as the image file and as a
one-pixel PNG. Removing or editing a source ends its track. A track carries the
plain capture, before any filter, and a clone has none, since it shows its
source's capture. A removed background stays in the track, since H.264 carries
no transparency, so a replay of the source shows it unless the replay source
removes it too. A browser source or an overlay has no track of its own, and a
broadcast source is already its publisher's.

A microphone, system audio, or media file's sound encodes its track as Opus,
from the same sound the audio scenes hear, only while it is used: while the audio scene on program plays it
(unmuted and held by it), so a replay of it has its recent past, or
while a recording reads its track. The track carries the sound as captured,
before its gain, mute, and delay, which stay the audio scenes'. A buffer the capture
lost, or a stretch the source was out of use, is a gap on its track. Opus
encodes on the CPU, about 1% of one Apple M4 core for a stereo source and
half that for mono, and holds no encode session.

Every encoder holds an encode session, and consumer GPUs allow only a few
hardware sessions at once: 8 on a GeForce card, while a base Apple M4 has one
encode engine for them all. The Health window counts the sessions open:

| Encoder | Sessions |
|---|---|
| The program's canvas size | 1 while on air |
| Its half size | 1 while someone watches it |
| Each published scene's own size and half size | as the program's |
| The portrait picture's, while the show goes out in both | as the program's |
| The recording's own size | 1 while recording |
| Each captured source or media file | 1 while used, as [above](#source-tracks) |

So a live scene showing a camera, a display, and a window holds 4 sessions, 5
with the half size watched, and 6 while recording too. A published portrait
scene drawing the program beside it adds 1 more, 2 with its half size watched,
plus 1 for each captured source it shows that program does not. When the program
encodes on hardware, so do the sources, and a
session the GPU refuses fails that source's track with the encoder's reason on
its layer rather than falling back to software. The source keeps
drawing in the studio's own scenes. The program comes first: when the GPU
refuses the program or a published scene an encoder, such as going live while
the sources on program hold every session, the studio closes the encoders those
sources keep only for a replay of their recent past and opens the program's in
their sessions. Each such source reopens once a session is given back, such as
when the stream ends, its replay starting over; one whose track something reads,
such as a recording, keeps its encoder. A scene published past the cap with no
replay left to close publishes without video and says why in the Health
window. When the program encodes in software, a source takes hardware where it can
and software otherwise.

### Published independently

A broadcast source, such as a guest's camera, or a source with a track of its
own can be **Published independently**, a switch in the inspector's Source
section, off by default and kept in the scene file's `[published.<name>]`
table. Viewers then fetch it as a broadcast of its own and place it over the
program themselves, as the [composition](#composition) says:

- A local source's track goes to the relay as a broadcast of its own at
  `<broadcast>/source/<name>`, with a hang catalog on the program's clock,
  so its name is one segment of that path: no `/`, not `.` or `..`, and a `\`
  only before another `\`.
- A broadcast source is already its publisher's, so viewers read it where the
  studio does, which their relay must let them: a guest's, under the room.
  The studio publishes nothing more of it.

The program leaves a published source out, wherever it or a clone of it is
placed, and so does every audio scene's mix the studio encodes: the
program's audio, recordings, and published scenes. People in the studio
still hear it as viewers do: **Listen** and guests' return feeds mix it in
where its audio scene reaches it, at its gain, mute, pan, and delay. The
studio draws it only as viewers will: over the program, on every page and in
the program's virtual camera, so both show what viewers see. It goes over
every other layer of its scene, and over the always-on-top layers, however
the scene stacks it. Recordings hold the program as published, without it,
and recording **Each source too** gives a published camera, display, window,
or microphone a file of its own. Published and recorded scenes, and a scene
that shows the program, leave it out too. While viewers read a local
source's track, it claims its bitrate of the connection beside the
program's, and its video encodes at what the connection grants it. Its track
carries one rendition, the plain capture before any filter or gain, and
encodes for as long as it is published, holding an encode session as one on
program does.

Once it is published, two more controls say how viewers may treat it, which
the composition carries. They sit under the switch, and in Settings, Advanced
under each published source, which is where a sound alone, with no layer to
inspect, sets them:

- **Viewers may switch it off**, off by default, lets each viewer turn it
  off in their player, which then stops fetching it, such as a facecam or a
  screen share they can do without. The studio encodes it either way.
- **Priority**, 0 by default, ranks its tracks against the program's when a
  viewer's connection cannot carry everything: its picture and sound are
  fetched at hang's priority for each, 60 and 80, plus this, so below 0 it
  degrades before the program and above 0 after. Within 19 either way, its
  sound still goes ahead of the program's picture, and its picture behind
  the program's sound.

Every player must reproduce how it is placed, so a layer showing it takes no
filter but opacity, no blend mode, and no shape, nor do its source's filters or
its keyframes, it does not keep out of the way, and no scene that another scene
draws can place it. A source with
its background removed cannot be published, since H.264 carries no
transparency and its track would show the background.

#### Composition

While on air, beside the program, the studio publishes a composition at
`<broadcast>/composition`, a broadcast of its own whose hang catalog carries
it in the `live.kixel.studio.composition` section, on the program's clock.
Each change is a new catalog, stamped with the program frame it takes effect
on, so a player applies it when it plays that frame, and a viewer who joins
late reads the composition in force. The catalog also lists the program's
video and audio renditions, each with `"broadcast": "."`, so a player that
knows no compositions plays the program from it, as it does at
`<broadcast>`. The section's format is the studio's own until hang specifies
a composition:

```json
{
  "video": {
    "renditions": {
      "video": { "broadcast": ".", "codec": "avc1.640028", "codedWidth": 1920, "codedHeight": 1080 }
    }
  },
  "audio": {
    "renditions": {
      "audio": { "broadcast": ".", "codec": "opus", "sampleRate": 48000, "numberOfChannels": 2 }
    }
  },
  "sync": { "clock": { "wall": 1751846400000000, "timescale": 1000000 }, "interval": 2000 },
  "live.kixel.studio.composition": {
    "version": 2,
    "canvas": { "width": 1920, "height": 1080 },
    "start": 10000000,
    "program": { "broadcast": ".", "instance": "5c1f0e3a9b7d4e21a8f6c0d3b2e19f47" },
    "sources": {
      "camera": { "broadcast": "source/camera", "priority": -10, "toggle": true },
      "mic": { "broadcast": "source/mic", "sound": { "gain": 1.0, "delay": "80ms" } },
      "alice": {
        "broadcast": "../guests/alice/camera.hang",
        "instance": "9e4b7c21d0a3f6e85b1c2d4f7a9e0b36",
        "offset": -1500000,
        "sound": { "gain": 0.8, "pan": -0.5 }
      }
    },
    "pictures": [
      {
        "source": "camera",
        "rect": { "x": 1440.0, "y": 760.0, "width": 400.0, "height": 225.0 },
        "filters": [{ "kind": "opacity", "amount": 0.9 }],
        "keyframes": [{ "at": "1s", "x": 600.0 }]
      },
      { "source": "alice", "rect": { "x": 40.0, "y": 760.0, "width": 400.0, "height": 225.0 } }
    ]
  }
}
```

- `version` is 2. A player refuses any other loudly and plays the program
  alone, as it does a composition in a catalog whose track has no timescale,
  which says nothing of when an update applies. Version 1 was the layer
  layout, a track of the program's broadcast naming its tracks.
- `canvas` is the program's size, which pictures are placed on in pixels.
- `start` is when the scene on program went on, in microseconds on the media
  clock: the moment its pictures' keyframes play from.
- `program` names the program's broadcast, and `sources` each source
  published independently that a picture shows or the program's audio plays,
  by source name. Each `broadcast` is relative to the composition's path, as
  hang's references are, so `.` is the program and `..` walks up a segment.
  An `instance` names the stream it describes, left out when the relay minted
  the one the studio reads.
- A source's `offset`, in microseconds (0 if absent), is added to its
  broadcast's timestamps to put them on the program's clock. A local source
  is stamped on that clock, at 0. A guest keeps a timeline of its own, so its
  offset is where the studio played it: a new composition follows it once it
  moves more than 10 ms, and a guest who joins again, as a new instance, gets
  a new one. A guest the studio shows nothing of, such as one offline, is
  left out with its pictures until it shows again. A still,
  such as another studio's published image, has no moment, so without sound
  its offset is 0.
- A source's `sound`, when the audio scene on program holds it, is the
  `gain`, `muted`, `pan`, and `delay` it plays at.
- A source's `priority` (0 if absent) ranks its tracks against the
  program's: a player subscribes to each of them at hang's priority for its
  kind (60 for video, 80 for audio) plus it, saturating, so under congestion
  one below 0 degrades first. `toggle` (`false` if absent) lets a viewer
  switch the source off, which unsubscribes it and drops its pictures and
  sound.
- `pictures` lists each layer of the scene on program, then of the
  always-on-top ones, showing a published source, back to front over the
  program, shown or hidden. Each is a layer as the scene file writes it: its
  `source` names a key of `sources` (the source a clone shows), and its
  `crop` and `filters` fold in its source's own. While the show goes out in
  both formats, each is placed as the landscape picture places it, hidden
  when shown in portrait alone: the composition goes with the program's
  landscape broadcast, and the portrait one draws the published sources in.

A take changes the composition on the frame the program starts showing the
new scene, so the published sources of the scene leaving go off and those of
the scene coming in go on at once, even through a transition. A published
scene named `composition` would publish where the composition is, so it does
not publish.

#### Watching

`moq-studio --watch https://relay.example/anon --broadcast my-show` plays a
studio's composition in a window as a viewer sees it: the program at
`my-show`, with each picture over it and each sound mixed with it. The URL
names the relay as `--connect` does, and must reach every broadcast the
composition lists, guests' included; `--connect-token` and the `--quic-*`
flags apply to it, and `--connect`, `--failover`, and `--room` are refused
beside it. It is a prototype for settling the composition's format, which
goes once `moq play` plays compositions.

- Each catalog's composition takes effect when the program plays the frame
  the catalog is stamped with, a catalog without one plays the program alone
  from there, as does a catalog that ends or fails at once, and a
  composition's pictures' keyframes play from its `start`.
- Each source plays its own broadcast on the program's clock, its timestamps
  moved by its `offset`: a picture shows when the program's picture stamped
  alike does, and a sound plays with the program's sound stamped alike, at
  its `gain`, `muted`, `pan`, and `delay`. A frame too late for its moment is
  dropped. A new offset plays on without fetching the broadcast again. A
  source's sound is fetched only while the composition gives it a `sound`,
  and gaining or losing one does not fetch its picture again.
- A source whose `instance` another stream replaced shows nothing until a
  composition names the new one, and a composition naming another program
  stream waits for it, so neither plays on a timeline its offset is not for.
- A broadcast without a composition, or with one of another `version`, in an
  untimed catalog, or one it cannot place, such as one listing a broadcast
  above the relay URL's path, plays its program alone; the window says why
  for the latter.
- Each source the composition lets a viewer `toggle` has a switch in the
  window's top-right corner, on until switched off. Off, it is not fetched,
  and neither its pictures nor its sound play; the switch holds across
  compositions, and a source the composition no longer lets a viewer toggle
  plays, whatever its switch said.
- `priority` is not applied yet: every source is fetched at hang's
  priorities, as the program is.
- Sound needs a build with `capture`; without it the window says so in the
  log and plays the pictures alone.

### Recording

The record button in the title bar, on any page, saves the program at the
canvas size, or the scene picked in Settings, Recording at its own size,
to `<scene file name> <UTC start time>.mp4` in the `--recordings` directory,
live or not. The recording has an encoder of its own, opened only while it
records, at full quality whatever the uplink grants the live stream, as OBS's
advanced output mode records apart from streaming. It costs one more encode
session and one more copy of each program frame while recording. With
**Each source too** in Settings, Recording, every camera, display, window, microphone, and system
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
size or frame rate, a recorded scene of another size, or a capture that
changes size or color space, moves the video to a new track, so the recording
or that source goes on in a new file,
numbered after the first, since players show only one video track. A file
starts with up to 2 s from before it opened, so a recording starts a little
early, and a track that goes on into the next file, such as the program
audio, repeats up to 2 s of the last. The
program and local sources are read from the studio's own broadcasts, never what
the relay serves, so a recording starts and stops without touching the live
stream. The audio scene it holds is
encoded a second time for it while recording: the one on program, so the file
holds what viewers hear, or the one Settings, Recording picks, which viewers never
can. Off air, only the
recording's encoder encodes the program.

Each file plays while it grows, so a crash leaves a playable fragmented MP4,
and becomes a regular MP4 when the recording stops or the studio quits.
Quitting first lets the recording's encoders publish what they still hold, and
the program's file keeps it. A file nothing reached before the recording
stopped is removed.

While a file is written, a sidecar `<name>.mp4.unfinished` sits beside it. A
crash, a power loss, or a write that failed leaves the sidecar, and the next
launch repairs that file without asking: it writes `<name>.repaired.mp4`
beside it, numbered if a file already has that name, a regular MP4 of
everything the file holds up to its last complete fragment, leaves the
original untouched, and removes the sidecar, so the file is repaired
only once. The repair runs in the background and appears at the top of the
Health window and in the log, and REVIEW marks its card **Repaired after a crash**. A file another studio is still recording into the
same directory is left alone.

### Source frames

Sources make the opposite trip. A frame already on the GPU is drawn where it
lies: PipeWire camera and VA-API decode DMA-BUFs on Linux, given a Vulkan
driver that imports them, and NV12 capture and VideoToolbox decode surfaces on
macOS. A browser source's page is copied on the GPU from the shared texture CEF
paints, given the same Vulkan DMA-BUF import and a display to paint through:
the Wayland one when `$WAYLAND_DISPLAY` is set and has an output, such as a
monitor, needing no XWayland, or else
the X one in `$DISPLAY`, when the X server runs on the studio's GPU, since
CEF paints on the X server's there. On macOS it is copied from the IOSurface
CEF paints, through Metal. Every other source, every source on Windows or on a
GPU that cannot import, and every still image delivers its frames from the
CPU, which the compositor uploads. A browser source takes that path with no
display at all too, and there renders its page in software, WebGL included, so
a WebGL page runs slower.

## Hotkeys

Settings, Hotkeys binds a key to the **Panic** key, TAKE and AUTO
for the cued scene, turning [Studio mode](#off-air-and-studio-mode) on or off,
**Replay**, **Save clip**, **Drop a marker**, starting or stopping a recording, **Go live**,
**End stream**, each scene, which its key takes, or cues in Studio mode,
as its pad does (the tab says which), and each sound, which its key mutes or
unmutes, as its strip's button does. The [command palette](#command-palette)
lists the same actions with their keys. Press **Bind**, then the key with any
modifiers; Escape, or leaving
the window or the tab, cancels. Every action starts unbound, so no key a game
uses is taken unasked, and a key bound to one action is refused for another
until it is cleared there. Replay and Save clip use REVIEW's picks, but take
the last seconds as long as the stretch picked there, so a key pressed from
another page acts on what just happened, as the clip button beside the panic
key does; a recording uses Settings, Recording.

The key pressed is the key that fires, wherever it sits, on any keyboard layout
and after a layout change: the tab names it as the layout printed it when it
was bound. Keypad keys bind apart from the keys above the letters, so keypad 1
and 1 can do different things; the tab calls them **Keypad 1** and **1**.

Bound keys fire even while another app, such as a game in full screen, has
focus, and the app does not see them, so bind keys you do not type or play
with, such as a function key, a keypad key, or one with modifiers:

- **macOS** and **Windows** register them with the system, which needs no
  permission. A key another app already registered is refused, saying so, as
  is a key the system has no name for. On macOS 15 and later, the system
  refuses a key whose only modifiers are Option, or Option and Shift. Windows
  hears a keypad key only with NumLock on, cannot tell keypad Enter from Enter
  so refuses it, and fires a binding on Enter for both.
- **Linux on X11** grabs them from the X server by where they sit, so any key
  binds, such as ö on a German layout or a key with no name, and fires
  whatever NumLock and CapsLock are.
- **Linux on Wayland**, where no app hears keys meant for another, asks the
  desktop through its GlobalShortcuts portal (KDE Plasma, and GNOME 48 and
  later). The desktop shows its own dialog, may bind other keys than those
  asked, and lists them in its shortcut settings; the tab shows what it
  bound. Without the portal, or if the dialog is declined, bound keys work only
  while the studio has focus, and the tab says so.

The bindings, the snap and grid settings, the
[multiview](#multiview)'s place and picks, and the output **Listen** plays on
are kept per user in the app's settings, `~/Library/Application
Support/moq-studio` on macOS, `$XDG_DATA_HOME/moq-studio` (or
`~/.local/share/moq-studio`) on Linux, and `%APPDATA%\moq-studio\data` on
Windows, apart from the scene file, so a scene file moves between machines
without one operator's keys. A key bound to a scene follows it when renamed in
the studio.

## Effects

The window draws effects of its own, which never reach the program, a
published scene, or a recording:

- The program's tally frame glows blue while the studio is connected to a
  relay, the record dot pulses, and each cut or transition, from a pad, the
  panic key, or a hotkey, bursts on the frame. Studio mode's TAKE is polished
  metal that ripples under the pointer.
- Settings, Hotkeys shows each bound key on a dark plastic keycap, and the
  title bar, the strip, and BUILD's columns have a faint grain.
- The health dot pulses amber while the worst problem is degraded, and
  flickers red with electricity while it is unhealthy.

While the system asks apps to reduce motion, each holds one still pose: Reduce
motion on macOS, animation effects on Windows, and on Linux the desktop's,
through its Settings portal.

## Replay

Add a `replay` source and place it like any other, such as full screen in a
scene of its own. On REVIEW, pick what to replay, the program or a source, by
its card, and a speed of 100%, 50%, or 25%. The strip shows the last 30
seconds of what is picked, a picture each time its track starts a group,
which is each keyframe. Drag the amber handles to pick any stretch of it,
and click or drag elsewhere on the strip to move the scrub head: the large
picture is the one under the head. With the right handle at the strip's right
edge, the stretch is the last that many seconds up to the click; dragged
left of it, the stretch stays on the moments picked as time goes on, until it
ages out of the 30 seconds. **Replay** plays the stretch in the replay source,
which is transparent before and after; with more than one replay source,
pick which one under the speed. Take its scene to put it on program.
Sound replays only at 100%, through the replay source's strip on MIX, so a
replay of a microphone or system audio, which has only sound, plays only at
100%. A new replay cuts off the one playing, and a stretch the source was
offline or paused replays as the gap it was. When Replay or Save clip cannot
act, such as on the program off air or for a source with no past, it says why
in place of what it would do.

**Save clip** writes the same stretch, in real time, into the recordings
folder as a regular MP4: `show-clip-1.mp4` for `show.toml`, then
`show-clip-2.mp4`, and so on. Nothing is re-encoded, so a clip starts at the keyframe before those
seconds, up to one keyframe interval earlier. It shows nothing after them, so
video with B-frames can end early: the pictures shown before the end that refer
to one shown after it are left out, along with everything decoded after that
one. That is usually a fraction of a second, but it can reach back as far as
the keyframe for a slow or deeply reordered source. The studio says where
once it is written, shortly after the click.

Nothing records in the background for this. The program's tracks keep their
last 30 seconds for late viewers anyway, and each source's track while it is on
program (a picture shown in a scene on air, or a sound played by the audio
scene on program) or recorded, and a replay copies from those, so a replay
goes back at most that far. A replay of the program needs it on air, and
reaches back no further than going live; a local source keeps its track off
air. With on demand, the program has
no past from before someone watched it, and a source has none from before it
was last on program or recorded; a replay of nothing is refused, saying why. A
broadcast source replays from what the studio received of it, and only over a
relay connection whose protocol version can address a track by time; an older
one refuses with the reason on REVIEW. An image, a color, or a web page has
no past to replay: a page publishes no track of its own, so replay the program
to show it. A replay source on program encodes each replay as its own track
while it plays, like a camera.

## Markers

Drop a marker on a moment as the show runs, on air or off, with the flag
beside the panic key or a key bound to **Drop a marker** (see
[Hotkeys](#hotkeys)). REVIEW lists this session's markers, the newest first,
each numbered in the order dropped, with how long ago it was. Type a name in
its row, or remove it. Each marker still within the last 30 seconds shows on
the strip as a numbered flag.

The scissors in a marker's row save a clip around it, as **Save clip** does,
of what REVIEW has picked to replay: from 10 seconds before the marker to 5
after, or up to now while those 5 have yet to pass. The same keyframe limits
apply. A clip reaches back only 30 seconds, so a marker older than that has
no clip; its scissors say so.

The markers are kept beside the recordings, not in the scene file:
`show 2026-11-18T12-00-00Z.markers.toml` for `show.toml`, named for when the
studio opened the show, lists each marker's number, the UTC moment it was
dropped, and its name. It is written again on every change and removed once
no marker is left, in the background, so a slow disk never holds up a key. A
marker the folder refuses is kept for the session anyway, and REVIEW says why
until a write succeeds. A session holds at most 255 markers.

## A capture gone dark

A camera, display, or window can go black while its capture still runs: a
window closed, minimized, or switched to exclusive full screen, a screen
locked or showing protected video, a camera behind its shutter, or a
permission the system took away. Four times a second at most, and only when
it delivers a picture (a Linux screen capture repeats its last one while the
screen stands still, so it is read every time), the studio shrinks each
one's picture on the GPU to a grid of 64 by 36 tiles and reads the grid
back. A picture dark in every
tile but a mouse pointer's few, and all one color, is dark.

For its first 3 seconds of dark the studio draws the capture's last picture
that was not dark in its place, on program and everywhere else but where its
background is removed, which shows the dark at once, so a moment
of black barely reaches viewers: the black shows until its picture is next
read and that read comes back from the GPU, up to a quarter second, plus GPU
readback time, as each picture is drawn before it is read, never held back for
it. After that it shows the dark as it is, so a
stale picture never passes for live, and the capture's layer row, its
layer on the picture, and each scene pad showing it turn amber, and the
health dot counts it. The row's badge says what may have happened and offers
the fixes the studio can take: pick a picked window or display again in the
system picker, capture another listed window or the whole screen in its
place, or open a camera or display again. As with **Reload**, picking or
opening it again leaves its last picture on program until it delivers a new
one, even while the picker is open, so cut away first if that picture should
not show. Where only you can fix it, such as
a permission macOS took away or a game that refuses capture, it says what to
do and where. **It is meant to go dark** stops watching it, as
`expect_dark` in its `[video]` table, and **Warn when it goes dark** in the
inspector's Source section watches it again.

A source published independently reaches viewers on its own track, which
shows the dark at once.

### A picture that stalls

A camera or broadcast can also stop sending pictures, so the program repeats
its last one: a camera unplugged or taken by another app, or a broadcast
whose sender stopped or lost its connection. A second without a new picture
counts. Its layer row reads **No new picture for 3 s**, and its layer on the
picture, each scene pad showing it, and the health dot turn amber as for a
capture gone dark. The badge offers to open the camera or subscribe to the
broadcast again, and says what to check where only you can: the camera's
cable and any other app using it, or the broadcast's sender. A window or
display that stands still is not stalled, as nothing new to draw sends no
new picture.

## Health

The Health window, from the health dot's **Details**, grades the stream every second on the thresholds moq's
`hang::health` shares with the rest of the stack, and lists each problem
with its cause in plain words: a relay is unreachable, the uplink is
congested or losing packets, the relay is far away, the encoder is behind or
stuck, a source stalled (a camera or broadcast the program or a published
scene draws, or a microphone or system audio that the audio scene on program
or one a published scene carries plays: unmuted and held by it), a source
one of them shows or plays failed or, as a media file that does not loop,
played to its end, a camera, display, or window one of them shows
[went dark](#a-capture-gone-dark), a browser source one of them shows is uploaded from memory
rather than copied on the GPU, or viewers are late, stalling, or cannot decode
a track. A relay lost while the other serves is
degraded, since viewers have nothing left to fall back to; with none serving
it is unhealthy. A problem with a published scene's broadcast, or a source
only it shows or plays, names the scene.
Below that it shows:

- **Relays**: each relay's state, and the protocol its session negotiated.
- **Uplink**: the round trip to the first connected relay, the bandwidth the
  connection estimates it can carry, the media every broadcast's tracks write, and
  the share lost and resent. Loss and headroom are graded only while someone
  watches, since the relay pulls nothing otherwise. Source tracks never reach
  the relay, so they are left out of the media.

Then, for the program's broadcast, the portrait picture's, and each published
scene's, each with its own viewers:

- **Watching**: the sessions subscribed to it, from the relays'
  `.stats` broadcasts. A relay shares those only with a connection allowed to
  read them, so most show "unknown". In a cluster, a relay forwarding the
  stream to another counts as one session. Two relays whose stats use the
  same node name publish them at the same path, so only one of them counts:
  the first relay's while it has them.
- **Tracks**: each track's share of the connection's bandwidth (**Granted**,
  or "off" for a rendition the share cannot fund), its bitrate and frame rate,
  and, from viewers that report, the lowest bitrate any received, its worst
  grade, and the latency from the program clock to the slowest viewer. A
  published scene lists the program's audio, graded where the program writes
  it. Latency compares the two hosts' wall
  clocks, so keep both synced.
- **Viewers**: each viewer that reports, with its round trip and latency.
  The studio reads 64 at most per broadcast, a sample the grades
  rest on: it counts the rest, and a reader one frees goes to the next viewer
  to arrive.

Last, **Sources** lists each source's own tracks, grouped by source; a still
image or color carries no media to grade.

Each catalog asks viewers to report beneath its broadcast, under
`<broadcast>/viewers/` or `<broadcast>/<scene>/viewers/`: a viewer publishes
`<broadcast>/viewers/<name>.echo`, as `moq export --echo <name>` does. The relay decides who may publish there, as for any broadcast.

## Virtual camera

The **Virtual camera** toggle in Settings, Advanced, shows the program as a
camera that video-call and conferencing apps can pick, at the canvas size and
frame rate, with each [published source](#published-independently) drawn
over it as viewers see it. Each published scene has a toggle of its own beneath it,
showing that scene at its own size as a camera of its own, so an app
can pick the program or any published scene. A camera stays on until you
turn it off or quit; it is not saved in the scene file. A scene's camera
turns off when you stop publishing or rename the scene. A picture is copied
for a camera only while an app reads it, and a scene's camera has the scene
drawn only then. Changing the size or frame rate of what a camera
shows reopens it at the new one, so an app reading it opens it again. BUILD's
Add leaves the studio's cameras out while they are on, so one
cannot be added as a source of the picture it shows.

- **Linux:** a PipeWire camera named "kixel.live Studio" for the program, and
  "kixel.live Studio: <scene>" for a scene (needs the `pipewire` feature), which
  browsers and other PipeWire-aware apps list beside your other cameras.
  Without PipeWire, each camera writes to the first
  [v4l2loopback](https://github.com/v4l2loopback/v4l2loopback) device that
  takes its size and that no other camera of the studio holds, for apps that
  open `/dev/video*` directly. The studio never loads that module, since it
  needs root. Load a device for each camera you turn on, such as two:
  `sudo modprobe v4l2loopback devices=2 exclusive_caps=1,1 card_label="kixel.live Studio","kixel.live Studio 2"`.
- **macOS:** a camera extension the app carries. The first time you turn a
  camera on, macOS asks you to allow it in System Settings > General > Login
  Items & Extensions > Camera Extensions, which needs the app in
  Applications; then turn the camera off and on again. From then on apps list
  "kixel.live Studio", the program's camera, even while the studio is closed,
  when it shows nothing, and "kixel.live Studio: <scene>" while a scene's
  camera is on. A newer app replaces the extension, without a restart, the
  first time you turn a camera on after updating. Only the app feeds the
  extension's cameras, so the studio run from a build outside the app has no
  virtual camera.
- **Windows:** not available yet.

## Development

Use the Nix dev shell (`direnv allow`, or `nix develop --command ...`) so
tooling matches CI, then:

```bash
just check   # Lint everything, including the quest tree
just test    # Run the tests
just ci      # Both, as CI runs them
just fix     # Auto-fix formatting
just join    # Serve the join page locally
just shots   # Render views of the window to PNG in target/shots
```

`just shots studio audio` renders only the views named, each a test in
`rs/moq-studio/src/shots.rs` that draws one state of the window, the studio
views on `rs/moq-studio/shots/show.toml` and `first_launch` on a new show. They draw on Mesa's software Vulkan driver, so need no display or GPU, and
run on Linux only, where that driver builds. `SHOTS_DIR` picks another output
directory; a full run clears old PNGs only from the default one.

`rs/moq-scene` is the UI-free compositor, `rs/moq-studio` the app,
`rs/moq-studio-camera` the macOS virtual camera's extension, and
`js/join` the join page, built on `@moq/room`, `@moq/publish`, and
`@moq/watch`. Like the
Rust crates, those come from the `moq` submodule rather than npm, so
`bun install` needs it checked out, and Nix to build `@moq/libav`. See
[AGENTS.md](AGENTS.md) and [CONTRIBUTING.md](CONTRIBUTING.md), and the
[quest tree](quest/README.md) for planned work.

## License

Licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

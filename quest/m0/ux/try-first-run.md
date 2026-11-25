# [XS] Try the first launch

## Goal

The maintainer tries the first launch and the Show menu on their own
machines and either signs them off or files what they find as quests: what
the epic's [What a person must check](/quest/m0/ux/README.md) asks of them,
which no headless run exercises.

## Plan

The zero-gate first launch is on `main`
(`cargo run -p moq-studio --features capture,openh264`, adding `pipewire` on a
Wayland desktop for its screen picker). What to try, beside
the [mock-up](/quest/m0/ux/mock/overhaul-first.webp):

- **Cold launch:** move the app's storage aside (`~/.local/share/moq-studio`
  on Linux, `~/Library/Application Support/moq-studio` on macOS), launch with
  no arguments on a machine with a real camera and microphone, and count the
  clicks to a live camera and screen. The quick start's Camera card adds the
  camera in one, and the quick start then gives way to the scene, so the
  screen comes from **Add** (A, or the Layers rail's Add tab), under the
  displays; the first-run
  quest hoped for two clicks in all. Note how long the window shows "Opening
  My Show…" and whether the canvas came out at 60 fps on a hardware encoder.
- **Permissions:** the camera and microphone prompts macOS raises on that
  launch (run `tccutil reset Camera live.kixel.studio` and `tccutil reset
  Microphone live.kixel.studio` first if you answered them before), the
  Screen Recording prompt the Screen card raises on macOS (`tccutil reset
  ScreenCapture live.kixel.studio`), and the desktop's screen picker on
  Wayland from the Screen card.
- **The microphone:** whether **Mic** in the sound moves with your voice
  without a click, and whether hearing it by default is right.
- **Shows:** relaunching reopens the show; New show, Open show file…,
  Rename show…, and Export show copy… from the title bar's menu, and whether
  a recording lands in your Videos folder.

An agent then turns each finding into a quest, or an edit to the quest that
owns that part, and deletes this one.

## Waiting

- Maintainer: try the first launch as the Plan lists and report what to
  change, or that nothing needs to. An agent cannot judge feel or taste, and
  the agent hosts have no camera, microphone, or permission prompts; a kittest
  run already covers the clicks, and `just shots` the layout. Check: a comment
  or quest records the result.

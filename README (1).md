# Unacademy Video Merge

Download an Unacademy class as a **single MP4 with the whiteboard rendered in**.

A class replay is not one video. `output.webm` is only the educator's webcam
and audio; every slide and pen stroke lives in `data.json` and is drawn by the
browser onto a canvas. Downloading "the video" gets you a talking head against
no background — the actual lesson is missing. This tool reconstructs the board
from the event stream, composites the webcam into a corner, muxes the original
audio back in, and writes one file.

```
webcam webm  ──┐
board events ──┼──> render + composite ──> <Class title> - <Educator>.mp4
slide images ──┘
```

## What a class actually is

| Piece | Where | What it is |
| --- | --- | --- |
| `output.webm` | `uamedia.uacdn.net/lesson-raw/<media_uid>/` | The educator's webcam: 640×360, ~16 fps, VP8 + Opus |
| `data.json` | `player.uacdn.net/lesson-raw/<media_uid>/` | Every slide change and pen stroke, timestamped in microseconds |
| Slide images | `uadoc.uacdn.net/...pdf?page=N` | The deck, one PDF page per request |

**Only the first is a video file.** Two API calls tie the rest together, both
authenticated with a bearer JWT — the same value as the `accessToken` cookie:

```
GET  api.unacademy.com/v1/uplus/classes/<class_uid>/details/
     -> video_url=...?uid=<media_uid>   the CDN folder; not the class uid
     -> slides_pdf.with_annotation

GET  unacademy.com/api/v1/uplus/classes/<class_uid>/replay_token/?include_meta_json=1
     -> meta_json.range   byte spans into data.json, ~180s per chunk
     -> domain_config     which CDN hosts to use
```

`meta_json.range` maps *the last event timestamp in a chunk* to that chunk's
byte span in the uncompressed `data.json`. Each span is a standalone JSON
array, which is how the browser player seeks without pulling all 7 MB.

## Install

### From a release

Every tagged release ships a prebuilt tarball for each Linux architecture.
Match yours with `uname -m`:

| `uname -m` | Target | Good for |
| --- | --- | --- |
| `x86_64` | `x86_64-unknown-linux-gnu` | ordinary 64-bit PCs, most servers, WSL |
| `x86_64` | `x86_64-unknown-linux-musl` | Alpine, old distros — fully static |
| `aarch64` / `arm64` | `aarch64-unknown-linux-gnu` | Pi 4/5 64-bit, Graviton, ARM VMs |
| `aarch64` / `arm64` | `aarch64-unknown-linux-musl` | Alpine on ARM — fully static |
| `armv7l` | `armv7-unknown-linux-gnueabihf` | Pi 2/3 32-bit, most ARM boards |
| `armv7l` | `armv7-unknown-linux-musleabihf` | Alpine on ARMv7 — fully static |
| `armv6l` | `arm-unknown-linux-gnueabihf` | Pi Zero / Pi 1 |
| `i686` / `i386` | `i686-unknown-linux-gnu` | 32-bit x86 |
| `i686` / `i386` | `i686-unknown-linux-musl` | 32-bit x86, static |
| `riscv64` | `riscv64gc-unknown-linux-gnu` | best-effort † |
| `ppc64le` | `powerpc64le-unknown-linux-gnu` | best-effort † |
| `s390x` | `s390x-unknown-linux-gnu` | best-effort † |
| `loongarch64` | `loongarch64-unknown-linux-gnu` | best-effort † |

**gnu or musl?** The `gnu` builds link the system glibc and are the normal
choice. The `musl` builds are statically linked — take those on Alpine, on a
distro too old for the glibc the release was built against, or whenever a
`gnu` binary complains about `GLIBC_2.xx not found`.

† Those four are built on a best-effort basis: the release publishes whatever
survived the toolchain, so if one is missing from the assets, build it from
source.

```bash
V=0.1.0
T=x86_64-unknown-linux-gnu          # from the table above
BASE=https://github.com/<owner>/unacademy-video-merge/releases/download/v$V

curl -LO "$BASE/uadl-$V-$T.tar.gz"
curl -LO "$BASE/SHA256SUMS"
sha256sum -c --ignore-missing SHA256SUMS

tar xzf "uadl-$V-$T.tar.gz"
cd "uadl-$V-$T"
./uadl --help
```

Each tarball holds the `uadl` binary, `run.sh` and this README. Drop the
binary somewhere on `PATH` if you want it available everywhere:

```bash
sudo install -m755 uadl /usr/local/bin/uadl
```

### Build from source

Needs a recent stable Rust toolchain ([rustup](https://rustup.rs)) and
`ffmpeg`.

```bash
git clone https://github.com/<owner>/unacademy-video-merge
cd unacademy-video-merge

cargo build --release          # -> target/release/uadl
./target/release/uadl --help
```

Or install it onto your `PATH` in one step:

```bash
cargo install --path .         # -> ~/.cargo/bin/uadl
```

While working on the code:

```bash
cargo check                    # fast type-check, no codegen
cargo clippy --all-targets     # lints
cargo build                    # debug build -> target/debug/uadl
```

Use `--release` for any real class. The renderer is compute-bound and an
unoptimised build is roughly an order of magnitude slower; the release profile
also turns on LTO, a single codegen unit and symbol stripping.

### Cross-compiling every architecture

`build-all.sh` produces the same tarballs the release workflow does, into
`dist/`. It drives [`cross`](https://github.com/cross-rs/cross), which keeps
each target's linker and sysroot in a container, so nothing has to be
installed per architecture beyond Docker:

```bash
cargo install cross --locked
./build-all.sh                 # every target
./build-all.sh aarch64         # only targets matching "aarch64"
```

One target without Docker, using a distro cross-compiler:

```bash
sudo apt install gcc-aarch64-linux-gnu
rustup target add aarch64-unknown-linux-gnu

CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
cargo build --release --target aarch64-unknown-linux-gnu
```

### Cutting a release

`.github/workflows/release.yml` builds the whole matrix and publishes the
tarballs plus a `SHA256SUMS` file:

```bash
# bump `version` in Cargo.toml first
git tag v0.1.0
git push origin v0.1.0
```

Tier-1 targets fail the run if they break; the best-effort ones are allowed to
drop out so one stubborn architecture cannot hold up a release. The workflow
also runs from the Actions tab (`workflow_dispatch`), which builds everything
and leaves the tarballs as artifacts without tagging anything.

### ffmpeg

`ffmpeg` is a runtime dependency — it is not bundled and static linking does
not change that. Install it with your package manager (`apt install ffmpeg`,
`apk add ffmpeg`, `dnf install ffmpeg`, `brew install ffmpeg`), or drop a
[static build](https://johnvansickle.com/ffmpeg/) next to `uadl`.

It is located in this order: `$UADL_FFMPEG`, then a copy sitting beside the
binary (so a portable directory is self-contained), then `PATH`. The check
runs *before* any network work, so a missing ffmpeg fails in a second rather
than after a 400 MB download.

## Use

```bash
./run.sh "https://unacademy.com/class/<slug>/<UID>"
```

`run.sh` builds the binary if needed, reads the token from `token.txt` beside
it (or `$UNACADEMY_TOKEN`), and drops the MP4 in the same directory. To get the
token: unacademy.com → F12 → Application → Cookies → `accessToken`. It is a
bearer JWT valid for about an hour.

Or call the binary directly:

```bash
uadl <URL-or-UID> <TOKEN> -o <output-dir> [flags]
uadl ABCD1234 "$UNACADEMY_TOKEN" -o ~/classes --fast
```

### Flags

| Flag | Default | Meaning |
| --- | --- | --- |
| `-o, --out DIR` | `.` | Output directory |
| `--fast` | | 960px, 8 fps, crf 30 — fewer pixels and frames |
| `--best` | | crf 23, `veryfast` — sharper, slower |
| `--width N` | `1280` | Board width; height follows at 16:9 |
| `--fps N` | `12` | Output frame rate |
| `--crf N` | `28` | x264 quality, lower is better |
| `--preset S` | `superfast` | x264 preset |
| `--no-pip` | | Drop the webcam inset |
| `--no-zoom` | | Ignore the educator's zoom/pan, always show the whole page |
| `-j, --jobs N` | cores | Render threads |
| `-c, --connections N` | `16` | Download connections |
| `--start`, `--end` | | Seconds — render a slice before committing to the whole class |
| `--keep` | | Keep the `.webm` and `.events.json`; reruns then skip the download |
| `-q, --quiet` | | No progress output |

A complete `.webm` from an earlier run is reused, so an interrupted or repeated
run does not re-download the media.

## How it works

### The event vocabulary

Decoded from ~45k events across two captured classes:

| Plugin | Event | Meaning |
| --- | --- | --- |
| `cw` | `d` `m` `u` | pen down / move / up, points normalised to `[0,1]` |
| `cw` | `p` | **pointer** — the cursor moving with no ink |
| `cw` | `ea` | erase everything on that canvas |
| `cw` | `zm` `pn` | zoom factor / pan offset |
| `dcn` | `as` `sc` | add slide `{i, u, bc}` / show slide `{s}` |
| `dcn` | `cc` `mc` | pen colour / mode (`marker`, `highlighter`, `eraser`) |
| `dcn` | `pstc` `estc` | pen and eraser thickness |
| `pl` | `opl` `cpl` | poll opened / closed with results |
| `ch` | `mp` | message pinned |
| `mcn` | `cle` | class ended |

Canvas ids line up 1:1 with slide indices. Rendering order matches the browser:
**slide → highlighter → ink → pointer**, with highlighter under ink so writing
over a highlight stays readable, and eraser strokes punching alpha out of both
layers.

### Zoom and pan

Stroke coordinates are always page-space; zoom/pan is pure presentation. The
player applies one transform to the slide, ink and pointer layers alike, with a
50%/50% origin and the translation clamped so the viewport can never leave the
page:

```
s  = zoom
tx = clamp(pan.x * s, -(s-1)/2, +(s-1)/2)
screen_x = 0.5 + s * (page_x - 0.5) + tx
```

That clamp is what keeps a zoomed board from showing blank margin. Verified
against a capture: at t=2520s on a 2.5× zoomed canvas the transform puts the
viewport at x 0.311–0.711, y 0.045–0.445, and every stroke drawn at that moment
falls inside it. `--no-zoom` skips the transform entirely.

### The pipeline

Board frames are rendered in Rust and pushed to ffmpeg over a pipe as raw
**yuv420p**; ffmpeg decodes the webm, scales the webcam, overlays it
bottom-right at 22%, takes the audio, and encodes with x264 — all in C on its
own threads while the next frames are being drawn.

Two things carry most of the speed:

- **Colour conversion is ours, not ffmpeg's.** Handing ffmpeg `rgb24` was the
  single biggest cost in the pipeline, bigger than encoding: a full-frame
  conversion per frame on twice the pipe bytes. At 1280×720 over 1200 frames —
  `rgb24 → null` 123.8 fps (the ceiling), `rgb24 → x264 superfast` 116.1 fps,
  `yuv420p → x264 superfast` **288.6 fps**. Coefficients are BT.601 limited
  range, so colours match what swscale produced.
- **Most frames are a memcpy.** The board only changes when an event changes
  it, so a fully composited YUV frame is cached and a typical frame costs one
  copy plus a repaint of the small square the cursor sits in. Ink blending is
  bounded to the strokes' bounding box.

Downloading is split across 16 connections by default: the CDN serves byte
ranges and throttles per connection, not per client — one stream tops out near
0.3 MB/s while sixteen reach 16 MB/s on the same file. Ranges are written
straight into their slots in a preallocated file.

Rendering is also segmented across threads. Each segment is independent given
the replay state at its start, and ffmpeg joins them with the concat demuxer —
stream copy, no re-encode. Encoding is ~90% of the work and x264 already uses
every core, so this buys less than it looks like it should, but it is free.

`ultrafast` is a trap here: it encodes faster but emits a much larger
bitstream, and writing that costs more than the encoder saves — measured slower
end to end than `superfast`. `--fast` cuts pixels and frames instead, which
reduces work everywhere.

## Layout

```
Cargo.toml
run.sh        convenience launcher: build, read token, render
src/
  main.rs     CLI, speed profiles, orchestration, progress
  api.rs      the class-locating endpoints, auth, byte-range seeking
  model.rs    wire format -> typed events; meta_json chunk index
  state.rs    the replay state machine (slides, strokes, pens, zoom, pointer)
  render.rs   state -> frame (layers, zoom/pan, pointer, board cache)
  yuv.rs      RGBA -> YUV420p, BT.601 limited range
  assets.rs   slide image fetch + on-disk cache
  download.rs multi-connection ranged HTTP download
  export.rs   segmented render, ffmpeg pipe, overlay, mux, concat
```

## Notes and limits

- `output.mp4` is 403 whenever `is_mp4_disabled_globally` is set — which is
  most classes. The webm is the real format.
- Stroke widths (`MARKER_PX`, `HIGHLIGHT_PX`, `ERASER_PX` in `render.rs`) are
  calibrated by eye against the browser player. Adjust to taste.
- Drop `--fps` to 8 for talk-heavy classes; the board barely moves.
- Chat replay (`ws-cdn.unacademy.com/replay/qna`) is parsed into state but not
  drawn; the captures had empty QnA to calibrate against.
- Rendering straight to a Windows mount (`/mnt/c`) is ~1.8× slower than local
  disk because the muxer writes incrementally. Output is staged locally and
  moved at the end, so this mostly does not bite.
- PDF export is not in this build.
- This only reaches classes your own account can already open. Nothing here
  bypasses access control — it re-implements the client, it doesn't defeat it.
  Redistributing what you download is a different question, and it's on you to
  keep it on the right side of Unacademy's terms.

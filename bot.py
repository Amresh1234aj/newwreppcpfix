import asyncio
import os
import re
import shutil
from pathlib import Path

from dotenv import load_dotenv
from pyrogram import Client, filters
from pyrogram.types import Message

load_dotenv()

API_ID = int(os.environ["API_ID"])
API_HASH = os.environ["API_HASH"]
BOT_TOKEN = os.environ["BOT_TOKEN"]
TARGET_CHAT_ID = int(os.environ["TARGET_CHAT_ID"])
UNACADEMY_TOKEN = os.environ.get("UNACADEMY_TOKEN", "").strip()

ROOT = Path(__file__).resolve().parent
UADL = ROOT / "UnacademyDownloader-main" / "target" / "release" / "uadl"
DOWNLOADS = ROOT / "downloads"
DOWNLOADS.mkdir(exist_ok=True)

app = Client(
    "board_face_uploader",
    api_id=API_ID,
    api_hash=API_HASH,
    bot_token=BOT_TOKEN,
)

def valid_class_url(s: str) -> bool:
    return bool(re.match(r"^https?://(?:www\.)?unacademy\.com/class/[^/\s]+/[^/\s]+/?$", s.strip()))

@app.on_message(filters.command("start"))
async def start(_, m: Message):
    await m.reply_text(
        "Send /download <authorized Unacademy class URL>.\n"
        "The renderer will create one MP4 containing board/slides + face overlay + audio."
    )

@app.on_message(filters.command("download"))
async def download_cmd(_, m: Message):
    if not UNACADEMY_TOKEN:
        await m.reply_text("UNACADEMY_TOKEN is not configured on the server.")
        return

    parts = m.text.split(maxsplit=1)
    if len(parts) != 2 or not valid_class_url(parts[1]):
        await m.reply_text("Usage:\n/download https://unacademy.com/class/<slug>/<UID>")
        return

    url = parts[1].strip()
    status = await m.reply_text("⏳ Downloading and rendering board + face...")

    # Clean only our temporary output directory.
    work = DOWNLOADS / str(m.from_user.id)
    if work.exists():
        shutil.rmtree(work)
    work.mkdir(parents=True)

    proc = await asyncio.create_subprocess_exec(
        str(UADL), url, UNACADEMY_TOKEN,
        "-o", str(work),
        "--best",
        "--pip",
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.STDOUT,
    )

    last = ""
    while True:
        line = await proc.stdout.readline()
        if not line:
            break
        text = line.decode(errors="ignore").strip()
        if text:
            last = text
            # Keep Telegram updates sparse.
            if any(k in text.lower() for k in ("render", "download", "error", "%")):
                try:
                    await status.edit_text(f"⏳ {text[-3500:]}")
                except Exception:
                    pass

    rc = await proc.wait()
    if rc != 0:
        await status.edit_text(
            "❌ Rendering failed.\n\n"
            f"{last[-3000:] if last else 'No diagnostic output was returned.'}"
        )
        shutil.rmtree(work, ignore_errors=True)
        return

    videos = sorted(work.glob("*.mp4"), key=lambda p: p.stat().st_mtime, reverse=True)
    if not videos:
        await status.edit_text("❌ Renderer finished but no MP4 was produced.")
        shutil.rmtree(work, ignore_errors=True)
        return

    video = videos[0]
    await status.edit_text("📤 Board + face MP4 तैयार है. Telegram पर upload कर रहा हूँ...")

    try:
        await app.send_video(
            TARGET_CHAT_ID,
            str(video),
            caption=f"🎓 {video.stem}",
            supports_streaming=True,
        )
        await status.edit_text("✅ Board + face video uploaded successfully.")
    except Exception as e:
        await status.edit_text(f"❌ Telegram upload failed:\n{e}")
    finally:
        shutil.rmtree(work, ignore_errors=True)

if __name__ == "__main__":
    app.run()

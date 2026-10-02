# Board + Face Telegram Uploader

[![Deploy to Heroku](https://www.herokucdn.com/deploy/button.svg)](https://www.heroku.com/deploy?template=https://github.com/YOUR_GITHUB_USERNAME/YOUR_REPO_NAME)

> Button काम करे इसके लिए यह folder अपने GitHub repo में push करो और ऊपर के link में
> `YOUR_GITHUB_USERNAME/YOUR_REPO_NAME` बदल दो। Deploy के बाद Heroku में **worker** dyno ON रखना
> (web dyno नहीं)।

This package combines the **Unacademy board renderer** from `UnacademyDownloader-main`
with a Telegram uploader using the same Pyrogram-style upload flow as the supplied
uploader project.

### Output
The renderer is configured with:
- board/slides + pen strokes
- educator webcam as a small face overlay (PIP)
- original audio
- MP4 output

### Important
Use this only for classes you own or are explicitly authorized to download/use.
Keep `UNACADEMY_TOKEN` private; it is a login/session credential.

## Docker deployment

1. Copy `.env.example` to `.env` and fill the secrets.
2. Build:
   `docker build -t board-face-uploader .`
3. Run:
   `docker run --env-file .env board-face-uploader`

For Render/Heroku-style Docker deployment, use this repository/folder as the
Docker build context and configure the same environment variables in the
platform's secret/environment settings.

## Telegram command

`/download https://unacademy.com/class/<slug>/<UID>`

The bot:
1. calls the Rust renderer;
2. renders board + face + audio into one MP4;
3. uploads that MP4 to `TARGET_CHAT_ID`;
4. removes its temporary local files.

The bot does not receive the Unacademy token through Telegram; it reads it from
the server environment.

## Local run

Install ffmpeg and Rust, then:

`cargo build --release --manifest-path UnacademyDownloader-main/Cargo.toml`

Install Python dependencies:

`pip install -r requirements.txt`

Set the environment variables and run:

`python bot.py`

If the board still does not appear for a particular class, that replay may have
a different/unsupported event format; the renderer needs the class's slide/event
data to reconstruct the board.

## Heroku deploy

`app.json` + `heroku.yml` included हैं (container stack, एक `worker` dyno). Button दबाओ,
5 env variables भरो (`API_ID`, `API_HASH`, `BOT_TOKEN`, `TARGET_CHAT_ID`, `UNACADEMY_TOKEN`)
और deploy करो। Rust build में कुछ मिनट लगते हैं।

CLI से:

    heroku create
    heroku stack:set container
    git push heroku main
    heroku ps:scale worker=1

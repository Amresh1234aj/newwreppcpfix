FROM rust:1-bookworm AS builder
RUN apt-get update && apt-get install -y --no-install-recommends ffmpeg pkg-config libssl-dev ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY UnacademyDownloader-main/Cargo.toml UnacademyDownloader-main/Cargo.lock ./UnacademyDownloader-main/
COPY UnacademyDownloader-main/src ./UnacademyDownloader-main/src
RUN cargo build --release --manifest-path UnacademyDownloader-main/Cargo.toml

FROM python:3.12-slim-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends ffmpeg ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt
COPY bot.py .
COPY --from=builder /src/UnacademyDownloader-main/target/release/uadl /app/UnacademyDownloader-main/target/release/uadl
RUN chmod +x /app/UnacademyDownloader-main/target/release/uadl && mkdir -p /app/downloads
CMD ["python", "bot.py"]

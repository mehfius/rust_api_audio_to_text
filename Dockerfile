# Base image for whisper.cpp
FROM docker.io/mehfius/whisper-ngrok:latest

WORKDIR /app

ENV NGROK_AUTHTOKEN="2xeZlinzvgCmBrqkuos53mB2YU2_5Qn9GiGTF61A1U4h1m6mw"
ENV NGROK_URL="reindeer-evident-primarily.ngrok-free.app"

COPY target/release/rust_api_audio_to_text ./rust_api_audio_to_text
COPY entrypoint.sh ./entrypoint.sh

RUN chmod +x ./rust_api_audio_to_text
RUN chmod +x ./entrypoint.sh

EXPOSE 6000

ENTRYPOINT ["./entrypoint.sh"]
CMD []

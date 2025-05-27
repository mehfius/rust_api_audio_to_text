# Base image for whisper.cpp
FROM docker.io/mehfius/whisper-ngrok:latest

WORKDIR /app


COPY target/release/rust_api_audio_to_text ./rust_api_audio_to_text
COPY entrypoint.sh ./entrypoint.sh

RUN chmod +x ./rust_api_audio_to_text
RUN chmod +x ./entrypoint.sh

EXPOSE 6000

ENTRYPOINT ["./entrypoint.sh"]
CMD []

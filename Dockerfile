FROM ghcr.io/ggerganov/whisper.cpp:main

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

RUN mkdir -p models
RUN curl -L https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin -o models/ggml-base.bin && \
    curl -L https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin -o models/ggml-large-v3.bin && \
    curl -L https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin -o models/ggml-large-v3-turbo.bin && \
    curl -L https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q8_0.bin -o models/ggml-large-v3-turbo-q8_0.bin

COPY target/release/rust_api_audio_to_text ./rust_api_audio_to_text

RUN chmod +x ./rust_api_audio_to_text

EXPOSE 6000

CMD ["./rust_api_audio_to_text"]

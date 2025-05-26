# Audio to Text API

A Rust-based API service that transcribes audio files to text using Whisper.cpp.

## Features

- REST API endpoint for audio transcription
- Supports WAV audio files (mono, 16-bit, 16 kHz)
- Returns transcription with timestamps
- Docker containerization
- Portuguese language support

## Dependencies

### Rust Dependencies
- actix-web (4.8)
- actix-multipart (0.6)
- futures-util (0.3)
- serde_json (1.0)
- tokio (1.38)
- hound (3.5)
- log (0.4)
- env_logger (0.11)
- serde (1.0)

### System Dependencies
- Whisper.cpp (base model)
- Docker

## API Endpoint

- POST /transcribe
  - Accepts multipart/form-data with WAV audio file
  - Returns JSON with transcription segments including timestamps

## Requirements

- WAV audio files must be:
  - Mono channel
  - 16-bit depth
  - 16 kHz sample rate

## Docker

The service is containerized using Docker and includes:
- Whisper.cpp base model
- Rust API service
- Exposed port: 6000 
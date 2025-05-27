use actix_multipart::Multipart;
use actix_web::{post, App, HttpResponse, HttpServer, Responder, Error};
use futures_util::TryStreamExt as _;
use tokio::process::Command;
use tokio::io::AsyncWriteExt;
use serde_json::json;
use serde::Serialize;
use std::path::Path;
use std::process::Stdio;
use log::{error, info, LevelFilter};
use env_logger::Env;
use hound::WavReader;
use std::os::unix::fs::PermissionsExt;
use actix_cors::Cors; // Import Cors

#[derive(Serialize)]
struct TranscriptionSegment {
    start: String,
    end: String,
    text: String,
}

fn format_bytes(bytes: usize) -> String {
    const KB: usize = 1024;
    const MB: usize = 1024 * KB;
    const GB: usize = 1024 * MB;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}

#[post("/transcribe")]
async fn transcribe_audio(
    mut payload: Multipart,
) -> Result<impl Responder, Error> {
    info!("Iniciando processamento da requisição de transcrição.");

    let mut wav_buffer = Vec::new();
    let mut model_filename: Option<String> = None;

    while let Some(mut field) = payload.try_next().await? {
        let field_name = field.name().to_string();
        match field_name.as_str() {
            "file" => {
                while let Some(chunk) = field.try_next().await? {
                    wav_buffer.extend_from_slice(&chunk);
                }
            },
            "model" => {
                let mut model_data = Vec::new();
                while let Some(chunk) = field.try_next().await? {
                    model_data.extend_from_slice(&chunk);
                }
                model_filename = Some(String::from_utf8_lossy(&model_data).to_string());
            },
            _ => {
            }
        }
    }

    if wav_buffer.is_empty() {
        error!("No WAV file provided in the request");
        return Ok(HttpResponse::BadRequest().json(json!({
            "error": "No WAV file provided"
        })));
    }

    info!("Arquivo WAV recebido com tamanho: {}", format_bytes(wav_buffer.len()));

    let final_model_filename = model_filename.unwrap_or_else(|| "ggml-base.bin".to_string());
    let model_path = format!("./models/{}", final_model_filename);

    if !Path::new(&model_path).exists() {
        error!("Model file {} not found in ./models", final_model_filename);
        return Ok(HttpResponse::InternalServerError().json(json!({
            "error": format!("Model file {} not not found in ./models", final_model_filename)
        })));
    }

    let binary_path = "/app/build/bin/whisper-cli"; 

    if !Path::new(&binary_path).exists() {
        error!("Whisper-cli binary not found at {}", binary_path);
        return Ok(HttpResponse::InternalServerError().json(json!({
            "error": format!("Whisper-cli binary not found at {}", binary_path)
        })));
    }
    if !Path::new(&binary_path).metadata().map(|m| m.permissions().mode() & 0o100 != 0).unwrap_or(false) {
        error!("Whisper-cli binary at {} is not executable", binary_path);
        return Ok(HttpResponse::InternalServerError().json(json!({
            "error": format!("Whisper-cli binary at {} is not executable", binary_path)
        })));
    }

    let cursor = std::io::Cursor::new(&wav_buffer);
    let wav = WavReader::new(cursor).map_err(|e| {
        error!("Invalid WAV format: {}", e);
        actix_web::error::ErrorBadRequest(format!("Invalid WAV format: {}", e))
    })?;
    let spec = wav.spec();
    if spec.channels != 1 || spec.sample_rate != 16000 || spec.bits_per_sample != 16 {
        error!("WAV must be mono, 16-bit, 16 kHz, got channels: {}, sample_rate: {}, bits_per_sample: {}", 
            spec.channels, spec.sample_rate, spec.bits_per_sample);
        return Ok(HttpResponse::BadRequest().json(json!({
            "error": "WAV must be mono, 16-bit, 16 kHz"
        })));
    }

    let mut whisper_cmd = Command::new(&binary_path)
        .args([
            "-m", &model_path,
            "-f", "-",
            "-l", "pt",
            "-ovtt",
            "-of", "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            error!("Failed to spawn whisper-cli command: {}", e);
            actix_web::error::ErrorInternalServerError(format!("Failed to spawn whisper-cli command: {}", e))
        })?;

    if let Some(mut stdin) = whisper_cmd.stdin.take() {
        match stdin.write_all(&wav_buffer).await {
            Ok(_) => {},
            Err(e) => {
                error!("Failed to write to stdin: {}", e);
                return Ok(HttpResponse::InternalServerError().json(json!({
                    "error": format!("Failed to write to stdin: {}", e)
                })));
            }
        }
        drop(stdin);
    } else {
        error!("Failed to acquire stdin pipe for whisper-cli");
        return Ok(HttpResponse::InternalServerError().json(json!({
            "error": "Failed to acquire stdin pipe for whisper-cli"
        })));
    }

    let output = whisper_cmd.wait_with_output().await
        .map_err(|e| {
            error!("Failed to wait for whisper-cli output: {}", e);
            actix_web::error::ErrorInternalServerError(format!("Failed to wait for whisper-cli output: {}", e))
        })?;

    let transcription_segments: Vec<TranscriptionSegment> = if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();

        let mut segments = Vec::new();
        let mut current_time_line_raw = String::new();
        let mut current_text_lines = Vec::new();
        let mut in_segment_block = false;

        for line in stdout.lines() {
            let trimmed_line = line.trim();

            if trimmed_line.is_empty() {
                if in_segment_block && !current_time_line_raw.is_empty() && !current_text_lines.is_empty() {
                    let cleaned_time_str = current_time_line_raw.trim_matches(|c| c == '[' || c == ']').to_string();
                    let time_parts: Vec<&str> = cleaned_time_str.split(" --> ").collect();
                    let start_time = time_parts.get(0).unwrap_or(&"").trim().to_string();
                    let end_time = time_parts.get(1).unwrap_or(&"").trim().to_string();

                    segments.push(TranscriptionSegment {
                        start: start_time,
                        end: end_time,
                        text: current_text_lines.join("\n").trim().to_string(),
                    });
                    current_time_line_raw.clear();
                    current_text_lines.clear();
                    in_segment_block = false;
                }
                continue;
            }

            if trimmed_line == "WEBVTT" {
                continue;
            }

            if trimmed_line.contains("-->") {
                if in_segment_block && !current_time_line_raw.is_empty() && !current_text_lines.is_empty() {
                    let cleaned_time_str = current_time_line_raw.trim_matches(|c| c == '[' || c == ']').to_string();
                    let time_parts: Vec<&str> = cleaned_time_str.split(" --> ").collect();
                    let start_time = time_parts.get(0).unwrap_or(&"").trim().to_string();
                    let end_time = time_parts.get(1).unwrap_or(&"").trim().to_string();

                    segments.push(TranscriptionSegment {
                        start: start_time,
                        end: end_time,
                        text: current_text_lines.join("\n").trim().to_string(),
                    });
                }
                
                if let Some(bracket_index) = trimmed_line.find(']') {
                    let time_part = &trimmed_line[..=bracket_index];
                    let text_part_raw = &trimmed_line[bracket_index + 1..];

                    current_time_line_raw = time_part.trim().to_string();
                    current_text_lines.clear();
                    current_text_lines.push(text_part_raw.trim().to_string());
                    in_segment_block = true;
                } else {
                    current_time_line_raw = trimmed_line.to_string();
                    current_text_lines.clear();
                    in_segment_block = true;
                }
            } else if in_segment_block {
                current_text_lines.push(trimmed_line.to_string());
            }
        }

        if in_segment_block && !current_time_line_raw.is_empty() && !current_text_lines.is_empty() {
            let cleaned_time_str = current_time_line_raw.trim_matches(|c| c == '[' || c == ']').to_string();
            let time_parts: Vec<&str> = cleaned_time_str.split(" --> ").collect();
            let start_time = time_parts.get(0).unwrap_or(&"").trim().to_string();
            let end_time = time_parts.get(1).unwrap_or(&"").trim().to_string();

            segments.push(TranscriptionSegment {
                start: start_time,
                end: end_time,
                text: current_text_lines.join("\n").trim().to_string(),
            });
        }
        segments
    } else {
        let error = String::from_utf8_lossy(&output.stderr).to_string();
        error!("Transcription failed with stderr: {}", error);
        return Ok(HttpResponse::InternalServerError().json(json!({
            "error": format!("Transcription failed: {}", error)
        })));
    };

    info!("Requisição de transcrição finalizada com sucesso.");
    Ok(HttpResponse::Ok().json(json!({
        "transcription_segments": transcription_segments
    })))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::Builder::from_env(Env::default())
        .filter_level(LevelFilter::Error)
        .filter_module("rust_api_audio_to_text", LevelFilter::Info)
        .init();
    
    tokio::fs::create_dir_all("./models").await?;
    
    info!("Servidor Actix-web iniciando em http://0.0.0.0:6000"); 
    
    HttpServer::new(|| {
        let cors = Cors::default()
            // Allow requests from localhost for development
            .allowed_origin("http://localhost:8080") 
            // Allow requests from your Vercel deployed frontend
            .allowed_origin("https://cvto.vercel.app") 
            .allowed_methods(vec!["POST"]) 
            .allowed_headers(vec!["Content-Type", "Accept"]) 
            .max_age(3600); 

        App::new()
            .wrap(cors) 
            .service(transcribe_audio)
    })
    .bind(("0.0.0.0", 6000))?
    .run()
    .await?;

    info!("Servidor Actix-web encerrado.");
    Ok(())
}
use actix_multipart::Multipart;
use actix_web::{post, App, HttpResponse, HttpServer, Responder, Error};
use futures_util::TryStreamExt as _;
use tokio::process::Command;
use tokio::io::AsyncWriteExt;
use serde_json::json;
use serde::Serialize;
use std::path::Path;
use std::process::Stdio;
use log::{error, info};
use env_logger::Env;
use hound::WavReader;
use std::os::unix::fs::PermissionsExt;

// Define uma struct para representar cada segmento de transcrição
#[derive(Serialize)]
struct TranscriptionSegment {
    start: String, // Tempo de início do segmento
    end: String,   // Tempo de fim do segmento
    text: String,
}

#[post("/transcribe")]
async fn transcribe_audio(mut payload: Multipart) -> Result<impl Responder, Error> {
    // Buffer para armazenar o arquivo WAV na memória
    let mut wav_buffer = Vec::new();

    // Processa o payload multipart para extrair o arquivo WAV
    while let Some(mut field) = payload.try_next().await? {
        while let Some(chunk) = field.try_next().await? {
            wav_buffer.extend_from_slice(&chunk);
        }
    }

    if wav_buffer.is_empty() {
        error!("No WAV file provided in the request");
        return Ok(HttpResponse::BadRequest().json(json!({
            "error": "No WAV file provided"
        })));
    }

    info!("Received WAV file with size: {} bytes", wav_buffer.len());

    // Define o caminho para o arquivo do modelo Whisper
    let model_path = "./models/ggml-base.bin";
    if !Path::new(model_path).exists() {
        error!("Model file ggml-base.bin not found in ./models");
        return Ok(HttpResponse::InternalServerError().json(json!({
            "error": "Model file ggml-base.bin not found in ./models"
        })));
    }

    // Define o caminho para o binário whisper-cli
    // É crucial que este caminho seja idêntico e acessível tanto na sua máquina quanto no ambiente Docker.
    let binary_path = "/app/build/bin/whisper-cli"; 

    if !Path::new(&binary_path).exists() {
        error!("Whisper-cli binary not found at {}", binary_path);
        return Ok(HttpResponse::InternalServerError().json(json!({
            "error": format!("Whisper-cli binary not found at {}", binary_path)
        })));
    }
    // Verifica se o binário é executável
    if !Path::new(&binary_path).metadata().map(|m| m.permissions().mode() & 0o100 != 0).unwrap_or(false) {
        error!("Whisper-cli binary at {} is not executable", binary_path);
        return Ok(HttpResponse::InternalServerError().json(json!({
            "error": format!("Whisper-cli binary at {} is not executable", binary_path)
        })));
    }

    // Valida o formato do arquivo WAV usando a biblioteca hound
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

    // Configura o comando para executar o binário whisper-cli
    let mut whisper_cmd = Command::new(&binary_path) // Usa o caminho hardcoded
        .args([
            "-m", model_path,
            "-f", "-",      // Lê o áudio da entrada padrão
            "-l", "pt",     // Define o idioma para português
            "-ovtt",        // Formato de saída WebVTT
            "-of", "-",     // Escreve a saída para a saída padrão
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            error!("Failed to spawn whisper-cli command: {}", e);
            actix_web::error::ErrorInternalServerError(format!("Failed to spawn whisper-cli command: {}", e))
        })?;

    // Escreve o buffer WAV na entrada padrão do processo whisper-cli
    if let Some(mut stdin) = whisper_cmd.stdin.take() {
        const CHUNK_SIZE: usize = 4096;
        for chunk in wav_buffer.chunks(CHUNK_SIZE) {
            match stdin.write_all(chunk).await {
                Ok(_) => info!("Wrote {} bytes to stdin", chunk.len()),
                Err(e) => {
                    error!("Failed to write to stdin: {}", e);
                    return Ok(HttpResponse::InternalServerError().json(json!({
                        "error": format!("Failed to write to stdin: {}", e)
                    })));
                }
            }
        }
        drop(stdin); // Importante: Fecha a entrada padrão para sinalizar EOF ao processo filho
    } else {
        error!("Failed to acquire stdin pipe for whisper-cli");
        return Ok(HttpResponse::InternalServerError().json(json!({
            "error": "Failed to acquire stdin pipe for whisper-cli"
        })));
    }

    // Aguarda a conclusão do processo whisper-cli e captura sua saída
    let output = whisper_cmd.wait_with_output().await
        .map_err(|e| {
            error!("Failed to wait for whisper-cli output: {}", e);
            actix_web::error::ErrorInternalServerError(format!("Failed to wait for whisper-cli output: {}", e))
        })?;

    let transcription_segments: Vec<TranscriptionSegment> = if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string(); 
        info!("Transcription successful - stdout: '{}'", stdout);
        info!("Transcription successful - stderr: '{}'", stderr);

        // --- INÍCIO DA LÓGICA DE PARSE MAIS ROBUSTA PARA VTT ---
        let mut segments = Vec::new();
        let mut current_time_line_raw = String::new(); // Armazena a linha de tempo bruta
        let mut current_text_lines = Vec::new();
        let mut in_segment_block = false; // Flag para indicar se estamos dentro de um bloco de cue VTT

        for line in stdout.lines() {
            let trimmed_line = line.trim();

            if trimmed_line.is_empty() {
                // Uma linha vazia geralmente significa o fim de um bloco de cue VTT
                if in_segment_block && !current_time_line_raw.is_empty() && !current_text_lines.is_empty() {
                    // Extrai start e end da current_time_line_raw
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
                continue; // Pula linhas vazias, elas agem como separadores
            }

            // Verifica o cabeçalho "WEBVTT" e o pula
            if trimmed_line == "WEBVTT" {
                continue;
            }

            // Verifica se a linha contém "-->", indicando uma linha de timestamp
            if trimmed_line.contains("-->") {
                // Se já estávamos processando um segmento, o adiciona antes de iniciar um novo
                if in_segment_block && !current_time_line_raw.is_empty() && !current_text_lines.is_empty() {
                    // Extrai start e end da current_time_line_raw
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
                
                // Tenta dividir a linha em parte de tempo e parte de texto
                if let Some(bracket_index) = trimmed_line.find(']') {
                    let time_part = &trimmed_line[..=bracket_index]; // Inclui o ']'
                    let text_part_raw = &trimmed_line[bracket_index + 1..];

                    current_time_line_raw = time_part.trim().to_string(); // Armazena a linha de tempo bruta
                    current_text_lines.clear();
                    current_text_lines.push(text_part_raw.trim().to_string()); // Adiciona o texto desta linha
                    in_segment_block = true;
                } else {
                    // Fallback se ']' não for encontrado (formato inesperado), trata a linha inteira como tempo
                    current_time_line_raw = trimmed_line.to_string();
                    current_text_lines.clear();
                    in_segment_block = true;
                }
            } else if in_segment_block {
                // Se estamos dentro de um bloco de segmento e não é um timestamp, é texto
                current_text_lines.push(trimmed_line.to_string());
            }
        }

        // Após o loop, adiciona o último segmento coletado, se houver
        if in_segment_block && !current_time_line_raw.is_empty() && !current_text_lines.is_empty() {
            // Extrai start e end da current_time_line_raw
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
        // --- FIM DA LÓGICA DE PARSE MAIS ROBUSTA PARA VTT ---
    } else {
        let error = String::from_utf8_lossy(&output.stderr).to_string();
        error!("Transcription failed with stderr: {}", error);
        return Ok(HttpResponse::InternalServerError().json(json!({
            "error": format!("Transcription failed: {}", error)
        })));
    };

    Ok(HttpResponse::Ok().json(json!({
        "transcription_segments": transcription_segments
    })))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Inicializa o logger para exibir mensagens de info e erro
    env_logger::init_from_env(Env::default().default_filter_or("info"));
    // Garante que o diretório 'models' exista para armazenar o modelo Whisper
    tokio::fs::create_dir_all("./models").await?;
    info!("Starting server at http://0.0.0.0:8080"); // Alterado para 0.0.0.0 para compatibilidade com Docker
    HttpServer::new(|| {
        App::new().service(transcribe_audio)
    })
    .bind(("0.0.0.0", 8080))? // Ouve em todas as interfaces de rede para ser acessível de fora do contêiner
    .run()
    .await
}

#!/bin/sh
echo "---------------------------------------------------"
echo "Iniciando serviços da aplicação e túnel ngrok..."
echo "---------------------------------------------------"

# Configura o authtoken do ngrok em tempo de execução
ngrok config add-authtoken "$NGROK_AUTHTOKEN"

# Inicia a aplicação Rust em segundo plano
./rust_api_audio_to_text &

# Inicia o túnel ngrok em primeiro plano, tornando-o o processo principal do contêiner
exec ngrok http --url="$NGROK_URL" 6000

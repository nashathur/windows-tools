# One-off installer, run via a scheduled task (outside the Claude app's MSIX container).
$log = "C:\Users\thurs\tools\local-llm\install.log"
Start-Transcript -Path $log -Force
New-Item -ItemType Directory -Force "D:\ollama\models" | Out-Null
[Environment]::SetEnvironmentVariable("OLLAMA_MODELS", "D:\ollama\models", "User")
winget install --id Ollama.Ollama -e --silent --accept-package-agreements --accept-source-agreements
"EXIT: $LASTEXITCODE"
Stop-Transcript

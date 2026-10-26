# One-off: Ollama memory settings + restart, run via a scheduled task (outside the Claude app's
# MSIX container, so the user environment and the restarted Ollama are the real ones).
# Flash attention + 8-bit KV cache roughly halve the context's VRAM, so qwen3.5:9b fits 32k on the GPU.
$log = "C:\Tools\local-llm\ollama_settings.log"
Start-Transcript -Path $log -Force
[Environment]::SetEnvironmentVariable("OLLAMA_FLASH_ATTENTION", "1", "User")
[Environment]::SetEnvironmentVariable("OLLAMA_KV_CACHE_TYPE", "q8_0", "User")
Get-Process "ollama app", "ollama" -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 2
$env:OLLAMA_FLASH_ATTENTION = "1"
$env:OLLAMA_KV_CACHE_TYPE = "q8_0"
Start-Process "$env:LOCALAPPDATA\Programs\Ollama\ollama app.exe"
"done"
Stop-Transcript

#Requires -Version 5.1
$ErrorActionPreference = "Stop"

$RepoDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$BinSrc = Join-Path $RepoDir "target\release\aion.exe"
$InstallBinDir = Join-Path $env:USERPROFILE ".aion\bin"
$BinDest = Join-Path $InstallBinDir "aion.exe"
$ManifestsDir = Join-Path $RepoDir "manifests"
$SkillSrc = Join-Path $RepoDir ".agents\skills\aion-language\SKILL.md"
$SkillDestDir = Join-Path $env:USERPROFILE ".gemini\config\skills\aion-language"
$SkillDest = Join-Path $SkillDestDir "SKILL.md"
$McpConfigDir = Join-Path $env:USERPROFILE ".gemini\config"
$McpConfigPath = Join-Path $McpConfigDir "mcp_config.json"

Write-Host "==> 1/4 Compilando binario aion.exe (release)..."
cargo build --release --bin aion --manifest-path (Join-Path $RepoDir "Cargo.toml")
if ($LASTEXITCODE -ne 0) {
    throw "Error compilando aion.exe con cargo."
}

Write-Host "==> 2/4 Instalando aion.exe en PATH de usuario ($InstallBinDir)..."
New-Item -ItemType Directory -Force -Path $InstallBinDir | Out-Null
Copy-Item -Force $BinSrc $BinDest

$CurrentUserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (-not $CurrentUserPath) { $CurrentUserPath = "" }
$PathParts = $CurrentUserPath -split ";" | Where-Object { $_ -ne "" }
if ($PathParts -notcontains $InstallBinDir) {
    $NewPath = ($PathParts + $InstallBinDir) -join ";"
    [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
    $env:Path = "$env:Path;$InstallBinDir"
    Write-Host "    Agregado al PATH de usuario: $InstallBinDir"
} else {
    Write-Host "    Ya estaba en el PATH de usuario: $InstallBinDir"
}

Write-Host "==> 3/4 Instalando Skill global 'aion-language' para Antigravity..."
New-Item -ItemType Directory -Force -Path $SkillDestDir | Out-Null
Copy-Item -Force $SkillSrc $SkillDest
Write-Host "    Copiado: $SkillDest"

Write-Host "==> 4/4 Configurando servidor MCP global en $McpConfigPath..."
New-Item -ItemType Directory -Force -Path $McpConfigDir | Out-Null

$ConfigObj = @{}
if ((Test-Path $McpConfigPath) -and ((Get-Item $McpConfigPath).Length -gt 0)) {
    $Raw = Get-Content -Raw -Encoding UTF8 $McpConfigPath
    $Parsed = $Raw | ConvertFrom-Json
    if ($Parsed) {
        foreach ($prop in $Parsed.PSObject.Properties) {
            $ConfigObj[$prop.Name] = $prop.Value
        }
    }
}

$Servers = @{}
if ($ConfigObj.ContainsKey("mcpServers") -and $ConfigObj["mcpServers"]) {
    foreach ($prop in $ConfigObj["mcpServers"].PSObject.Properties) {
        $Servers[$prop.Name] = $prop.Value
    }
}

$Servers["aion"] = @{
    command = $BinDest
    args    = @("mcp", "--manifests-dir", $ManifestsDir)
}
$ConfigObj["mcpServers"] = $Servers

$JsonOut = $ConfigObj | ConvertTo-Json -Depth 10
[System.IO.File]::WriteAllText($McpConfigPath, "$JsonOut`n", (New-Object System.Text.UTF8Encoding($false)))

Write-Host ""
Write-Host "Instalacion de AION completada en Windows."
Write-Host "Abre una nueva terminal y verifica con: where.exe aion"
Write-Host "Prueba el verificador con: aion check `"$(Join-Path $RepoDir 'examples\promg_web_demo.aion')`""
Write-Host "Recuerda recargar MCP Servers / abrir una nueva conversacion en Antigravity."

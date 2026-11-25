#!/usr/bin/env bash
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_PATH="$REPO_DIR/target/release/aion"
MANIFESTS_DIR="$REPO_DIR/manifests"
SKILL_SRC="$REPO_DIR/.agents/skills/aion-language/SKILL.md"
SKILL_DEST="$HOME/.gemini/config/skills/aion-language/SKILL.md"
MCP_CONFIG="$HOME/.gemini/config/mcp_config.json"

echo "==> 1/4 Compilando binario aion (release)..."
cargo build --release --bin aion --manifest-path "$REPO_DIR/Cargo.toml"

echo "==> 2/4 Instalando 'aion' en PATH..."
if [ -d "/opt/homebrew/bin" ] && [ -w "/opt/homebrew/bin" ]; then
  TARGET_BIN="/opt/homebrew/bin/aion"
elif [ -d "/usr/local/bin" ] && [ -w "/usr/local/bin" ]; then
  TARGET_BIN="/usr/local/bin/aion"
else
  mkdir -p "$HOME/.local/bin"
  TARGET_BIN="$HOME/.local/bin/aion"
  echo "    Nota: Asegúrate de tener \$HOME/.local/bin en tu PATH."
fi
ln -sf "$BIN_PATH" "$TARGET_BIN"
echo "    Enlazado: $TARGET_BIN -> $BIN_PATH"

echo "==> 3/4 Instalando Skill global 'aion-language' para Antigravity..."
mkdir -p "$(dirname "$SKILL_DEST")"
cp "$SKILL_SRC" "$SKILL_DEST"
echo "    Copiado: $SKILL_DEST"

echo "==> 4/4 Configurando servidor MCP global en $MCP_CONFIG..."
mkdir -p "$(dirname "$MCP_CONFIG")"
python3 - "$MCP_CONFIG" "$BIN_PATH" "$MANIFESTS_DIR" <<'PY'
import json, os, sys

cfg_path, bin_path, manifests_dir = sys.argv[1], sys.argv[2], sys.argv[3]
data = {}
if os.path.exists(cfg_path) and os.path.getsize(cfg_path) > 0:
    with open(cfg_path, "r", encoding="utf-8") as f:
        data = json.load(f)

data.setdefault("mcpServers", {})
data["mcpServers"]["aion"] = {
    "command": bin_path,
    "args": ["mcp", "--manifests-dir", manifests_dir],
}

with open(cfg_path, "w", encoding="utf-8") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
PY

echo ""
echo "Instalación de AION completada."
echo "Verifica con: which aion && aion check $REPO_DIR/examples/promg_web_demo.aion"
echo "Recuerda recargar MCP Servers / abrir una nueva conversación en Antigravity."

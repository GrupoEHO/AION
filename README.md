# AION (AI-Oriented Native Full-Stack Language)

**AION** es un lenguaje de programación full-stack con compilador y runtimes escritos en **Rust**, diseñado específicamente para ser escrito, parcheado y verificado por **agentes de IA (LLMs)** con cero anidamiento (*flat graph*), contratos unificados Protobuf (`#M`), módulos externos verificados por ABI (`#EXT`), layout Flexbox determinístico (`taffy`) y diagnóstico JSON con sugerencias de parche (`fix_hint`).

---

## Estructura del Workspace

| Crate / Directorio | Descripción |
| :--- | :--- |
| `crates/aion-ast` | Grafo plano (`#CFG`, `#EXT`, `#SRV`, `#MW`, `#M`, `#API`, `#S`, `#N`), tipos y motor de parches transaccionales (`ADD`, `PATCH`, `DEL`). |
| `crates/aion-parser` | Lexer y parser con `chumsky` y errores léxicos/sintácticos en JSON. |
| `crates/aion-ext` | Cargador/validador ABI v1 (`scehma.json`), registro de plugins (`ExtPlugin`) y generador WIT. |
| `crates/aion-checker` | Verificador estático de tipos, ciclos (`E_CIRCULAR_REF`), contratos `#EXT`, `@err` obligatorio y *overflow* de layout (`E_UI_OVERFLOW`). |
| `crates/aion-runtime-net` | Servidor HTTP genérico (`axum` + `tokio`), pipeline `#MW`, Protobuf binario dinámico (`prost-reflect`) + fallback JSON y `GET /__aion/schema.proto`. |
| `crates/aion-runtime-ui` | Reactividad (`Signals`), layout (`taffy`), puente full-stack `CALL(#API...)` y renderizador GPU (`wgpu` + `cosmic-text`). |
| `crates/aion-cli` | Binario `aion` (`create`, `parse`, `check`, `patch`, `web`, `run`, `mcp`) y servidor MCP sobre `stdio`. |
| `plugins/aion-ext-firebase` | Plugin real `aion/firebase-auth@2.0.0` (JWT RS256 contra JWKS de Google + modo offline). |
| `plugins/aion-ext-mongo` | Plugin real `aion/mongo@1.2.0` sobre el driver oficial `mongodb` con traducción dinámica a BSON. |
| `manifests/` | Manifiestos JSON ABI v1 de módulos externos (`#EXT`). |
| `.agents/skills/aion-language/SKILL.md` | Skill global de AION para que cualquier agente de Antigravity conozca la sintaxis y flujo de trabajo. |

---

## Guía de Instalación en un Nuevo Dispositivo (macOS / Linux / Windows)

Para que **AION** quede instalado globalmente en tu máquina (como Rust o Go) y cualquier agente de **Antigravity** lo reconozca desde cualquier carpeta, necesitas 3 cosas:
1. El binario `aion` (`aion.exe` en Windows) en tu `PATH`.
2. El **Skill global** `aion-language` en `~/.gemini/config/skills/aion-language/SKILL.md` (`%USERPROFILE%\.gemini\config\skills\aion-language\SKILL.md` en Windows).
3. El **servidor MCP global** en `~/.gemini/config/mcp_config.json` (`%USERPROFILE%\.gemini\config\mcp_config.json` en Windows).

### Requisitos Previos
- **Rust y Cargo:**
  - macOS: `brew install rust` o [rustup.rs](https://rustup.rs)
  - Windows: [rustup-init.exe](https://rustup.rs) (incluye `cargo` y `rustc`; requiere Visual Studio Build Tools C++)
- **Python 3** (solo para `install.sh` en macOS/Linux; `install.ps1` en Windows usa PowerShell nativo y no necesita Python).

### Opción A: Instalación Automática (1 comando)

**macOS / Linux:**
```bash
git clone https://github.com/GrupoEHO/AION.git
cd AION
chmod +x install.sh
./install.sh
```

**Windows (PowerShell):**
```powershell
git clone https://github.com/GrupoEHO/AION.git
cd AION
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

El instalador compila `aion` en modo `release`, lo agrega a tu `PATH` de usuario (`%USERPROFILE%\.aion\bin\aion.exe` en Windows; `/opt/homebrew/bin/aion` o `/usr/local/bin/aion` en macOS/Linux), instala el skill global `aion-language` y registra el servidor MCP `aion` en `mcp_config.json` sin borrar otros servidores existentes.

---

### Opción B: Instalación Manual Paso a Paso

#### 1. Clonar y compilar `aion`
```bash
git clone https://github.com/GrupoEHO/AION.git
cd AION
cargo build --release --bin aion
```

#### 2. Agregar `aion` al `$PATH` global
En **macOS (Apple Silicon / Homebrew)**:
```bash
ln -sf "$(pwd)/target/release/aion" /opt/homebrew/bin/aion
```
En **macOS (Intel) / Linux**:
```bash
ln -sf "$(pwd)/target/release/aion" /usr/local/bin/aion
# O si prefieres sin sudo:
# mkdir -p ~/.local/bin && ln -sf "$(pwd)/target/release/aion" ~/.local/bin/aion
```

Verifica que quedó en el `$PATH`:
```bash
which aion
```

#### 3. Instalar el Skill Global en Antigravity
Esto permite que cualquier agente en cualquier workspace reconozca la sintaxis de AION automáticamente:
```bash
mkdir -p ~/.gemini/config/skills/aion-language
cp .agents/skills/aion-language/SKILL.md ~/.gemini/config/skills/aion-language/SKILL.md
```

#### 4. Configurar el Servidor MCP Global en Antigravity
Edita (o crea) `~/.gemini/config/mcp_config.json` reemplazando `/RUTA/ABSOLUTA/A/AION` por la ruta real donde clonaste el repositorio:

```json
{
  "mcpServers": {
    "aion": {
      "command": "/RUTA/ABSOLUTA/A/AION/target/release/aion",
      "args": [
        "mcp",
        "--manifests-dir",
        "/RUTA/ABSOLUTA/A/AION/manifests"
      ]
    }
  }
}
```

#### 5. Recargar Antigravity y Verificar
1. En Antigravity ve a **Additional Options (`...`) > MCP Servers** y verifica que aparezca **`aion`** con las **6 herramientas**:
   - `aion_create`
   - `aion_check`
   - `aion_patch`
   - `aion_inspect_ext`
   - `aion_layout_tree`
   - `aion_spec`
2. Abre una **conversación nueva** en cualquier carpeta vacía y prueba pedirle al agente:
   > *"Crea un proyecto nuevo llamado `mi_app` en AION y valídalo con `aion_check`"*

---

## Comandos del CLI `aion`

```bash
# Crear un proyecto mínimo canónico (aion.toml, app.aion, AGENTS.md)
aion create mi_app

# Validar sintaxis, tipos, #EXT, @err y layout móvil/desktop (salida JSON)
aion check examples/promg_web_demo.aion

# Ver el AST plano en JSON
aion parse examples/promg_web_demo.aion

# Aplicar parches diferenciales transaccionales (ADD/PATCH/DEL) y validar
aion patch app.aion patch.json

# Levantar demo web interactiva + backend HTTP/Protobuf en http://127.0.0.1:8080
aion web examples/promg_web_demo.aion

# Levantar solo el servidor HTTP/Protobuf de producción
aion run app.aion

# Ejecutar servidor MCP por stdio (usado por Antigravity / agentes)
aion mcp --manifests-dir ./manifests
```

## Desarrollo y Pruebas
```bash
cargo test --workspace
```

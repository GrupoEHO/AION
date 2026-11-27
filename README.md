> **Idioma:** [English](./README.en.md) | **Español**

# AION (AI-Oriented Native Full-Stack Language)

## ¿Qué es AION?

**AION** es un lenguaje de programación full-stack nativo —con compilador, verificador estático, runtimes de backend/UI y servidor MCP escritos en **Rust**— diseñado **exclusivamente para ser escrito, parcheado y mantenido por agentes de IA (LLMs)**.

En AION, un solo archivo `.aion` describe el backend (servidor HTTP/Protobuf, middlewares, modelos de datos, endpoints e integraciones externas) y el frontend reactivo (estado `Signals` y árbol de interfaz) usando un **grafo plano de nodos** (`#CFG`, `#EXT`, `#SRV`, `#MW`, `#M`, `#API`, `#S`, `#N`).

---

## ¿Por qué se hace AION?

Los stacks tradicionales (por ejemplo, **Flutter + Go** o **React + Node**) fueron diseñados para humanos, pero introducen fricción constante cuando el desarrollo es liderado por agentes de IA:

1. **Anidamiento profundo vs. Grafo Plano:** En UIs declarativas tradicionales, modificar un botón implica reescribir bloques profundamente anidados donde los LLMs pierden contexto o rompen llaves/paréntesis. En AION **no existe el anidamiento**: cada elemento es un nodo plano identificado por `#ID` que puede modificarse con parches diferenciales atómicos (`ADD`, `PATCH`, `DEL`).
2. **Deriva entre Backend y Frontend:** Mantener structs en Go, archivos `.proto` y modelos en Dart duplica esfuerzo y genera incompatibilidades. En AION, un nodo `#M` es la **única fuente de verdad** para backend, Protobuf y frontend.
3. **Alucinaciones de librerías externas:** Los LLMs suelen inventar métodos o firmas inexistentes de SDKs de terceros. En AION, las integraciones (`MongoDB`, `Firebase Auth`, etc.) viven desacopladas como módulos `#EXT` con un **manifiesto de contrato ABI v1**; el compilador rechaza cualquier llamada inválida devolviendo la firma exacta esperada en JSON.
4. **Errores silenciosos y layouts rotos:** AION obliga en tiempo de compilación a declarar manejadores `@err` en toda llamada `#API` desde la UI y calcula el layout Flexbox con `taffy` antes de ejecutar para detectar desbordamientos (`E_UI_OVERFLOW`) en móvil (`390×844`) y desktop (`1440×900`).
5. **Compilador orientado a agentes:** Todo error léxico, de sintaxis, tipos, contratos `#EXT` o layout se emite en **JSON estructurado** con `fix_hint` accionable para que el agente se autocorrija en un ciclo cerrado vía MCP.

---

## ¿Cuál es su intención?

El objetivo de AION es convertirse en una **plataforma full-stack de alto rendimiento gobernada por agentes de IA**, capaz de:
- Sustituir progresivamente stacks multiproyecto complejos (como aplicaciones empresariales en producción de cientos de miles de líneas en **Flutter + Go**) mediante migraciones incrementales por *vertical slices*.
- Reducir drásticamente el consumo de tokens, el *boilerplate* (proyectos canónicos de 3 archivos: `aion.toml`, `app.aion`, `AGENTS.md`) y los errores en producción gracias a verificación estática estricta.
- Permitir que cualquier agente (en Antigravity u otro cliente MCP) cree, valide, parche y pruebe aplicaciones completas de punta a punta de forma autónoma.

---

## ¿Dónde está parado hoy? (Estado Actual — v0.1 MVP)

Los **6 hitos fundacionales** definidos en [roadmap.md](./roadmap.md) están completados y verificados con `30/30` pruebas automatizadas:

| Área | Estado Actual (v0.1) | Próxima Evolución (Fase 2 / Producción) |
| :--- | :--- | :--- |
| **Lenguaje, Parser y AST** | Completo (`aion-ast`, `aion-parser` con `chumsky`, parches transaccionales `ADD`/`PATCH`/`DEL`). | *Unparser* canónico (`AST -> .aion`) para escritura directa en disco desde `aion patch --write`. |
| **Verificador (`aion-checker`)** | Tipos, ciclos (`E_CIRCULAR_REF`), contratos `#EXT`, `@err` obligatorio y *overflow* visual (`E_UI_OVERFLOW`). | Validaciones avanzadas de seguridad/permisos (`caps`), flujos condicionales y análisis de rutas. |
| **Backend (`aion-runtime-net`)** | Servidor HTTP `axum` + `tokio`, middlewares `#MW`, parámetros `:id`/query, Protobuf binario dinámico (`prost-reflect`), fallback JSON y `/__aion/schema.proto`. | WebSockets/SSE, observabilidad/métricas y empaquetado de despliegue optimizado. |
| **Plugins `#EXT`** | ABI v1 + generador WIT + plugins reales `aion-ext-mongo` y `aion-ext-firebase`. | Carga dinámica de módulos WebAssembly Components (`.wasm` / WIT) y más conectores (GCS, PDF, CFDI/SAT, pagos, cron). |
| **UI (`aion-runtime-ui`)** | Motor reactivo `Signals`, layout `taffy` (`ROOT`, `FLEX`, `NAV`, `LIST`, `TXT`, `INP`, `BTN`), puente `CALL(#API...)`, base `wgpu` + `cosmic-text` y preview web en vivo (`aion web`). | Loop interactivo nativo GPU completo, compilación directa a `wasm32-unknown-unknown` (WebGPU/WebGL) y targets ARM64 (Android/iOS). |
| **CLI y MCP (`aion-cli`)** | Binario global `aion` (`create`, `parse`, `check`, `patch`, `web`, `run`, `mcp`), 6 herramientas MCP sobre `stdio` y Skill global `aion-language`. | `aion build --target web\|android\|ios`, *hot-reload* y herramientas de migración asistida. |

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

# Compilar un Android App Bundle (.aab) listo para Google Play (lee aion.toml automáticamente)
aion build appbundle --release

# Compilar app nativa iOS (.app para Simulador/Dispositivo) o paquete .ipa (requiere macOS + Xcode)
aion build ios --simulator
aion build ipa --release --no-codesign
# Con firma de Apple Developer para App Store / TestFlight:
# aion build ipa --release --team-id TU_TEAM_ID --bundle-id com.tuempresa.miapp

# Compilar un bundle Web estático de producción en build/web/ (HTML ligero o WebAssembly real con --wasm)
aion build web --release
aion build web --wasm --release

# Levantar solo el servidor HTTP/Protobuf de producción
aion run app.aion

# Ejecutar servidor MCP por stdio (usado por Antigravity / agentes)
aion mcp --manifests-dir ./manifests
```

## Desarrollo y Pruebas
```bash
cargo test --workspace
```

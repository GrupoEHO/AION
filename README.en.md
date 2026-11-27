> **Language:** **English** | [Español](./README.md)

# AION (AI-Oriented Native Full-Stack Language)

## What is AION?

**AION** is a native full-stack programming language — with its compiler, static checker, backend/UI runtimes, and MCP server written in **Rust** — designed **exclusively to be written, patched, and maintained by AI agents (LLMs)**.

In AION, a single `.aion` file describes both the backend (HTTP/Protobuf server, middlewares, data models, endpoints, and external integrations) and the reactive frontend (`Signals` state and UI layout tree) using a **flat node graph** (`#CFG`, `#EXT`, `#SRV`, `#MW`, `#M`, `#API`, `#S`, `#N`).

---

## Why AION?

Traditional stacks (such as **Flutter + Go** or **React + Node**) were designed for human readability, but introduce constant friction when development is led by AI agents:

1. **Deep Nesting vs. Flat Graph:** In traditional declarative UIs, editing a button often requires rewriting deeply nested widget trees where LLMs lose context or break delimiters. In AION there is **zero nesting**: every element is a flat node identified by `#ID` that can be mutated via atomic differential patches (`ADD`, `PATCH`, `DEL`).
2. **Backend/Frontend Drift:** Maintaining Go structs, `.proto` files, and Dart models duplicates work and causes contract mismatches. In AION, a `#M` node is the **single source of truth** for backend, Protobuf, and frontend.
3. **External Library Hallucinations:** LLMs frequently invent nonexistent methods or wrong signatures on third-party SDKs. In AION, external integrations (`MongoDB`, `Firebase Auth`, etc.) live decoupled as `#EXT` modules with a strict **ABI v1 contract manifest**; the compiler rejects invalid calls and returns the exact expected signature in JSON.
4. **Silent Errors & Broken Layouts:** AION enforces `@err` handlers at compile time on every UI `#API` call and statically computes Flexbox layouts with `taffy` to catch overflows (`E_UI_OVERFLOW`) on mobile (`390×844`) and desktop (`1440×900`) viewports before execution.
5. **Agent-First Compiler:** Every lexical, syntax, type, `#EXT` contract, or layout error is emitted as **structured JSON** with an actionable `fix_hint` so AI agents can self-correct autonomously via MCP.

---

## Vision & Intent

AION's goal is to become a **high-performance, agent-governed full-stack platform** capable of:
- Progressively replacing complex multi-repo stacks (such as large production **Flutter + Go** enterprise applications) through incremental *vertical slice* migrations.
- Dramatically reducing token usage, boilerplate (canonical 3-file projects: `aion.toml`, `app.aion`, `AGENTS.md`), and production regressions via strict static verification.
- Enabling any AI agent (in Antigravity or any MCP client) to scaffold, validate, patch, and test full-stack applications end-to-end autonomously.

---

## Current Status (v0.1 MVP)

All **6 foundational milestones** in [roadmap.md](./roadmap.md) are completed and verified (`30/30` automated tests passing):

| Area | Current Status (v0.1) | Next Evolution (Phase 2 / Production) |
| :--- | :--- | :--- |
| **Language, Parser & AST** | Complete (`aion-ast`, `aion-parser` via `chumsky`, transactional `ADD`/`PATCH`/`DEL` patches). | Canonical unparser (`AST -> .aion`) for direct file writes via `aion patch --write`. |
| **Checker (`aion-checker`)** | Types, cycles (`E_CIRCULAR_REF`), `#EXT` ABI contracts, mandatory `@err`, and layout overflow (`E_UI_OVERFLOW`). | Advanced capability/security checks (`caps`), conditional control flow, and route analysis. |
| **Backend (`aion-runtime-net`)** | Generic `axum` + `tokio` HTTP server, `#MW` pipelines, `:id`/query params, dynamic binary Protobuf (`prost-reflect`), JSON fallback, and `/__aion/schema.proto`. | WebSockets/SSE, observability/metrics, and optimized deployment packaging. |
| **`#EXT` Plugins** | ABI v1 + WIT generator + real `aion-ext-mongo` and `aion-ext-firebase` plugins. | Dynamic WebAssembly Component (`.wasm` / WIT) loading and additional connectors (GCS, PDF, CFDI/SAT, payments, cron). |
| **UI (`aion-runtime-ui`)** | `Signals` reactivity, `taffy` layout (`ROOT`, `FLEX`, `NAV`, `LIST`, `TXT`, `INP`, `BTN`), `CALL(#API...)` bridge, `wgpu` + `cosmic-text` foundation, and live Web Preview (`aion web`). | Full native GPU interactive event loop, `wasm32-unknown-unknown` (WebGPU/WebGL) build target, and ARM64 (Android/iOS) targets. |
| **CLI & MCP (`aion-cli`)** | Global `aion` binary (`create`, `parse`, `check`, `patch`, `web`, `run`, `mcp`), 6 MCP tools over `stdio`, and global `aion-language` Skill. | `aion build --target web\|android\|ios`, hot-reload, and assisted migration tooling. |

---

## Workspace Structure

| Crate / Directory | Description |
| :--- | :--- |
| `crates/aion-ast` | Flat graph (`#CFG`, `#EXT`, `#SRV`, `#MW`, `#M`, `#API`, `#S`, `#N`), types, and transactional patch engine (`ADD`, `PATCH`, `DEL`). |
| `crates/aion-parser` | `chumsky`-based lexer/parser with structured JSON diagnostics. |
| `crates/aion-ext` | ABI v1 manifest loader/validator (`scehma.json`), plugin registry (`ExtPlugin`), and WIT generator. |
| `crates/aion-checker` | Static verifier for types, circular refs (`E_CIRCULAR_REF`), `#EXT` contracts, mandatory `@err`, and layout overflow (`E_UI_OVERFLOW`). |
| `crates/aion-runtime-net` | Generic `axum` + `tokio` HTTP server, `#MW` pipeline, dynamic binary Protobuf (`prost-reflect`) + JSON fallback, and `GET /__aion/schema.proto`. |
| `crates/aion-runtime-ui` | `Signals` reactivity, `taffy` layout, full-stack `CALL(#API...)` bridge, and `wgpu` + `cosmic-text` GPU renderer. |
| `crates/aion-cli` | `aion` CLI binary (`create`, `parse`, `check`, `patch`, `web`, `run`, `mcp`) and `stdio` MCP server. |
| `plugins/aion-ext-firebase` | Real `aion/firebase-auth@2.0.0` plugin (RS256 JWT validation against Google JWKS + offline RSA test mode). |
| `plugins/aion-ext-mongo` | Real `aion/mongo@1.2.0` plugin backed by the official `mongodb` driver with dynamic BSON translation. |
| `manifests/` | External module ABI v1 JSON manifests (`#EXT`). |
| `.agents/skills/aion-language/SKILL.md` | Global AION skill so any Antigravity agent knows the language syntax and workflow automatically. |

---

## Installation Guide on a New Machine (macOS / Linux / Windows)

To install **AION** globally (just like Rust or Go) so every **Antigravity** agent recognizes it from any folder, 3 components are set up:
1. The `aion` (`aion.exe` on Windows) binary on your `PATH`.
2. The global **`aion-language` Skill** at `~/.gemini/config/skills/aion-language/SKILL.md` (`%USERPROFILE%\.gemini\config\skills\aion-language\SKILL.md` on Windows).
3. The global **MCP server** entry in `~/.gemini/config/mcp_config.json` (`%USERPROFILE%\.gemini\config\mcp_config.json` on Windows).

### Prerequisites
- **Rust & Cargo:**
  - macOS: `brew install rust` or [rustup.rs](https://rustup.rs)
  - Windows: [rustup-init.exe](https://rustup.rs) (includes `cargo` and `rustc`; requires Visual Studio C++ Build Tools)
- **Python 3** (only needed for `install.sh` on macOS/Linux; `install.ps1` uses native PowerShell on Windows).

### Option A: One-Command Automatic Install

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

The installer builds `aion` in release mode, adds it to your user `PATH` (`%USERPROFILE%\.aion\bin\aion.exe` on Windows; `/opt/homebrew/bin/aion` or `/usr/local/bin/aion` on macOS/Linux), installs the global `aion-language` skill, and merges the `aion` MCP server into `mcp_config.json` without overwriting existing entries.

---

### Option B: Manual Step-by-Step Installation

#### 1. Clone and build `aion`
```bash
git clone https://github.com/GrupoEHO/AION.git
cd AION
cargo build --release --bin aion
```

#### 2. Add `aion` to your global `PATH`
On **macOS (Apple Silicon / Homebrew)**:
```bash
ln -sf "$(pwd)/target/release/aion" /opt/homebrew/bin/aion
```
On **macOS (Intel) / Linux**:
```bash
ln -sf "$(pwd)/target/release/aion" /usr/local/bin/aion
# Or without sudo:
# mkdir -p ~/.local/bin && ln -sf "$(pwd)/target/release/aion" ~/.local/bin/aion
```

#### 3. Install the Global Skill in Antigravity
```bash
mkdir -p ~/.gemini/config/skills/aion-language
cp .agents/skills/aion-language/SKILL.md ~/.gemini/config/skills/aion-language/SKILL.md
```

#### 4. Configure the Global MCP Server in Antigravity
Add the `aion` server to `~/.gemini/config/mcp_config.json` (replacing `/ABSOLUTE/PATH/TO/AION` with your clone path):

```json
{
  "mcpServers": {
    "aion": {
      "command": "/ABSOLUTE/PATH/TO/AION/target/release/aion",
      "args": [
        "mcp",
        "--manifests-dir",
        "/ABSOLUTE/PATH/TO/AION/manifests"
      ]
    }
  }
}
```

#### 5. Reload Antigravity and Verify
1. In Antigravity, open **Additional Options (`...`) > MCP Servers** and verify **`aion`** is connected with all **6 tools**:
   - `aion_create`
   - `aion_check`
   - `aion_patch`
   - `aion_inspect_ext`
   - `aion_layout_tree`
   - `aion_spec`
2. Open a **new conversation** in any empty folder and try:
   > *"Create a new AION project called `my_app` and validate it with `aion_check`"*

---

## `aion` CLI Commands

```bash
# Scaffold a minimal canonical project (aion.toml, app.aion, AGENTS.md)
aion create my_app

# Validate syntax, types, #EXT contracts, @err handlers, and mobile/desktop layout (JSON)
aion check examples/promg_web_demo.aion

# Output the flat AST as JSON
aion parse examples/promg_web_demo.aion

# Apply transactional differential patches (ADD/PATCH/DEL) and validate
aion patch app.aion patch.json

# Launch live interactive Web Preview + HTTP/Protobuf backend on http://127.0.0.1:8080
aion web examples/promg_web_demo.aion

# Compile an Android App Bundle (.aab) ready for Google Play (auto-reads aion.toml)
aion build appbundle --release

# Compile a native iOS app (.app for Simulator/Device) or .ipa archive (requires macOS + Xcode)
aion build ios --simulator
aion build ipa --release --no-codesign
# Signed for App Store / TestFlight with your Apple Developer Team ID:
# aion build ipa --release --team-id YOUR_TEAM_ID --bundle-id com.yourcompany.myapp

# Compile a standalone production Web bundle in build/web/ (lightweight HTML or real WebAssembly with --wasm)
aion build web --release
aion build web --wasm --release

# Run the production HTTP/Protobuf backend server
aion run app.aion

# Run the stdio MCP server (used by Antigravity / AI agents)
aion mcp --manifests-dir ./manifests
```

## Development & Tests
```bash
cargo test --workspace
```

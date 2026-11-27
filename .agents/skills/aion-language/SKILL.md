---
name: aion-language
description: >-
  Complete language specification, syntax reference, CLI, and MCP workflow for AION
  (AI-Oriented Native Full-Stack Language). Trigger automatically whenever the user
  mentions AION, `.aion` files, `aion_check`, `aion_create`, `aion_layout_tree`, or
  asks to build, scaffold, edit, check, or run an AION application.
---

# AION Language Reference & Agent Workflow

**AION** is a flat-graph, full-stack programming language compiled by a Rust toolchain (`aion` CLI + `aion` MCP server) installed globally on this machine.

## 1. Core Architectural Rules
1. **Zero Nesting (Flat Graph):** Never nest widget trees or blocks. Every declaration is a top-level flat node identified by `#PREFIX+ID` (`#CFG`, `#EXT`, `#SRV0`, `#MW0`, `#M0`, `#API0`, `#S0`, `#N0`...).
2. **Single Contract (`#M`):** Data models generate Protobuf + typed backend/frontend contracts simultaneously.
3. **External Modules (`#EXT`):** Databases and third-party services are NEVER hardcoded in the language. Always call MCP tool `aion_inspect_ext` first to inspect exact available `#EXT` functions, argument types, and error codes.
4. **Mandatory `@err` Handler:** Every UI node (`#N`) that invokes `CALL(#API..., ...)` MUST also declare `@err->...` (e.g. `@err->TOAST("Error")`), otherwise `aion_check` rejects it with `E_MISSING_ERR_HANDLER`.
5. **Layout Safety:** Every UI tree is statically laid out with Taffy for `mobile` (`390x844`) and `desktop` (`1440x900`). Any child exceeding its non-scrollable container emits `E_UI_OVERFLOW`.

## 2. Installed CLI & MCP Tools
- **CLI (on `$PATH`):**
  - `aion create <name> [dir]` — Scaffolds a minimal canonical project (`aion.toml`, `app.aion`, `AGENTS.md`).
  - `aion check <file.aion>` — Validates syntax, types, cycles, `#EXT` calls, `@err`, and layout (`mobile` + `desktop`) as JSON.
  - `aion parse <file.aion>` — Outputs flat AST JSON.
  - `aion patch <file.aion> <patch.json>` — Applies transactional `ADD`/`PATCH`/`DEL` AST patches and validates.
  - `aion web <file.aion>` — Runs live HTTP/Protobuf backend + interactive Web Preview (with Hot Reload & Hot Restart) at `http://127.0.0.1:8080`.
  - `aion build appbundle --release [file.aion]` (or `aion build android`) — Validates and compiles a native Android App Bundle (`.aab`) via Gradle (auto-reads `aion.toml` when `[file.aion]` is omitted).
  - `aion build web --release [--wasm] [file.aion]` — Validates and compiles a static production Web bundle (`build/web/index.html`, `aion_bundle.json`, `schema.proto`; with `--wasm`, compiles Rust `taffy` + `Signals` into `pkg/aion_web_wasm_bg.wasm`).
  - `aion run <file.aion>` — Runs production `axum` + Protobuf backend server.
- **MCP Tools (`aion` server):**
  - `aion_create`: Scaffold a new AION project (`{ name, dir? }`).
  - `aion_inspect_ext`: List or inspect `#EXT` ABI manifests & WIT (`{ module_id?, version? }`).
  - `aion_check`: Full compiler verification returning structured JSON diagnostics (`{ source }`).
  - `aion_patch`: Transactional AST patching + validation (`{ source, patches }`).
  - `aion_layout_tree`: Compute Taffy layout boxes & detect overflows (`{ source, viewport?: "mobile" | "desktop" }`).
  - `aion_spec`: Returns the concise AION grammar and examples.
  - `aion_build_android`: Compiles an AION program into an Android App Bundle (`.aab`).
  - `aion_build_web`: Compiles an AION program into a standalone static Web bundle (`build/web/`).

## 3. Primitive & Composite Types
- Primitives: `i32`, `f64`, `str`, `bool`, `bytes`, `ts`, `void`, `any`
- Composites: `list<T>`, `map<K,V>`
- Model reference: `#M0`, `#M1`, ...
- External ABI struct: `@AuthUser`, `@MongoDoc` (defined in `#EXT` manifest `types`)

## 4. Node Syntax Reference

### Configuration & External Modules
```aion
#CFG:ENV{MONGO_URI, FB_PROJ_ID}
#EXT:db = "aion/mongo@1.2"
#EXT:auth = "aion/firebase-auth@2.0"
```
Access env vars in `#MW`/`#API` via `CFG.MONGO_URI`, `CFG.FB_PROJ_ID`.

### HTTP Server (`#SRV`)
```aion
#SRV0:HTTP[port:8080, proto:on]
```

### Middleware (`#MW`)
Must terminate with `-> NEXT` or `-> ERR(status, msg)`. Short-circuit fallible calls with `? ERR(status, msg)`.
Built-in `ctx` methods: `ctx.hdr(name) -> str`, `ctx.get(key) -> any`, `ctx.set(key, val) -> void`, `ctx.param(name) -> str`, `ctx.query(name) -> str`.
```aion
#MW0:PIPE(ctx) {
  let tok = ctx.hdr("Authorization")
  let usr = auth.verify(tok, CFG.FB_PROJ_ID) ? ERR(401, "invalid_token")
  ctx.set("uid", usr.uid) -> NEXT
}
```

### Data Models (`#M`)
```aion
#M0:User{id:str, uid:str, pts:i32}
#M1:Budget{id:str, category:str, limit:f64, spent:f64, tags:list<str>}
```

### API Endpoints (`#API`)
Supports `GET`, `POST`, `PUT`, `PATCH`, `DELETE`, Gin-style `:id` or `{id}` route params, optional middlewares, `IN{...}`, and `OUT{#M0}` (or `OUT{field:type}` / `OUT{list<#M0>}`). Return with `-> RET`. Access input fields via `in.<field>`. MongoDB `$`-prefixed keys like `$inc`, `$set` are valid object keys.
```aion
#API0:#SRV0.POST["/v1/pts", #MW0] IN{delta:i32} OUT{#M0} {
  db.connect(CFG.MONGO_URI)
  db.upd("users", {uid:ctx.get("uid")}, {$inc:{pts:in.delta}}) -> RET
}

#API1:#SRV0.GET["/v1/budgets/:id", #MW0] OUT{#M1} {
  db.connect(CFG.MONGO_URI)
  db.find_one("budgets", {id:ctx.param("id"), uid:ctx.get("uid")}) -> RET
}
```

### Reactive State (`#S`)
```aion
#S0:i32=0
#S1:str=""
```

### UI Nodes (`#N`) — Positional Arguments (Zero Nesting)
- `ROOT[pad_x, pad_y, width, height, bg_hex]{#N1}` — Root container (`100%, 100%`).
- `FLEX[dir(v|h), justify(c|s|e|sb|sa), align(c|s|e|st), gap, scroll?]{#N2,#N3}` — Flexbox container.
- `NAV[width, height, bg_hex]{#N_tab1,#N_tab2}` — Horizontal navigation bar.
- `LIST[dir(v|h), gap, noscroll?]{#N_item}` — Scrollable list container.
- `TXT[font_size, weight, color_hex]{"Text with {#S0} interpolation"}` — Text label.
- `INP[width, height, radius, bg_hex, #S_bound]{"Placeholder..."}` — Text input bound to signal `#S_bound`.
- `BTN[width, height, radius, bg_hex]{#N_label} @tap->... @err->...` — Interactive button.

### UI Event Handlers
- API Call with signal binding (requires `@err` on the same node!):
  `@tap->CALL(#API0,{delta:10})=>#S0=res.pts @err->TOAST("Error")`
- Optional loading handler:
  `@loading->TOAST("Cargando...")`
- Navigation:
  `@tap->NAV(#N_screen)`
- Direct signal mutation:
  `@tap->#S0=0`

## 5. Complete Valid Example
```aion
#CFG:ENV{MONGO_URI, FB_PROJ_ID}
#EXT:db = "aion/mongo@1.2"
#EXT:auth = "aion/firebase-auth@2.0"

#SRV0:HTTP[port:8080, proto:on]

#MW0:PIPE(ctx) {
  let tok = ctx.hdr("Authorization")
  let usr = auth.verify(tok, CFG.FB_PROJ_ID) ? ERR(401, "invalid_token")
  ctx.set("uid", usr.uid) -> NEXT
}

#M0:User{id:str, uid:str, pts:i32}

#API0:#SRV0.POST["/v1/pts", #MW0] IN{delta:i32} OUT{#M0} {
  db.connect(CFG.MONGO_URI)
  db.upd("users", {uid:ctx.get("uid")}, {$inc:{pts:in.delta}}) -> RET
}

#S0:i32=0
#N0:ROOT[0,0,100%,100%,#F8FAFC]{#N1}
#N1:FLEX[v,c,c,16]{#N2,#N3}
#N2:TXT[24,700,#0F172A]{"Puntos: {#S0}"}
#N3:BTN[160,48,8,#2563EB]{#N4} @tap->CALL(#API0,{delta:10})=>#S0=res.pts @err->TOAST("Error")
#N4:TXT[16,600,#FFFFFF]{"Sumar +10"}
```

## 6. Mandatory Agent Workflow
1. If starting a new project, call `aion_create` (or `aion create <name>`).
2. Call `aion_inspect_ext` before writing any `#EXT` calls.
3. Write or patch `.aion` files and **always** run `aion_check` until `ok: true` (`0` errors).
4. Run `aion_layout_tree` for both `mobile` and `desktop` viewports to confirm zero overflows.
5. To preview live in a browser, run `aion web <file.aion>`.

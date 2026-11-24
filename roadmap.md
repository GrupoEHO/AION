# ROADMAP DE DESARROLLO: AION (AI-Oriented Native Full-Stack Language)

## REGLAS DE EJECUCIÓN PARA EL AGENTE (ANTIGRAVITY)
1. **Un Hito a la vez:** Completa todas las tareas de un hito antes de pasar al siguiente.
2. **Cero Regresiones:** Después de cada hito, ejecuta `cargo test --workspace`. Todos los tests anteriores y nuevos deben pasar (0 failed).
3. **Actualización de Estado:** Marca cada tarea completada cambiando `[ ]` por `[x]` en este archivo.
4. **Aislamiento de Dependencias:** Mantén `aion-runtime-net` y `aion-runtime-ui` agnósticos a proveedores; toda integración externa (MongoDB, Firebase) debe implementarse como un crate separado que implemente el trait `ExtPlugin` de `aion-ext`.

---

## HITO 1: Núcleo del Lenguaje, Parser y Verificador [COMPLETADO]
- [x] Estructura del Cargo Workspace (`aion-ast`, `aion-parser`, `aion-ext`, `aion-checker`, `aion-runtime-net`, `aion-cli`).
- [x] `aion-ast`: Grafo plano, tipos primitivos/compuestos, motor de parches transaccionales (`ADD`, `PATCH`, `DEL`) y diagnósticos JSON.
- [x] `aion-parser`: Lexer y Parser (`chumsky`) con errores léxicos/sintácticos en JSON.
- [x] `aion-ext`: Cargador/validador de manifiestos ABI v1, `ExtRegistry` y generador WIT.
- [x] `aion-checker`: Validación de tipos, ciclos (`E_CIRCULAR_REF`), llamadas `#EXT` y obligatoriedad de `@err`.

---

## HITO 2: Runtime de Backend Genérico (`aion-runtime-net`) [COMPLETADO]
**Objetivo:** Ejecutar un grafo AION compilado como un servidor HTTP real con soporte Protobuf y pipeline de middlewares genéricos.

- [x] **2.1 Motor de Ejecución de Expresiones (Eval):** Implementar un evaluador ligero en `aion-runtime-net` capaz de ejecutar sentencias dentro de `#MW` y `#API` (`let`, `ctx.hdr`, `ctx.set`, `ctx.get`, llamadas a `ExtRegistry`, `? ERR(status, msg)`, `-> NEXT`, `-> RET`).
- [x] **2.2 Servidor HTTP Genérico (`axum` + `tokio`):** Construir un `Router` dinámico a partir de los nodos `#SRV` y `#API` del AST.
- [x] **2.3 Pipeline de Middlewares (`#MW`):** Encadenar los middlewares declarados en cada `#API` antes de ejecutar el handler. Si un middleware devuelve `ERR(code, msg)`, abortar la petición con ese HTTP status.
- [x] **2.4 Serialización Nativa Protobuf + Fallback JSON:**
  - Generar descriptores Protobuf a partir de los nodos `#M` y los bloques `IN`/`OUT` de `#API`.
  - Soportar `Content-Type: application/x-protobuf` (binario) y `application/json` (fallback para debugging) según el header `Accept`/`Content-Type`.
  - Exponer un endpoint automático `GET /__aion/schema.proto` que sirva el `.proto` generado.
- [x] **2.5 Verificación:** Escribir tests de integración usando `tower::ServiceExt` (sin levantar puertos reales) probando:
  1. Petición rechazada por `#MW0` con `401` cuando falta el header `Authorization`.
  2. Petición exitosa a `#API0` en formato JSON y en formato Protobuf binario usando un plugin mock en `ExtRegistry`.

---

## HITO 3: Plugins Externos Reales (`plugins/aion-ext-mongo` y `plugins/aion-ext-firebase`)
**Objetivo:** Reemplazar los mocks por implementaciones reales desacopladas del núcleo.

- [ ] **3.1 Crate `aion-ext-firebase`:**
  - Implementar `ExtPlugin` para `aion/firebase-auth@2.0.0`.
  - Implementar `verify(bearer, project_id)` validando JWT RS256 contra el endpoint JWKS de Google (con caché en memoria de llaves públicas).
  - Incluir un modo de test con llaves RSA locales generadas en memoria para pruebas *offline*.
- [ ] **3.2 Crate `aion-ext-mongo`:**
  - Implementar `ExtPlugin` para `aion/mongo@1.2.0` usando el driver oficial `mongodb` de Rust.
  - Soportar `connect`, `find_one`, `ins`, `upd`, `del` traduciendo `map<str,any>` a BSON dinámicamente.
- [ ] **3.3 Verificación:** Tests unitarios en ambos crates validando su conformidad estricta contra sus respectivos manifiestos JSON ABI.

---

## HITO 4: Motor de Layout, Reactividad y Verificador Visual (`aion-runtime-ui`)
**Objetivo:** Habilitar la capa de interfaz gráfica multiplataforma empezando por el motor headless (layout + estado) antes de la GPU.

- [ ] **4.1 Activar `aion-runtime-ui` en el Workspace.**
- [ ] **4.2 Motor de Reactividad (`Signals`):** Implementar el grafo de dependencias entre nodos `#S` y nodos `#N`. Al mutar `#S0`, marcar como *dirty* únicamente los nodos `#N` suscritos.
- [ ] **4.3 Motor de Layout (`taffy`):** Traducir nodos `#N` (`ROOT`, `FLEX`, `TXT`, `BTN`) a un árbol de layout `taffy` y calcular cajas (`x, y, width, height`) para viewports estándar (Móvil `390x844`, Desktop `1440x900`).
- [ ] **4.4 Verificador de Layout en Tiempo de Compilación (`aion-checker`):**
  - Detectar *overflow* (hijos que exceden el contenedor padre sin scroll) y emitir error JSON `E_UI_OVERFLOW` con `fix_hint`.
- [ ] **4.5 Renderizador GPU (`wgpu` + `cosmic-text`):**
  - Dibujar primitivas básicas (rectángulos con border-radius, colores hex, texto) en una ventana de escritorio nativa (target inicial) y preparar la compilación a WebAssembly (`wasm32-unknown-unknown`).
- [ ] **4.6 Verificación:** Tests unitarios que calculen el layout del ejemplo de `ARCHITECTURE.md` y validen que `E_UI_OVERFLOW` se dispara cuando un botón de `500px` se coloca dentro de un viewport móvil de `390px`.

---

## HITO 5: Puente Full-Stack (`CALL(#API...)`)
**Objetivo:** Conectar la UI con el Backend usando el mismo contrato tipado.

- [ ] **5.1 Cliente HTTP/Protobuf en `aion-runtime-ui`:** Implementar la ejecución de `@tap->CALL(#API0, {...})` serializando el payload `IN` en Protobuf, enviándolo al `#SRV` correspondiente y deserializando `OUT` hacia el estado `#S` asignado.
- [ ] **5.2 Manejo de Estados `@loading` y `@err`:** Conectar los callbacks de error (`TOAST`, mutación de estado) ante fallos de red o respuestas HTTP 4xx/5xx.
- [ ] **5.3 Verificación:** Test *end-to-end* en memoria donde un evento sintético `@tap` en `#N3` dispara una llamada real a `#API0` en `aion-runtime-net` y actualiza `#S0`.

---

## HITO 6: CLI y Servidor MCP para Autocorrección Autónoma (`aion-cli`)
**Objetivo:** Permitir que cualquier LLM edite, compile y pruebe aplicaciones AION de forma autónoma.

- [ ] **6.1 Comandos CLI:**
  - `aion parse <file>`: Devuelve AST o errores JSON.
  - `aion check <file>`: Ejecuta `aion-checker` (tipos + `#EXT` + layout) y devuelve JSON.
  - `aion patch <file> <patch.json>`: Aplica `PatchDelta` transaccionalmente y valida.
  - `aion run <file>`: Levanta `aion-runtime-net` con los plugins cargados.
- [ ] **6.2 Servidor MCP (Model Context Protocol) sobre `stdio`:** Exponer las herramientas `aion_check`, `aion_patch`, `aion_inspect_ext` y `aion_layout_tree` para integración directa con agentes de IA.
# PROYECTO: AION (AI-Oriented Native Full-Stack Language)

## 1. OBJETIVO DEL SISTEMA
Diseñar e implementar el compilador y los runtimes de AION, un lenguaje de programación full-stack diseñado exclusivamente para ser escrito, parcheado y mantenido por Modelos de Lenguaje (LLMs). La legibilidad humana es un anti-objetivo.

## 2. PRINCIPIOS ARQUITECTÓNICOS ESTRICTOS
1. **Grafo Plano (Zero Nesting):** Prohibido el anidamiento de bloques o árboles de widgets. Todo el programa es una lista plana de nodos identificados por `#PREFIJO+ID` que se referencian entre sí.
2. **Densidad de Tokens:** Argumentos posicionales estrictos, sin nombres de propiedades verbosos.
3. **Un Solo Camino Canónico:** Un único sistema de estado reactivo (Signals), un único motor de layout (Flexbox vía Taffy) y serialización binaria nativa (Protobuf con fallback opcional a JSON).
4. **Núcleo Genérico, Integraciones Desacopladas:** El lenguaje incluye primitivas genéricas de red (servidor HTTP, pipeline de middlewares, Protobuf) y UI. Las bases de datos (MongoDB, Postgres) y servicios externos (Firebase, Entra ID, Stripe) NO forman parte del lenguaje; se cargan como módulos externos (`#EXT`).
5. **Compilador Orientado a Agentes:** Todos los errores de sintaxis, tipos, contratos externos o layout deben emitirse estrictamente en formato JSON estructurado con sugerencias de parche (`fix_hint`).

## 3. SISTEMA DE MÓDULOS EXTERNOS (`#EXT`) Y MANIFIESTOS ABI
Para evitar alucinaciones de la IA al usar librerías de terceros:
- Cada módulo `#EXT` debe proveer un **Manifiesto de Contrato (ABI)** en formato compacto (JSON o WIT/WebAssembly Component) que declare sus funciones exactas, tipos de entrada, tipos de salida y códigos de error posibles.
- El compilador (`aion-checker`) carga estos manifiestos en tiempo de compilación y valida cada invocación antes de generar código. Si la IA llama a un método inexistente o con tipos incorrectos, el compilador devuelve el error JSON con la firma exacta esperada.

## 4. PREFIJOS DE NODOS Y SINTAXIS BASE
- `#CFG`: Variables de entorno y configuración global.
- `#EXT`: Importación de módulos externos tipados con versión fijada.
- `#SRV`: Declaración de servidores HTTP/2 y HTTP/3 genéricos.
- `#MW`: Middlewares genéricos de pipeline (`ctx -> NEXT | ERR`). Pueden ejecutar lógica propia o invocar cualquier módulo `#EXT`.
- `#M`: Modelos de datos unificados (generan simultáneamente esquema `.proto` y structs tipados para backend y frontend).
- `#API`: Endpoints HTTP montados sobre un `#SRV`, con middlewares opcionales y contratos estrictos `IN`/`OUT`.
- `#S`: Estado reactivo de frontend (Signals).
- `#N`: Nodos de interfaz gráfica (UI) renderizados en GPU.

### Ejemplo de Programa Full-Stack Desacoplado en AION:
```aion
// Configuración y módulos externos (intercambiables sin tocar el lenguaje)
#CFG:ENV{MONGO_URI, FB_PROJ_ID}
#EXT:db = "aion/mongo@1.2"
#EXT:auth = "aion/firebase-auth@2.0"

// Servidor HTTP genérico con soporte nativo Protobuf
#SRV0:HTTP[port:8080, proto:on]

// Middleware genérico (agnóstico al proveedor)
#MW0:PIPE(ctx) {
  let tok = ctx.hdr("Authorization")
  let usr = auth.verify(tok, CFG.FB_PROJ_ID) ? ERR(401, "invalid_token")
  ctx.set("uid", usr.uid) -> NEXT
}

// Modelo de contrato único (Backend + Protobuf + Frontend)
#M0:User{id:str, uid:str, pts:i32}

// Endpoint REST/Protobuf usando el servidor genérico y el driver externo
#API0:#SRV0.POST["/v1/pts", #MW0] IN{delta:i32} OUT{#M0} {
  db.connect(CFG.MONGO_URI)
  db.upd("users", {uid:ctx.get("uid")}, {$inc:{pts:in.delta}}) -> RET
}

// Frontend Multiplataforma (Web/Android/iOS)
#S0:i32=0
#N0:ROOT[0,0,100%,100%,#F8FAFC]{#N1}
#N1:FLEX[v,c,c,16]{#N2,#N3}
#N2:TXT[24,700,#0F172A]{"Puntos: {#S0}"}
#N3:BTN[160,48,8,#2563EB]{#N4} @tap->CALL(#API0,{delta:10})=>#S0=res.pts @err->TOAST("Error")
#N4:TXT[16,600,#FFFFFF]{"Sumar +10"}
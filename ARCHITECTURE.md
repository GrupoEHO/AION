# Arquitectura Oficial de AION (Modelo de 4 Capas)

AION está diseñado para mantenerse **pequeño, determinista y verificable estáticamente** por compiladores y agentes de IA. Para evitar que el lenguaje crezca como un monolito, toda funcionalidad pertenece estrictamente a una de las siguientes 4 capas:

---

## Capa 1: Lenguaje Base (`aion-core`) — *Congelado y atemporal*

Contiene únicamente la gramática plana, el sistema de tipos y las garantías estáticas del compilador (`aion-ast`, `aion-parser`, `aion-checker`):

- **Grafo plano sin anidación:** `#CFG`, `#EXT`, `#SRV`, `#MW`, `#M`, `#API`, `#S`, `#N` (con IDs semánticos como `#N_inicio`, `#S_total`, `#M_Gasto`, `#API_listar`).
- **Tipos:** `i32`, `f64`, `str`, `bool`, `bytes`, `ts`, `list<T>`, `map<K,V>`, `#M<id>`, `@ExtType`.
- **Expresiones y control de flujo:** operadores aritméticos, lógicos y de comparación, concatenación de texto, guardas condicionales `@evento if <expr> -> <acción>`, `? ERR(...)`, `-> RET / NEXT`.
- **Verificación estática:** tipos estrictos, detección de ciclos (`E_CIRCULAR_REF`), `@err` obligatorio en `CALL`, prevención de desbordamientos con Taffy (`E_UI_OVERFLOW`), formateador canónico (`aion fmt`) y parcheo transaccional (`aion patch --write`).

**Regla:** Ninguna dependencia de proveedor, moda visual ni hardware específico entra en `aion-core`.

---

## Capa 2: Framework Visual Base (`aion-ui`) — *15 primitivas ortogonales*

Conjunto mínimo y componible medido por Taffy y compilado 1:1 a Web/WASM, Android nativo y SwiftUI:

| Categoría | Primitivas | Propósito |
| :--- | :--- | :--- |
| **Estructura y navegación (6)** | `ROOT`, `FLEX`, `LIST`, `ROUTER`, `MODAL`, `NAV` | Contenedores, listas dinámicas `list<#M>`, multipantalla y diálogos |
| **Entrada / Formularios (4)** | `BTN`, `INP`, `SEL`, `SW` | Botones, texto/números, dropdown nativo, switch booleano |
| **Presentación (5)** | `TXT`, `IMG`, `ICON`, `PROG`, `RING` | Texto reactivo, imágenes, iconos vectoriales, barra lineal y anillo circular de progreso |

Widgets compuestos (tarjetas, calendarios, selectores complejos, temas) deben construirse componiendo estas 15 primitivas en librerías externas importables mediante `#USE "ruta/lib.aion"`, sin añadir sintaxis especial al compilador.

> **Regla de congelamiento visual:** Las 15 primitivas de `aion-ui` son el conjunto base cerrado del lenguaje. **Todo widget que no esté entre estas 15 primitivas directas DEBE implementarse como librería externa**, ya sea:
> 1. **Librería externa de UI compuesta (100% AION)** importada con `#USE "ruta/componente.aion"`, o
> 2. **Librería externa nativa (`#EXT`)** declarando widgets en su manifiesto ABI (`widgets`) e invocada como `#N_id:alias.WidgetName[w, h, ...]{}` (verificada estáticamente por `aion_check` y medida por Taffy).
>
> Para **iconos**, `ICON[size, color]{"nombre"}` incluye un catálogo base curado en `aion/std` (~65 iconos generales vectoriales 24×24, estilo Flutter `Icons.*`), y además permite importar o exportar packs externos/propios mediante `#EXT:pack = "org/icons@1.0"` (campo `icons` en el manifiesto ABI) usando `"pack:nombre"`.

---

## Capa 3: Librería Estándar Incluida (`aion/std`) — *Zero-config*

Utilidades transversales incluidas en el binario `aion` con comportamiento uniforme en todas las plataformas:

- **Colecciones:** `PUSH(#S, val)`, `SETAT(#S, idx, val)`, `REMOVE(#S, idx)`, `SUM(#S, "campo"?)`, `COUNT(#S)`.
- **Formato y cálculo:** `FMT_TIME(segundos)`, `FMT_MONEY(monto, simbolo?)`, `ROUND(num, decimales?)`.
- **Identidad y tiempo:** `UID()`, `NOW()`, `@init`, `@tick` / `@tick<ms>`.
- **Estado y feedback:** persistencia local automática de `#S` (`localStorage` / `SharedPreferences` / `UserDefaults`) y `TOAST(msg)`.

---

## Capa 4: Ecosistema Externo (`#EXT` con ABI JSON + WIT) — *Opt-in e intercambiable*

Todo lo que depende de SDKs externos, credenciales, infraestructura o permisos sensibles del sistema operativo se integra mediante `#EXT:alias = "org/pkg@ver"` validado contra un manifiesto ABI:

- **Datos y nube:** `aion/mongo`, `aion/firebase-auth` (incluidos como referencia), `aion/postgres`, `aion/sqlite`, `aion/http`, `aion/storage` (S3/GCS), `aion/pdf`, `aion/stripe`.
- **Dispositivo / Hardware:** `aion/device` (notificaciones programadas en background, audio avanzado, cámara, GPS, biometría, archivos). *(Nota: `BEEP()` y `NOTIFY()` están disponibles hoy en el runtime para prototipado rápido y migrarán progresivamente a `aion/device` con permisos declarativos).*
- **Paquetes visuales de comunidad:** librerías de componentes compuestos (gráficas, mapas, tablas) encima de las 14 primitivas de `aion-ui`.

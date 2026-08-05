# PLAN — EntropIA-Agent: motor de investigación historiográfica

> Documento de diseño consolidado. Recoge el análisis de los proyectos de referencia
> (Chronos, AIstorian, HistAgent/HistBench), la frontera arquitectónica definida con
> EntropIA Lite/Pro, y el plan de fases para implementar un agente de investigación
> largo, multietapa e iterativo sobre el corpus ya procesado de EntropIA.
> Fecha: 2026-08-05. Estado: propuesta (nada del plan implementado todavía).

---

## 1. Contexto y estado actual

`EntropIA-Agent` es un PoC en Rust de motor de investigación historiográfica. Hoy es un
CLI de un solo agente con tool-calling que produce informes sobre el corpus del SOIP
(sindicato de la industria pesquera de Mar del Plata, 1930–1970).

**Estado verificado (2026-08-05):**

- Crate bin-only (`src/main.rs` con `mod`), sin `lib.rs`.
- Módulos: `agente.rs`, `cliente_llm.rs`, `embeddings.rs`, `informe.rs`, `main.rs`,
  `prompts.rs`, `recuperacion.rs`, `repositorio.rs`, `rerank.rs`, `vector.rs`.
- Loop de tool-calling con 5 herramientas: `buscar_fuentes`, `leer_fragmento`,
  `listar_colecciones`, `preguntar_al_investigador`, `guardar_informe`. Tope: 25 pasos.
- RAG híbrido: embeddings BGE-M3 + FTS5/BM25 + fusión RRF + rerank (`cohere/rerank-4-fast`)
  vía OpenRouter. Recuperación por consulta (top-k), sin paginación ni filtros.
- SQLite **solo lectura** sobre la base común de la app: `items`, `collections`,
  `rag_chunks`, `entities`, `triples`, `transcriptions`, `extractions`, `annotations`,
  `assets`, `vec_assets`. El prompt del agente está hardcodeado en `prompts.rs`.
- Informes Markdown en `informes/`. Sin memoria entre sesiones, sin estado persistente,
  sin planificación, sin manejo de trabajos largos.
- 24 tests unitarios (`agente`, `recuperacion`, `repositorio`, `informe`, `vector`).
- **Higiene aplicada (2026-08-05):** `cargo fmt` y `cargo clippy` limpios; se eliminaron
  la descripción heredada de otro proyecto en `Cargo.toml` y la documentación de un
  "cliente simulado" / modo "Mock" que nunca existió (`.env.example`, `main.rs`).

### 1.1 El corpus real (medido, no supuesto)

| Medición (2026-08-05) | Valor |
|---|---|
| Colecciones | **17** — 11 reales, 6 de prueba (`stress222`, `stress-test-01`, `validacioneventos2222`, `nuevo proyecto`, `probar2`, `prueba3`) |
| Items | 2.393 — ~418 en colecciones reales, 1.975 en las de prueba |
| Assets | 2.477 — **270 procesados (10,9 %)**: 12 `transcriptions` + 258 `extractions` |
| Chunks | 1.648 — **266 (16 %) provienen de colecciones de prueba** |
| Items reales sin chunks | **255 de ~418 (61 % invisible al agente)** |
| Entidades | 1.339 (`organization` 392, `place` 311, `person` 256, `misc` 243, `date` 137) |
| Otros | `triples` 700 · `annotations` 3 |

Detalle que condiciona el diseño:

| Colección real | Items | Items sin chunks |
|---|---|---|
| Conflicto SOIP 1965-66 | 148 | 136 |
| SOIP 1965 | 57 | **57 — colección entera invisible** |
| SOIP 1961 | 56 | 30 |
| Volantes, Panfletos, etc. | 100 | 26 |
| Voces | 12 | 5 |
| Resoluciones SOIP | 19 | 1 |

**Consecuencia de diseño.** La fuente dominante de error hoy no es la precisión sino la
**cobertura**. Un Verifier contrasta la afirmación contra la evidencia recuperada: no
puede detectar los 136 documentos que nunca existieron para el agente. Un informe sobre
el Conflicto SOIP 1965-66 saldría `supported` en cada afirmación y sería falso por
omisión. Además, `listar_colecciones` ordena por cantidad de items, de modo que hoy el
agente ve `stress222` y `stress-test-01` como las colecciones #1 y #2 del archivo.

Por eso la cobertura es **precondición de validez** (§6.5) y entra en Fase 0, antes que
cualquier maquinaria epistémica.

---

## 2. Frontera arquitectónica: EntropIA Lite/Pro ↔ EntropIA-Agent

Decisión fundacional del proyecto. **Lite/Pro son dueñas del procesamiento y
enriquecimiento de fuentes; EntropIA-Agent consume lo ya procesado y no duplica nada.**

| Lo escribe Lite/Pro (procesamiento) | Lo consume EntropIA-Agent (solo lectura) |
|---|---|
| `transcriptions` / `extractions` (por asset) | `rag_chunks` — texto transcrito/extraído y chunked, con `source_kind`, offsets de caracteres y `chunking_contract` |
| `entities` (NER + geocoding, con `confidence`, `model_name`) | `entities` — recuperación centrada en entidades |
| `triples` (sujeto → predicado → objeto) | `triples` — traversal de grafo read-only |
| Embeddings de chunk (`rag_chunks.embedding`, bge-m3/1024) | Vectores para búsqueda semántica. **`vec_assets` no se consume**: es asset-level, 270 filas, redundante con `rag_chunks` |
| `items.title` + `items.metadata` (JSON) | Los títulos codifican fechas en varias colecciones (verificado: `65-03-17-a`, `1964-12-23 - AOMA`, `B - 23-07-2011`); metadata JSON solo metadatos de archivo (`originalName`/`originalPath`/`importedAt`) |
| `assets` (path, `page_number`, `parent_asset_id`) | `mostrar_fuente`: navegar al escaneo original para verificación humana |
| `annotations` | (opcional) pistas del investigador sobre pasajes ya marcados |

El esquema ya codifica la frontera: `rag_chunks.source_kind ∈ ('extraction','transcription')`
— el texto que lee el agente es *producto* del procesamiento de Lite/Pro.

**Regla de oro:** el agente nunca escribe en las tablas de procesamiento y nunca ejecuta
layout analysis, OCR, NER, embeddings, chunking ni extracción de tripletes. Si encuentra
un asset sin transcribir, lo reporta como brecha (y lo deriva a Lite/Pro), no lo procesa.
Con 255 items reales sin chunks, esta no es una rama excepcional: es el caso frecuente.

**EntropIA-Agent es un módulo independiente** (crate lib + bin) que se desarrolla y prueba
solo, y que una vez estabilizado se integra como solapa en EntropIA Lite/Pro. El seam de
integración es una **job API** (`start / plan / step / pause / resume / status / events /
artifacts`): hoy la envuelve el CLI; mañana la envuelven commands Tauri con eventos de
progreso. No hay que rediseñar para integrar.

---

## 3. Modos de funcionamiento

### Modo 1 — Agente de Investigación / Generación de Informes

Produce informes de investigación extensos y longitudinales sobre las fuentes de EntropIA.
No es un chat con RAG: es un agente que ejecuta **planes de investigación largos,
iterativos y multietapa**. Capacidades requeridas:

- Formular y ejecutar múltiples consultas sobre la base.
- Recuperar conjuntos potencialmente muy grandes de fuentes.
- Trabajar por etapas, lotes e iteraciones; reformular consultas según resultados previos.
- Búsquedas sucesivas y loops de investigación (log de consultas como memoria de búsqueda).
- Combinar resultados de miles de documentos/assets (dedup y agregación de evidencia).
- Detectar patrones, cambios, continuidades, actores, acontecimientos y relaciones
  a lo largo del tiempo (síntesis longitudinal).
- Mantener trazabilidad afirmación → evidencia → fuente original.
- **Declarar la cobertura real** del recorte consultado (§6.5).
- Construir progresivamente un informe de largo alcance (artefactos parciales).

Ejemplo: "conflictividad obrera a lo largo de un siglo" → división automática en etapas
(por período, actor, territorio, organización, tipo de conflicto), múltiples consultas por
etapa, integración en una interpretación longitudinal coherente.

### Modo 2 — Agente Redactor de Papers

Producción asistida de artículos académicos combinando:

1. informes previos producidos por EntropIA-Agent;
2. evidencia y fuentes primarias de EntropIA;
3. bibliografía académica gestionada por el usuario en **Zotero**;
4. búsquedas bibliográficas ad hoc en fuentes académicas web.

**Separación estricta de provenance en 4 clases** (modelo de datos, §7):

| Clase | Origen | Verificación |
|---|---|---|
| 1 | Fuente primaria de EntropIA | Contra `chunk_id` / item concreto |
| 2 | Resultado/interpretación previa del agente | Contra el informe previo citado |
| 3 | Bibliografía de Zotero | Contra el item de Zotero |
| 4 | Bibliografía/información externa | Contra la cita recuperada |

Cada afirmación del paper queda etiquetada por la clase de sus evidencias (vía
ClaimEvidence → evidence → `sources.kind`); la copia de trabajo renderiza las citas con la
clase visible; el workflow incluye estado de la cuestión, contraste de interpretaciones
y redacción progresiva por secciones.

---

## 4. Marco de referencia analizado

Tres sistemas de referencia (repos + papers) fueron analizados a fondo. De cada uno se
asimilaron ideas de *diseño* (no código, salvo indicación). **El veredicto de cada idea
bajo la frontera está en §10**; acá solo se describe la idea.

### 4.1 Chronos — AI Co-Historian (arXiv 2604.03553 · ai-historian/chronos · PolyForm NC 1.0)

- **Skills con `requires`**: procedimientos de investigación como Markdown con frontmatter
  `name / description / requires`; el campo `requires` encadena skills en pipelines
  ordenados sin orquestador externo.
- **Memoria por fuente** entre sesiones (`memory/`).
- **Workflow de extracción** en 4 pasos: localizar sección → diseñar prompt → batch con
  **aprobación humana + estimación de costo** → merge con `page_id` (provenance).
- **Inspección visual delegada** a un VLM no-agéntico (el agente nunca carga imágenes en
  su contexto).
- **Evaluación**: F1 a nivel de registro con matching húngaro, Levenshtein normalizado
  (τ=0.8), CER matched/corpus, match estricto/relajado por variable.

### 4.2 AIstorian — KG-powered RAG + anti-alucinación (arXiv 2503.11346 · ZJU-DAILY/AIstorian)

- **Chunking por patrón con in-context learning.**
- **Índice KG**: nodos = entidades, aristas = relaciones, cada nodo ligado a su chunk
  fuente; recuperación por nodo + vecinos.
- **Verifier de hechos atómicos**: descompone la generación en oraciones-afirmaciones y
  chequea cada una contra las referencias (Jaccard token-level + soporte LLM), por
  oración, para evitar el efecto cascada.
- **Router + Solvers**: clasifica errores *not-included* vs *not-supported*, con
  tratamientos por tipo: era-conflict, ref-conflict, knowledge-lack, alias-conflict.
- Fine-tuning SFT+StylePO; la idea de *distractor documents* sirve para evaluación.

### 4.3 HistAgent + HistBench (arXiv 2505.20246 · CharlesQ9/HistAgent · Apache-2.0)

- **Manager + especialistas** (smolagents CodeAct): OCR, imagen, literatura académica,
  archivos, audio, traducción, video.
- **Literature Search Agent**: búsqueda priorizada (Google Scholar → Google Books →
  Springer), devuelve **citas textuales exactas + metadatos bibliográficos**.
- **HistBench**: benchmark de razonamiento histórico (414 preguntas, 6 dimensiones,
  3 niveles, 29 idiomas) con protocolo de revisión en 3 etapas (screening → filtro LLM
  que descarta preguntas resolubles sin material → revisión experta).
- **Evaluación**: pass@1/pass@2 con LLM-as-judge (estilo HLE) + validación humana de una
  muestra.

---

## 5. Arquitectura objetivo

```
EntropIA-Agent (crate Rust: lib + bin)
├── src/lib.rs                    ← re-exporta el núcleo (lo que se integra en Tauri)
├── src/dominio/                  ← jobs, stages (DAG), claims, evidence, memories, artifacts
├── src/orquestador.rs            ← Planner: construye y actualiza el plan
├── src/trabajador.rs             ← Workers acotados (un stage = un worker)
├── src/verificador.rs            ← Verifier de dos modos (factual + interpretativo)
├── src/puerta_lectura.rs         ← Gateway read-only (cobertura, filtros, paginado, batches)
├── src/memoria.rs                ← 3 capas de memoria (§7)
├── src/fechas.rs                 ← document_date: reglas del corpus SOIP
├── src/especialistas/            ← zotero.rs, bibliografia_web.rs (solo Modo 2)
├── src/estado.rs                 ← SQLite propia (escritura) + artefactos en disco
├── src/bin/entropia-agent.rs     ← CLI de desarrollo/prueba (delgado)
└── tests/                        ← integración con una copia de la base
```

**Decisiones de fondo:**

- **Rust, sin framework de agentes nuevo.** El loop de tool-calling existente es
  suficiente; un framework externo complicaría la integración Tauri. El sistema de agentes
  se construye como **composiciones de rol** sobre el mismo `ClienteLlmOpenRouter`
  (cada rol = prompt de sistema + set de herramientas + tope de pasos).
- **Núcleo = Orquestador → Workers → Verificador**, con el plan como artefacto persistido
  entre medio. Esto implementa el requisito de "planes de investigación largos y
  multietapa" en lugar de un chat con RAG.
- **Frontera de confianza**: todo contenido recuperado (chunks, OCR, PDFs, bibliografía,
  web) es **dato, nunca instrucción** (§6.10).
- **Convención de nombres (decidida 2026-08-05).** SQL en inglés — tablas, columnas y
  valores de enum — alineado con el esquema del corpus (`rag_chunks`, `items`,
  `collections`). Rust, CLI, prompts y nombres de herramientas en español, alineado con el
  código existente (`Fuente`, `buscar_fuentes`). **Sin mezcla dentro de una misma capa.**
- **Cada tabla nace con su primer escritor** (§7.1). No se crea esquema especulativo:
  para eso están las migraciones incrementales.

---

## 6. Decisiones de diseño por dimensión

### 6.1 Sistema de agentes — jerárquico, 3 roles

| Rol | Qué hace | Contexto |
|---|---|---|
| **Orquestador (planner)** | Descompone la pregunta en un plan multietapa; asigna stages; revisa resultados; reformula consultas; integra síntesis; decide el cierre | Acotado: plan + resúmenes de stages + preguntas abiertas + log de consultas — nunca chunks crudos |
| **Workers** | Un stage = un worker: ejecuta consultas en batch, lee fragmentos, produce una síntesis con evidencia (artefacto) | Acotado: brief del planner + lote de evidencia |
| **Verificador** | **Dos modos**: factual (hechos atómicos + span check + entailment) e interpretativo (suficiencia, cobertura, interpretaciones rivales). Protocolo aislado del productor: solo claim + evidencia, sin síntesis previa, sin conocimiento externo, `buscar_contraevidencia` acotado | Acotado: claim + evidencia (soporte y contraevidencia) |

Los workers son **turnos LLM con toolset propio**, no procesos ni sub-crates.

El Verificador corre con **prompt independiente y contexto restringido a claim +
evidencia** (nunca la síntesis previa, prohibición explícita de conocimiento externo,
span check determinista de las quotes citadas) para evitar auto-confirmación; puede usar
el mismo modelo con otro rol.

**Estados epistémicos — cuatro, uno por acción distinta:**

| Estado | Significado | Acción |
|---|---|---|
| `supported` | La evidencia sostiene la afirmación | Entra al informe |
| `partially_supported` | Sostiene una parte; el resto excede la evidencia | Entra acotada al alcance sostenido |
| `contradicted` | La evidencia contradice la afirmación | Se retira o se reformula |
| `unverifiable` | No hay evidencia accesible suficiente para decidir | **Se eleva al historiador** |

`contested` no es un estado almacenado: se **deriva** de `claim_evidence` cuando una misma
afirmación tiene relaciones `supports` y `contradicts` a la vez. `inconclusive` se colapsa
en `unverifiable`: ambos disparaban exactamente la misma acción (elevar al historiador),
y una distinción sin consecuencia no se persiste. En Modo 2, un paper existente en
OpenAlex/CrossRef sin texto accesible es `unverifiable`, no evidencia.

Cada ejecución queda en `verification_runs` (append-only): el estado del claim es solo la
proyección del último juicio aceptado. La clasificación de error de AIstorian (alias /
era / ref / knowledge-lack) vive como campo `error_kind` de la corrida, con **una sola
ruta de escalación** (`preguntar_al_investigador`); no hay un componente "Solvers".
**Invalidación automática**: cualquier cambio en el texto del claim, en sus evidencias o
en la versión de su source invalida la verificación vigente y la marca obsoleta.

### 6.2 Planificación — plan estructurado y persistido, con loop de reformulación

El orquestador genera un **plan** (JSON, editable, persistido en el job): hipótesis,
etapas, consultas previstas, criterios de cierre. Ejecución cíclica:

```
plan → stage_i → (consultas batch → evidencia → síntesis con evidencia)
      → verificar → revisar plan
                        ↑
      reformular consultas según resultados (log de queries)
```

El **log de consultas** habilita "reformular en función de resultados anteriores": el
orquestador ve precisión/recubrimiento por consulta y genera variantes (ampliar términos,
cambiar período, cambiar actor, filtrar colección).

**Stages en DAG, no en secuencia.** Una investigación real tiene dependencias cruzadas
(cronología + actores + demandas → síntesis). Las dependencias se modelan en una tabla
relacional (`stage_dependencies`: `stage_id` → `depends_on_stage_id`), no en JSON — lo
que simplifica detección de ciclos, invalidación transitiva, consultas SQL y la
visualización futura del DAG. El DAG lo arma el orquestador y **puede revisarse en
ejecución** (un hallazgo tardío reabre un stage anterior).

**Reutilización de stages — snapshots y timestamps, sin hashes de contenido.** Un stage
reutiliza su artefacto si y solo si: (a) `config_snapshot_id` y `corpus_snapshot_id` del
job no cambiaron, y (b) ninguna dependencia directa o transitiva terminó *después* de que
él terminó (`completed_at`). Eso da la misma garantía epistemológica que un
`input_hash`/`output_hash` — no reutilizar resultados vencidos — sin canonicalización de
entradas ni propagación de hashes, para un DAG de 5 a 12 stages que corre una vez.
Marcar un stage como rehecho invalida sus dependientes por traversal de
`stage_dependencies`.

La ordenación topológica con detección de ciclos es **una función**, no un motor: la usan
el DAG de stages y, si algún día existen, las skills encadenadas.

### 6.3 Memoria — 3 capas (decisión tecnológica en §8)

1. **Memoria de trabajo (por job)**: plan, stage actual, evidencia del stage — en el
   estado del job.
2. **Memoria longitudinal (por línea de investigación)**: hallazgos establecidos,
   preguntas abiertas, decisiones — estructurada y consultable por SQL.
3. **Memoria de artefactos**: informes y papers previos indexados, para que el Modo 2 los
   cite como clase 2.

### 6.4 Herramientas

**Modo 1 (investigación)** — sobre las 5 actuales:

- `buscar_fuentes` — ampliada: filtros por colección, item, rango de `document_date`,
  entidad; paginación; modo batch (conjuntos grandes). **Nunca devuelve chunks de
  colecciones excluidas** (§6.5).
- `listar_colecciones` — ampliada: `items`, `items_con_chunks`, `chunks` por colección,
  con las colecciones de prueba fuera. Es la herramienta de cobertura; no se agrega otra.
- `buscar_entidad` — read-only sobre `entities`/`triples`: nodo + vecinos → chunks ligados.
- `leer_fragmento` / `leer_asset` — fragmento completo; texto completo del asset cuando
  un chunk no alcanza.
- `mostrar_fuente` — ruta del asset + página para verificación humana (sin VLM).
- `registrar_evidencia` — append al ledger (interno, usado por workers).
- `verificar_afirmacion` — el Verifier.
- `actualizar_informe` — construcción progresiva **por secciones versionadas**
  (v1→v2→v3 con provenance y diff); regenerar una sección no invalida las demás (§6.7).
- `consultar_memoria` / `registrar_hallazgo` — capa longitudinal.

**Modo 2 (papers)** — suma:

- `consultar_zotero` — API local de Zotero (`localhost:23119`), read-only, con
  degradación elegante si Zotero no está abierto.
- `buscar_bibliografia` — OpenAlex/CrossRef para **metadata estructurada** (Crossref es
  una API de metadata, no de contenido); el full text se recupera desde la fuente
  original (editor, repositorios OA vía DOI/Unpaywall) respetando acceso y licencia;
  si el texto no es accesible, la evidencia queda `unverifiable` (§6.1).
- `leer_informe_previo`, `redactar_seccion` — secciones del paper como artefactos.

### 6.5 Cobertura y búsqueda — de top-k a conjuntos grandes

**Cobertura primero: es precondición de validez, no una métrica de reporte.**

- **Denylist de colecciones.** Las 6 colecciones de prueba (§1.1) se excluyen de
  recuperación y de listados. Configuración explícita y versionada, no heurística sobre
  el nombre. Sin esto, el 16 % de los chunks del corpus es ruido de stress-test y las dos
  colecciones más grandes que ve el agente son basura.
- **Cobertura declarada.** Toda consulta se responde junto con el estado del recorte
  consultado: items totales, items con chunks, items sin procesar. Todo informe abre con
  esa tabla. Con 255 de ~418 items reales sin chunks, un informe que no la declara es
  engañoso aunque cada afirmación esté verificada.
- **Brechas como salida de primera clase.** Los items sin chunks se listan como brecha
  derivable a Lite/Pro (§2), no se omiten en silencio.

**Fechas: dos conceptos separados.**

- **`document_date`** — la fecha del documento. Candidatos en capas, por colección
  (verificado 2026-08-05):

  | Formato | Ejemplo | Colección |
  |---|---|---|
  | `YY-MM-DD[-sufijo]` | `65-03-17-a` | Conflicto SOIP 1965-66 |
  | `YYYY-MM-DD - texto` | `1964-12-23 - AOMA`, `1941-05-01 - Conferencia…` | AOMA, Volantes |
  | `DD-MM-YYYY` | `B - 23-07-2011` | Voces |
  | Parcial con comodín | `1965-00-0x - AOMA` | AOMA |
  | Sin fecha | `IMG_2991` | SOIP 1961 |
  | Numérico que **no** es fecha | `54`, `151` (nº de resolución) | Resoluciones SOIP |

  Capas de respaldo cuando el título no alcanza: año de la colección (`SOIP 1961`);
  carpeta original (`originalPath`, "LC - Huelga SOIP julio 1961"); dateline en el texto
  del asset (regex determinista para formas españolas + anotación del worker al leer).
  Cada candidato lleva `precision` (día/mes/año/ninguna) + `confidence` + `source`. Vive
  en `source_temporal_metadata`, a nivel de **source** y no de evidencia: varias citas del
  mismo documento no repiten la fecha y se conservan varios candidatos (§7.1).
- **Fechas mencionadas** — fechas de *eventos* dentro del documento (timeline). No van
  a evidence: son **claims factuales con dimensión temporal** (p. ej. "la huelga
  comenzó el 17/03/65") que alimentan la cronología longitudinal del Modo 1.
- **Las reglas viven en `src/fechas.rs`**, no en un directorio de perfiles por colección:
  hay un solo corpus. La tabla de arriba es su especificación. Se extrae a configuración
  cuando exista un segundo corpus, no antes.
- **Límite de frontera**: todo esto es interpretación del agente (regex y anotación
  sobre texto ya procesado, lado consulta) — vive en `estado.sqlite`, nunca se escribe
  de vuelta en las tablas de Lite/Pro.

**Recuperación a escala:**

- **Filtrado previo a la semántica**: por colección, por rango de `document_date` y por
  entidad (`organization` 392 / `place` 311 / `person` 256 bien pobladas; `date` 137,
  débil), para que "un siglo de conflictividad" se parta en pasadas por década/actor.
- **Paginar y acumular**: el worker acumula resultados de múltiples consultas en el
  ledger, con dedup y agregación (cientos de chunks redundantes → clusters de evidencia).
- **Traversal de grafo** como pierna complementaria al RAG híbrido (que se mantiene).
- El gateway reemplaza al `cargar_chunks` actual: acceso indexado y paginado en lugar de
  una carga completa del corpus por consulta (§9, Fase 0).

### 6.6 Recuperación de contexto — síntesis jerárquica

Para miles de documentos, el principio es **no inflar contextos**:

```
chunks recuperados → síntesis de stage (con refs de evidencia)
                  → síntesis de tema → informe
```

El worker ve solo su lote; el orquestador ve solo síntesis; el informe se construye por
capas. La trazabilidad no se pierde: cada síntesis arrastra sus `evidence_id`.

### 6.7 Trabajos largos — ciclo de vida del job

- **Seis estados**: `planned → running → paused → awaiting_human → done / failed`.
  El cierre lleva `close_reason` (`completed` / `cancelled` / `budget_exhausted` /
  `blocked`), de modo que agotar presupuesto o cancelar son cierres con motivo y no
  estados propios. `awaiting_human` cubre tanto la aprobación de un gate como la falta de
  información: en ambos casos el job espera a la misma persona por la misma vía.
  Cada aprobación se persiste en `human_decisions` con el alcance y costo mostrados —
  tras un crash queda claro si un batch caro ya fue autorizado.
- **Checkpoint por stage**: antes de avanzar, el artefacto (markdown/JSON) y su fila de
  estado están persistidos; `resume` retoma desde el último stage completo (crash-safe).
- **Idempotencia**: re-ejecutar un stage reutiliza artefactos existentes según la regla de
  snapshots + timestamps de §6.2.
- **Gates humanos**: antes de una pasada masiva o de cerrar el informe, el agente muestra
  alcance, consultas previstas y **costo estimado** y espera aprobación
  (`preguntar_al_investigador`).
- **Dos budgets duros, a nivel de job**: `max_cost` y `max_llm_calls`. Son los que frenan
  el gasto real. El tope por stage es el que ya existe: pasos del loop (`MAX_PASOS`).
  Agotar un budget cierra el job con `close_reason = budget_exhausted`: el orquestador
  reporta lo que tiene, no deriva en silencio.
- **Eventos de progreso**: el CLI imprime stages; la futura solapa los muestra en vivo.
- **Informe por secciones versionadas**: `actualizar_informe` trabaja sobre secciones
  (`section v1→v2→v3`) con provenance y diff por versión; un hallazgo tardío puede
  reabrir y regenerar una sección sin regenerar el informe completo.

### 6.8 Persistencia de estado — SQLite propia de escritura

`estado.sqlite` (separada y escribible, a diferencia del corpus): esquema en §7. El
propio esquema del agente está versionado (`agent_schema_version`) con migraciones
incrementales desde Fase 1 — al integrar en Tauri con investigaciones reales la base no
se recrea. Las migraciones son también el mecanismo que permite que **cada tabla aparezca
en la fase de su primer escritor** en lugar de crear el esquema completo por adelantado.

### 6.9 Reproducibilidad científica (primer nivel)

Cada job congela su configuración de ejecución al iniciar (`jobs.config_snapshot`):

- modelo + proveedor y temperatura;
- versión/hash de todos los prompts de rol (orquestador, workers, verificador);
- parámetros de retrieval (piernas, LEG_K, RERANK_DEPTH, RRF_K, límites, top_n);
- embedding/index y reranker utilizados;
- **denylist de colecciones vigente** (§6.5): sin ella, dos jobs "idénticos" pueden haber
  visto corpus distintos;
- versión de esquema del corpus (`_migrations`) y **snapshot lógico del corpus**
  (`corpus_snapshot_id`: versión de esquema + conteos por tabla + `max(updated_at)` +
  hash agregado de `source_text_hash` de chunks — barato, no fila por fila).

Objetivo: meses después, "¿por qué este informe produjo esta conclusión?" debe poder
responderse reconstruyendo qué sistema vio qué corpus con qué configuración.

Además, cada llamada LLM queda en `llm_calls` (tokens, costo, latencia, reintentos) y el
progreso en `job_events` (append-only) — permite responder "¿por qué este stage costó
4×?" o "¿dónde empezó a reformularse en loop?".

### 6.10 Frontera de confianza — contenido recuperado = datos, no instrucciones

Regla arquitectónica: **todo contenido recuperado (chunks, OCR, PDFs, bibliografía,
páginas web) es dato no confiable, nunca instrucciones**. Un corpus puede contener
"ignore previous instructions..." y no debe tener efecto alguno.

- El gateway entrega fuentes con **delimitación estructurada**: el contenido va en un
  bloque de datos claramente separado del prompt, sin mezclarse con las instrucciones.
- Los prompts de Worker y Verifier **prohíben explícitamente** (regla de sistema)
  obedecer instrucciones presentes en las fuentes.
- En Modo 2 (contenido web) aplica igual: páginas y PDFs externos son datos; solo se
  ejecutan las instrucciones del sistema/orquestador.
- Verificación: tests dedicados con fuentes adversariales (injection en chunk, en
  bibliografía, en web) desde Fase 2.

---

## 7. Modelo de datos

### 7.1 `estado.sqlite` (SQLite propia del agente, escribible)

Núcleo conceptual: **Job → Stage DAG → Claim → ClaimEvidence → Evidence → Source →
SourceVersion**, y en paralelo **Claim → VerificationRuns** (historial append-only).
**Source** es la unidad formal (`kind` = entropia_chunk / agent_report / zotero /
external → clase 1–4) con scope de project/corpus; **SourceVersion** es el snapshot
recuperable de lo que el agente leyó; **Evidence** es la cita concreta dentro de una
Source (quote + offsets exactos + locator); la fecha del documento vive en
`source_temporal_metadata` (varios candidatos); las relaciones de soporte/contradicción
viven en **ClaimEvidence**; Claims lleva `type` y su estado epistémico es **proyección
del último verification run aceptado** (§6.1). El mismo principio se aplica a la
memoria longitudinal (`memory_evidence`).

**Cada tabla nace con su primer escritor.** La columna «Fase» no es informativa: es la
regla. Crear en Fase 1 tablas que recién escribe Fase 2 congela una forma que todavía no
se conoce.

| Tabla | Fase | Contenido |
|---|---|---|
| `jobs` | 1 | modo, pregunta, plan_json, `status` (planned/running/paused/awaiting_human/done/failed) + `close_reason`, costo acumulado, **budgets** (`max_cost`, `max_llm_calls`), **config_snapshot + corpus_snapshot_id** (§6.9), project/corpus |
| `stages` | 1 | nodo del **DAG** (vía `stage_dependencies`), tipo, `status`, `artifact_path`, `checkpoint`, `started_at`/`completed_at` (base de la reutilización, §6.2) |
| `stage_dependencies` | 1 | **junction** stage ↔ stage: `stage_id`, `depends_on_stage_id` (detección de ciclos e invalidación transitiva en SQL) |
| `queries` | 1 | consulta, filtros, resultados, `reformulated_from` (log para el loop) |
| `llm_calls` | 1 | job_id, stage_id, rol, modelo, tokens input/output, costo, latencia, reintentos/error — diagnóstico y atribución de costo |
| `job_events` | 1 | **append-only** de progreso: job_id, stage_id, tipo, payload, timestamp — base del timeline Tauri |
| `human_decisions` | 1 | aprobaciones/gates: job_id, stage_id, alcance y costo mostrados, decisión, timestamp |
| `artifacts` | 1 | tipo (síntesis/informe_parcial/dataset/paper/sección), path, padre, **versión de sección** |
| `memories` + `memory_relations` | 1 | memoria longitudinal + conflict judgment (§7.2) |
| `sources` | 2 | **fuente (identidad estable)**: `kind`, `external_id`, `item_id`/`asset_id`/`chunk_id`, `locator`, project/corpus — la clase 1–4 deriva de `kind` |
| `evidence` | 2 | `source_id`, quote textual, `span_start`/`span_end` + `text_hash` + `quote_normalized_hash` (offsets contra el texto del source), `locator` (página/precisión), confianza |
| `claims` | 2 | afirmación, `type` (factual/interpretive/causal/synthetic), `status` epistémico (supported/partially_supported/contradicted/unverifiable) como **proyección** del último run aceptado |
| `claim_evidence` | 2 | **junction** claim ↔ evidence: `relation` (supports/contradicts/contextualizes/qualifies), `support_strength`. `contested` se deriva de acá |
| `verification_runs` | 2 | **append-only**: claim_id, estado resultante, modelo, prompt_hash, evidencia considerada, contraevidencia, rationale estructurado, `error_kind`, timestamp, aceptado |
| `memory_evidence` | 2 | **junction** memory ↔ evidence (§7.2) |
| `source_temporal_metadata` | 2 | **candidatos de fecha del documento**: source_id, date, precision, confidence, derivation |
| `source_versions` | 3 | **snapshot recuperable por versión**: source_id, `retrieved_at`, `content_hash`, metadata bibliográfica congelada, excerpt/texto recuperado (cuando licencia/acceso lo permitan). Para `entropia_chunk` la versión es `chunk_id` + `corpus_snapshot_id` y no necesita fila: la tabla existe recién cuando entran Zotero y fuentes externas |

El esquema de `estado.sqlite` está versionado (`agent_schema_version`) con migraciones
incrementales desde Fase 1; `jobs`, `sources` y `memories` llevan scope explícito de
project/corpus (no asumir "una base = una investigación").

Los artefactos van a disco (`informes/`, `papers/`, `sintesis/`) en Markdown/JSON,
legibles por el investigador, como hoy.

### 7.2 Tablas de memoria — enfoque de engram, nativo

Se implementa el **modelo de engram** sobre SQLite nativo (§8) con las convenciones de
EntropIA (IDs TEXT, `created_at`/`updated_at` INTEGER epoch, FTS5):

```sql
CREATE TABLE memories (
  id           TEXT PRIMARY KEY,
  title        TEXT NOT NULL,
  type         TEXT NOT NULL CHECK(type IN ('decision','finding','question',
                'hypothesis','interpretation','learning')),
  content      TEXT NOT NULL,              -- What/Why/Where/Learned o contenido de dominio
  project      TEXT NOT NULL,              -- línea de investigación: 'soip-conflictividad'
  topic_key    TEXT,                       -- upsert estable
  session_id   TEXT,
  created_at   INTEGER NOT NULL,
  updated_at   INTEGER NOT NULL
);

-- FTS5 de contenido externo: NO se puebla solo. Sin estos tres triggers el índice
-- queda vacío y toda búsqueda de memoria devuelve cero.
CREATE VIRTUAL TABLE memories_fts USING fts5(title, content, content='memories',
  content_rowid='rowid', tokenize='unicode61 remove_diacritics 1');

CREATE TRIGGER memories_ai AFTER INSERT ON memories BEGIN
  INSERT INTO memories_fts(rowid, title, content) VALUES (new.rowid, new.title, new.content);
END;
CREATE TRIGGER memories_ad AFTER DELETE ON memories BEGIN
  INSERT INTO memories_fts(memories_fts, rowid, title, content)
    VALUES ('delete', old.rowid, old.title, old.content);
END;
CREATE TRIGGER memories_au AFTER UPDATE ON memories BEGIN
  INSERT INTO memories_fts(memories_fts, rowid, title, content)
    VALUES ('delete', old.rowid, old.title, old.content);
  INSERT INTO memories_fts(rowid, title, content) VALUES (new.rowid, new.title, new.content);
END;

-- Un hallazgo puede apoyarse en varias evidencias (mismo principio que claim_evidence);
-- la clase 1-4 deriva de las sources referenciadas, no se duplica en memories.
CREATE TABLE memory_evidence (
  id          TEXT PRIMARY KEY,
  memory_id   TEXT NOT NULL REFERENCES memories(id),
  evidence_id TEXT NOT NULL REFERENCES evidence(id),
  relation    TEXT CHECK(relation IN ('supports','contextualizes','qualifies'))
);

CREATE TABLE memory_relations (
  id              TEXT PRIMARY KEY,
  source_id       TEXT NOT NULL REFERENCES memories(id),
  target_id       TEXT NOT NULL REFERENCES memories(id),
  relation        TEXT CHECK(relation IN ('related','compatible','scoped',
                    'conflicts_with','supersedes','not_conflict')),
  judgment_status TEXT NOT NULL DEFAULT 'pending'
                    CHECK(judgment_status IN ('pending','judged')),
  judged_at       INTEGER
);
```

**Conflict judgment** (la idea más valiosa de engram): al guardar un hallazgo se buscan
candidatos por similitud FTS5 y se superficia la relación en vez de sobrescribir en
silencio. Un hallazgo que contradice uno previo **no se pisa**: se juzga — y el juez puede
ser el historiador (UI en Fase 4) o el agente vía `preguntar_al_investigador`.

`memory_evidence` referencia `evidence`, que nace en Fase 2: hasta entonces la memoria
longitudinal guarda hallazgos con su texto y su `project`, y se liga a evidencia cuando
el ledger existe.

---

## 8. Decisión de tecnología de memoria

Engram (Gentleman-Programming/engram, MIT — binario Go, SQLite + FTS5, MCP stdio, HTTP
API, conflict judgment) fue evaluado como sustrato de la memoria del agente.

**Veredicto: SQLite nativo del agente para todas las capas de producto; engram queda como
memoria de la *sesión de desarrollo*, que es su caso de uso exacto.** Razones:

1. La trazabilidad afirmación → evidencia → chunk es relacional; engram guarda texto libre.
2. Es data de producto: debe ser testeable, versionable y migrable con el resto del
   esquema. Un runtime externo (binario + `serve`, MCP stdio-only) es una dependencia
   inaceptable para un CLI standalone y para un módulo Tauri.
3. El stack ya provee SQLite + FTS5 (mismo patrón que `rag_chunks_fts`).

**Qué se toma de engram:** modelo de observaciones estructuradas, topic keys con upsert,
FTS5 y conflict judgment. **Qué no:** el daemon, el MCP, el TUI y el sync propio. Un
puente HTTP opcional y degradable queda disponible si algún día se quiere compartir la
memoria de investigación con otras herramientas.

---

## 9. Plan de fases

### Fase 0 — Reestructuración del crate + validez del corpus

- **Alcance:**
  - Convertir el crate bin-only en `lib.rs` + `bin/entropia-agent.rs`. El CLI actual pasa
    a ser un harness delgado sobre la lib.
  - **Denylist de colecciones** (§6.5): las 6 colecciones de prueba salen de
    `buscar_fuentes` y de `listar_colecciones`.
  - **Cobertura declarada**: `listar_colecciones` devuelve `items`, `items_con_chunks` y
    `chunks`; todo informe abre con esa tabla.
  - **Sin base de datos, el agente aborta.** Hoy redacta un informe historiográfico
    completo sin una sola fuente y sin avisar.
  - **Los chunks se cargan una vez por proceso**, no por consulta: hoy `cargar_chunks()`
    mueve 7,6 MB y 1.648 decodificaciones `Vec<f32>` en cada `buscar_fuentes`.
  - Deuda menor pendiente: `limite` de `buscar_fuentes` topea en silencio en
    `RERANK_DEPTH`; `fts5_query` descarta tokens de ≤2 chars y pierde `65`/`17` en
    consultas por fecha; `informe::guardar` escribe relativo al CWD y se romperá en Tauri;
    registro español unificado (voseo) en prompts y copy del CLI.
- **Por qué:** habilita el desarrollo aislado (frontera, §2) y la integración Tauri
  (Fase 4); y hace que los informes que el PoC ya produce sean válidos, que es la
  condición para que la maquinaria de las fases siguientes tenga sentido.
- **Criterios de aceptación:** `cargo test` verde; la lib expone los módulos y el bin
  funciona igual; ninguna consulta devuelve chunks de colecciones excluidas; un informe
  sobre "Conflicto SOIP 1965-66" declara que 136 de 148 items no están procesados; sin
  `ENTROPIA_DB_PATH` el agente termina con error en vez de redactar.

### Fase 1 — Estado persistente y trabajos largos

- **Alcance:**
  - `estado.sqlite` con las tablas de Fase 1 (§7.1): `jobs`, `stages`,
    `stage_dependencies`, `queries`, `llm_calls`, `job_events`, `human_decisions`,
    `artifacts`, `memories` + `memory_relations` + FTS5 con sus triggers.
    **Las tablas epistémicas (claims/evidence/sources/verification_runs) no se crean
    todavía**: entran en Fase 2 con su primer escritor.
  - **`agent_schema_version` + migraciones incrementales** desde el primer commit de
    estado.sqlite (la base no se recrea al integrar en Tauri).
  - **Scope project/corpus explícito** en jobs y memories.
  - **Fase 1 mecánica, sin inteligencia LLM nueva**: persistencia + DAG + checkpoints +
    snapshots + ledger de eventos. La complejidad historiográfica entra en Fase 2, una
    vez que el motor de jobs sea confiable.
  - Snapshot de reproducibilidad por job, con la denylist incluida (§6.9).
  - Ciclo de vida del job con checkpoints, `resume` crash-safe y reutilización de stages
    por snapshots + timestamps (§6.2, §6.7).
  - Orden topológico con detección de ciclos (una función, §6.2).
  - Gates humanos + estimación de costo antes de pasadas masivas; budgets `max_cost` y
    `max_llm_calls` por job.
  - Eventos de progreso (CLI).
- **Criterios de aceptación:** un job sobrevive a un kill del proceso y retoma desde el
  último stage completo; el log `queries` persiste; el snapshot de configuración se
  congela al iniciar el job; el DAG rechaza ciclos; `job_events` registra el progreso
  (append-only); una aprobación humana queda persistida en `human_decisions`; agotar
  `max_cost` cierra el job con `close_reason = budget_exhausted` y reporta lo producido;
  la memoria longitudinal guarda y **recupera por FTS5** hallazgos; un hallazgo
  contradictorio superficia un conflicto pendiente.

### Fase 2 — Planificación, workers y verificación (Modo 1 end-to-end)

- **Alcance:**
  - Tablas epistémicas (§7.1, Fase 2) creadas junto con su primer escritor.
  - Orquestador/planner con plan persistido y loop de reformulación (§6.2).
  - Workers acotados con síntesis jerárquica (§6.1, §6.6).
  - Gateway de lectura: filtros, paginación, batches, dedup/agregación, entity traversal,
    reporte de cobertura por recorte (§6.5).
  - **Verifier de dos modos** (factual: span check + entailment; interpretativo:
    suficiencia/cobertura/interpretaciones rivales) con protocolo aislado del productor y
    los cuatro estados epistémicos; `error_kind` y escalación única al historiador (§6.1).
  - Memoria longitudinal operativa: el orquestador consulta/actualiza `memories` y las
    liga a `evidence`.
  - `document_date` en `source_temporal_metadata` según las reglas de `src/fechas.rs`, y
    extracción de fechas mencionadas como claims temporales (timeline) (§6.5).
  - Herramientas del Modo 1 (§6.4): `buscar_fuentes` ampliada, `listar_colecciones`
    ampliada, `buscar_entidad`, `leer_asset`, `mostrar_fuente`, `registrar_evidencia`,
    `verificar_afirmacion`, `actualizar_informe`, `consultar_memoria`,
    `registrar_hallazgo`.
  - Tests adversariales de prompt injection (§6.10).
  - **EntropIA-Bench** arranca en paralelo: primeras preguntas sobre el corpus SOIP.
- **Criterios de aceptación:** una investigación multietapa (p. ej. conflictividad por
  décadas) se planifica, ejecuta por etapas con reformulación de consultas, produce un
  informe con trazabilidad afirmación → evidencia → fuente **y con su cobertura
  declarada**, y las afirmaciones pasan por el Verifier; el informe se construye por
  **secciones versionadas** (v1→v2→v3 con provenance y diff) y un hallazgo tardío
  regenera una sección sin invalidar las demás; un informe previo se retoma consultando
  la memoria longitudinal; una fuente con instrucciones hostiles no altera la conducta
  del agente.

### Fase 3 — Modo 2: Redactor de Papers

- **Alcance:**
  - `source_versions` (§7.1), que recién acá tiene contenido no trivial.
  - `consultar_zotero` (API local 23119, degradación elegante) y `buscar_bibliografia`
    (OpenAlex/CrossRef, patrón Literature Search de HistAgent simplificado).
  - Provenance de 4 clases (§3): `sources.kind` etiquetado y verificado por clase (por
    claim vía ClaimEvidence → evidence).
  - Paper por secciones como artefactos (`redactar_seccion`, `leer_informe_previo`).
  - Workflow: identificar bibliografía → relacionar con resultados sobre fuentes
    primarias → estado de la cuestión → contraste de interpretaciones → redacción
    progresiva.
- **Criterios de aceptación:** un paper de prueba combina informes previos + fuentes
  primarias + items de Zotero + bibliografía web, y cada afirmación está etiquetada con su
  clase de provenance; una referencia sin texto accesible queda `unverifiable` y se eleva;
  sin Zotero abierto, el agente degrada y lo reporta.

### Fase 4 — Integración en EntropIA Lite/Pro

- **Alcance:**
  - La lib como dependencia del backend Tauri; job API expuesta como commands
    (`research_start / research_step / research_pause / research_resume /
    research_status / research_events / research_artifacts`).
  - Nueva solapa con progreso en vivo, consulta de memoria longitudinal y conflict
    judgment resuelto en la UI del desktop.
  - `mostrar_fuente` abriendo el asset real (path + página).
  - **`estado.sqlite` pasa a ser gestionado por el desktop** (archivo separado en el dir
    de datos de la app, decisión §11#1): la frontera read-only se mantiene a nivel de
    archivo — el agente nunca abre el corpus para escribir. El corpus y su allowlist de
    16 tablas de sync quedan intactos. La replicación de la memoria entre dispositivos
    es problema futuro abierto (§11#1).
- **Criterios de aceptación:** una investigación iniciada en la solapa corre con progreso
  en vivo, se puede pausar/reanudar, y la memoria de investigación queda disponible
  localmente.

### Transversal — Evaluación (desde Fase 2)

- **EntropIA-Bench:** banco de 50–100 preguntas ancladas al corpus SOIP (fechas,
  decretos, interpretación) en 3 niveles, con el protocolo de HistBench de 3 etapas
  (screening → filtro LLM que descarta preguntas resolubles sin el material → revisión
  del historiador). Evaluación pass@1/pass@2 con LLM-as-judge + validación humana de una
  muestra. **Las preguntas se anclan solo a items con chunks**: preguntar por material no
  procesado mide la brecha de Lite/Pro, no al agente.
- **Cadena de atribución de fallos**: cobertura del corpus → retrieval recall → evidence
  recall → citation precision → claim support → answer quality. Es la métrica que guía el
  desarrollo, porque distingue "no recuperé el documento" de "razoné mal sobre el que
  recuperé" — y, con 61 % de items sin procesar, de "el documento no existía para mí".
- **Verifier** como control continuo de calidad en cada fase.

---

## 10. Matriz de asimilaciones (veredicto bajo la frontera)

| # | Asimilación | Fuente | Veredicto bajo la frontera |
|---|---|---|---|
| A | Skills con `requires` | Chronos | **Difiere** — el planner LLM ya secuencia stages. Se extraen skills cuando existan ≥3 procedimientos repetidos y estables, no antes |
| B | Memoria entre sesiones | Chronos | **Mantiene** — capa longitudinal, estructurada en `memories` (§7.2) |
| C | Extracción estructurada de eventos | Chronos | **Re-encuadre**: analítica sobre texto ya procesado (`rag_chunks`/`extractions`); no visual desde escaneos |
| D | Evaluación F1/CER con matching húngaro | Chronos | **Descarta por ahora** — mide datasets estructurados, y tras el re-encuadre de C no hay ninguno en el alcance de Fases 1–3. La cadena de atribución de fallos cubre la necesidad real |
| E | VLM delegado sobre escaneos | Chronos | **Descarta el VLM** (procesamiento = Lite/Pro). Queda solo `mostrar_fuente` |
| F | Gates de aprobación + costo | Chronos | **Mantiene** — esencial en trabajos largos (§6.7) |
| G | Extensiones compartibles | Chronos | **Difiere** — depende de A |
| H | Verifier de dos modos (factual + interpretativo) | AIstorian | **Mantiene — central** (trazabilidad y rigor epistémico) |
| I | KG sobre entidades | AIstorian | **Re-encuadre**: consumir `entities`/`triples` read-only; Lite/Pro los construye |
| J | Solvers de errores (alias/era/ref) | AIstorian | **Re-encuadre**: taxonomía `error_kind` en `verification_runs` con una sola ruta de escalación, no un componente propio (§6.1) |
| K | EntropIA-Bench | HistBench | **Mantiene** — transversal, valida ambos modos |
| L | Provenance bibliográfica estructurada | HistAgent | **Mantiene** — con las 4 clases del Modo 2 |
| M | Agentes especializados | HistAgent | **Re-encuadre**: especialistas de investigación (recuperación, síntesis, verificación, bibliografía, redacción), no de procesamiento |

---

## 11. Riesgos y decisiones abiertas

| # | Riesgo / decisión | Mitigación / estado |
|---|---|---|
| 1 | **Destino de las tablas de memoria** — **DECIDIDO (2026-08-05):** archivo SQLite separado (`estado.sqlite`) gestionado por el desktop, no en el corpus DB. Motivo: la allowlist de sync de EntropIA-Cloud (16 tablas, verificada en `src/sync.rs`) no se toca, la frontera read-only se mantiene a nivel de archivo y no hay contención con la base activa. **Replicación entre dispositivos: ABIERTO** — el archivo SQLite vivo como blob puede producir conflictos semánticos entre escritorios en paralelo; la unidad futura debería ser lógica (jobs, claims, memories, manifests) o un journal append-only | Dónde viven: cerrado. Cómo se replican: Fase 4+ |
| 2 | **Cobertura del corpus: 61 % de los items reales sin chunks**, 10,9 % de los assets procesados, 16 % de los chunks provenientes de colecciones de prueba (§1.1). Es la fuente dominante de error y el Verifier no puede detectarla | Fase 0: denylist + cobertura declarada en toda consulta e informe + brechas derivadas a Lite/Pro. La cobertura encabeza la cadena de atribución de fallos |
| 3 | **Fechas heterogéneas por colección** — seis formatos verificados, incluidos `DD-MM-YYYY` (Voces) y numéricos que no son fechas (Resoluciones SOIP); `entities.date` débil (137 filas); las fechas también viven en el texto del asset | §6.5: `document_date` (candidatos por capas, reglas en `src/fechas.rs`) separado de fechas mencionadas (claims temporales), con precision/confidence/source. Si Lite/Pro indexa fechas normalizadas, el gateway las adopta |
| 4 | **Lectura WAL del corpus** — una conexión `SQLITE_OPEN_READ_ONLY` a una base en modo WAL **funciona mientras el desktop está abierto** (reutiliza su `-shm`) y **falla con `SQLITE_CANTOPEN` cuando está cerrado**, porque no puede crear el `-shm`. Es lo contrario de lo que uno supondría | Test de 5 minutos en Fase 0, no supuesto de Fase 4. Fallback: abrir con `immutable=1` sobre una copia, o exigir que la app esté corriendo |
| 5 | **Licencias**: Chronos PolyForm NC 1.0; AIstorian dataset restringido a investigación no comercial; HistAgent Apache-2.0; engram MIT | Asimilar solo *diseño* (salvo HistAgent y engram, reutilizables); no copiar código de Chronos/AIstorian |
| 6 | **Deriva del orquestador en planes largos** (loop de reformulación sin convergencia) | Criterios de cierre explícitos por stage y por job; tope de pasos; budgets `max_cost`/`max_llm_calls`; gates humanos |
| 7 | **Calidad de verificación** (estados epistémicos erróneos, p. ej. `supported` cuando la evidencia no alcanza) | Cuatro estados en vez de seis reducen la ambigüedad del juicio; validación humana de muestra en EntropIA-Bench; calibrar umbrales y protocolo del Verifier |
| 8 | **Acceso a full text académico en Modo 2** — Crossref/OpenAlex dan metadata; el contenido exige la fuente original y respeta licencias | Rutas OA (Unpaywall, repositorios) vía DOI; si el texto no es accesible, la evidencia queda `unverifiable` y se eleva al historiador |
| 9 | **Prompt injection desde el corpus** | Frontera de confianza (§6.10): contenido = datos, delimitación estructurada, prompts que prohíben obedecer fuentes; tests adversariales desde Fase 2 |

Decisiones ya cerradas en el diseño y por lo tanto fuera de este registro: costo de
pasadas batch (gates + budgets, §6.7) y VLM sobre escaneos (fuera de alcance, §2 y §12).

---

## 12. Fuera de alcance (explícito)

- Layout analysis, OCR, NER, embeddings, chunking, indexación y extracción de tripletes:
  **pertenecen a Lite/Pro**.
- VLM sobre escaneos / inspección visual delegada: fuera del agente (Frontera §2).
- Fine-tuning / entrenamiento de modelos (AIstorian SFT/StylePO): sin presupuesto.
- Scraping de Google Scholar / browser agents: se reemplaza por APIs estructuradas
  (OpenAlex/CrossRef) y la API local de Zotero.
- Integración con la sync de EntropIA-Cloud **antes** de la Fase 4.
- No se modifican las tablas de procesamiento del corpus en ninguna fase.
- Hashes de contenido para invalidación incremental de stages: se resuelve con snapshots
  y timestamps (§6.2).
- Perfiles de corpus configurables: hay un solo corpus (§6.5).

---

## 13. Definición de listo (global)

El proyecto se considera completo cuando:

1. EntropIA-Agent es un crate lib+bin con tests que corre sobre una copia de la base,
   sin tocar las tablas de procesamiento, y **ningún informe se emite sin declarar la
   cobertura del recorte consultado** (Fase 0).
2. El Modo 1 ejecuta investigaciones multietapa (DAG de stages) de larga duración con
   checkpoints, reformulación de consultas, trazabilidad afirmación → evidencia → fuente
   (vía ClaimEvidence), claims tipados con los cuatro estados epistémicos, verificación de
   dos modos y snapshot de reproducibilidad por job (Fases 1–2).
3. El Modo 2 produce papers con las 4 clases de provenance y soporte Zotero + bibliografía
   web (Fase 3).
4. El módulo está integrado como solapa en EntropIA Lite/Pro con job API, progreso en
   vivo y memoria de investigación disponible localmente (replicación entre dispositivos:
   problema futuro abierto, §11#1) (Fase 4).
5. EntropIA-Bench mide ambos modos sobre material efectivamente procesado, y la cadena de
   atribución de fallos —empezando por cobertura— guía las iteraciones (transversal).

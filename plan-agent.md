# PLAN-AGENT — Arquitectura de equipo y camino a producción

> Documento de desarrollo de EntropIA-Agent. Continúa `PLAN.md`, que cubre las
> Fases 0 a 4 y está **implementado y verde** (143 tests). Este plan define el
> equipo de agentes, el método de investigación y la integración en producción.
>
> Estado: **propuesta para evaluación**. Fecha: 2026-09-06.

---

## 1. Punto de partida verificado

`PLAN.md` está ejecutado. Verificación sobre el árbol al 2026-09-06 (HEAD `bd854b4`):

| Fase | Alcance | Estado |
|---|---|---|
| 0 | Crate lib+bin, denylist, cobertura declarada, aborto sin base | Implementada |
| 1 | `estado.sqlite`, DAG de stages, checkpoints, budgets, gates, memoria FTS5 | Implementada |
| 2 | Planificación, workers, verificación de dos modos, herramientas Modo 1 | Implementada |
| 3 | Modo 2 — redactor de papers, provenance de 4 clases, Zotero, OpenAlex/CrossRef | Implementada |
| 4 | Job API (`src/api.rs`) como seam de integración | Implementada **como seam** |

`cargo test`: 143 tests, 0 fallos. La deuda menor de Fase 0 que el plan listaba
(`informe::guardar` relativo al CWD, `limite` topando en silencio, tokens de fecha
perdidos en FTS5) está resuelta y verificada en el código.

**Lo que este plan corrige del anterior:** la Fase 4 de `PLAN.md` especifica "nueva
solapa". Esa decisión se reemplaza en §4 por una vista raíz propia, con la
justificación correspondiente.

---

## 2. Marco de referencia evaluado

Cuatro proyectos analizados como fuente de diseño. Ninguno aporta código: tres son
TypeScript/Bun y el cuarto es un CLI de flujo de trabajo. Se asimila **diseño**.

| Proyecto | Veredicto | Qué se toma |
|---|---|---|
| `can1357/oh-my-pi` | **Asimilable** | Roles ruteados por intención; subagente que devuelve un **objeto validado por schema** leído por campo, no prosa; patrón *advisor* |
| `Gentleman-Programming/gentle-ai` | **Asimilable** | La disciplina SDD: cadena de artefactos durables por fase con gate humano entre cada transición |
| `earendil-works/pi` | Descartado | Es un agente único con tools extensibles. No tiene orquestador ni subagentes especializados |
| `deepseek-ai/deepseek-harness` | Descartado por ahora | Developer preview, arquitectura "todo es plugin" sobre Cordis; no documenta orquestación ni multi-agente |

### 2.1 Advertencia sobre la analogía

oh-my-pi y gentle-ai son **agentes de programación**. Sus roles se dividen por costo y
profundidad de razonamiento (`smol`, `slow`, `tiny`) y necesitan aislamiento de
*workspace* —worktrees, clones de FS— porque **escriben archivos y colisionan entre sí**.

EntropIA-Agent lee un corpus en modo solo lectura y no escribe nada sobre él. **No
necesita aislamiento de workspace.** Necesita aislamiento **epistémico**: que el agente
que juzga la evidencia no vea la síntesis de quien la produjo. Eso ya está implementado
en `verificador.rs`. Copiar el mecanismo de worktrees sería importar un costo sin el
problema que lo justifica.

### 2.2 Hallazgo: RDD ya está implementado

El principio de receipt-driven development de gentle-ai es *"trust what the system can
derive, not what the agent says"*. Ese es el problema epistemológico de la historiografía
asistida, y sus mecanismos ya tienen equivalente exacto en el crate:

| RDD (código) | EntropIA-Agent (existente) |
|---|---|
| Candidate congelado en START | Snapshot de reproducibilidad por job (§6.9 de `PLAN.md`) |
| Receipt / lineage append-only | `verification_runs` append-only |
| Lens (perspectiva de revisión acotada) | Verificador de dos modos, contexto aislado del productor |
| Corrección acotada, una sola ronda | Presupuesto de reformulación por stage |
| Acknowledgement token | `human_decisions` persistidas |
| *"Review never governs delivery"* | El Verificador no publica: eleva al historiador |

**Conclusión: no hay que construir RDD.** Está construido. Lo que falta es lo otro.

### 2.3 El hueco real: SDD historiográfico

Lo que gentle-ai tiene y el agente no es la **cadena de artefactos durables con gate
humano entre fase y fase**. Hoy el agente persiste un plan JSON: una sola pieza que el
historiador aprueba o no. La disciplina SDD lo descompone en artefactos separados,
revisables y versionados. Traducido al dominio:

```
pregunta de investigación
  → prospección      ¿qué hay en el corpus sobre esto? ¿qué falta?      [GATE]
  → diseño           hipótesis, corpus acotado, criterios de cierre     [GATE]
  → plan             etapas, consultas previstas, presupuesto           [GATE]
  → ejecución        los asistentes trabajan
  → verificación     claims → estados epistémicos                       [GATE]
  → informe          archivado, con cobertura y provenance declaradas
```

Para un programador esto es ceremonia. Para un historiador es el **cuaderno de campo**:
el registro de qué se buscó, dónde, con qué criterio y qué quedó afuera. Es exactamente
lo que hace auditable una investigación.

---

## 3. El equipo

### 3.1 Qué es un agente en este sistema

**Un agente = un turno LLM con system prompt propio, un subconjunto acotado de
herramientas y su propia ventana de contexto.** No es un proceso, ni un hilo, ni un
sub-crate. Sigue siendo un único proceso Rust, secuencial, con gates humanos.

Esta es la forma que ya tiene el Verificador y la que usa gentle-ai para sus fases
(`sdd-apply`, `sdd-verify`): roles con prompt y herramientas propias, no procesos
paralelos.

**Descartado explícitamente:** subagentes como procesos concurrentes al estilo oh-my-pi.
En un corpus de solo lectura el paralelismo no compra corrección, y compra concurrencia,
backpressure, cancelación y presupuesto por agente. Se reevalúa sólo si la evaluación
(§5, Fase 8) demuestra que la latencia es el cuello de botella.

**Asimilado de oh-my-pi:** cada agente devuelve un **struct Rust tipado**, nunca prosa
que el orquestador tenga que interpretar. El equivalente de su yield validado por schema.
Si un agente no puede llenar su struct, falla explícito; no improvisa una respuesta.

### 3.2 Los seis roles

| Agente | Responsabilidad | Contexto | Devuelve |
|---|---|---|---|
| **Investigador principal** (orquestador) | Descompone la pregunta, asigna etapas, integra síntesis, decide el cierre | Plan + síntesis + preguntas abiertas + log de consultas. **Nunca chunks crudos** | `Plan`, `DecisiónDeCierre` |
| **Asistente de prospección** | Antes de investigar: qué hay y qué falta en el corpus sobre esta pregunta | Pregunta + metadatos de colecciones. **No lee contenido** | `InformeDeCobertura` |
| **Asistente de archivo** | Fuentes primarias: consultas en lote, lectura de fragmentos, síntesis con evidencia | Brief del orquestador + lote de evidencia | `SíntesisConEvidencia` |
| **Asistente de bibliografía** | Zotero + OpenAlex/CrossRef; estado de la cuestión | Brief + resultados bibliográficos | `ReferenciasConProvenance` |
| **Asistente de redacción** | Secciones versionadas con provenance; nunca inventa evidencia | Claims verificados + evidencia. **Sin acceso de búsqueda** | `SecciónVersionada` |
| **Asistente validador** | Verificación factual e interpretativa; cuatro estados epistémicos | Claim + evidencia, **aislado**: sin la síntesis previa, sin conocimiento externo | `VerificationRun` |

### 3.3 El rol agregado: por qué prospección es un agente y no una consulta

De los seis roles de §3.2, cinco corresponden al equipo previsto (investigador principal
más los cuatro asistentes). **Prospección es el rol que este plan agrega**, y es la
propuesta que más justificación necesita.

El hallazgo dominante del proyecto es que **la cobertura del corpus, no la precisión, es
la fuente principal de error**: 61 % de los items sin chunks, 10,9 % de los assets
procesados. Y el Verificador **no puede detectarlo**: verifica lo que se le trae, no lo
que nunca se buscó porque no existía indexado.

La parte determinista ya existe (`listar_colecciones` devuelve `items`,
`items_con_chunks`, `chunks`). Lo que requiere juicio es traducir una pregunta
historiográfica a un recorte del corpus y dictaminar **sobre qué se puede afirmar y sobre
qué no**. Eso es criterio, no una consulta SQL.

Se separa del asistente de archivo por el mismo argumento que aísla al Verificador:
**el agente que busca evidencia no debe ser el que juzga si la evidencia alcanza.** Su
métrica es inversa a la del archivista — mide ausencia, no presencia — y mezclarlos crea
un conflicto de interés estructural.

La prospección es consultiva: cobertura insuficiente produce una **advertencia**, no
un veto. El historiador puede confirmar **continuar de todos modos**; esa decisión y
las brechas quedan persistidas y acompañan el informe. Los trabajos cerrados por el
veto anterior pueden retomarse con la misma confirmación, sin repetir prospección.

### 3.4 Roles evaluados y descartados

- **Cronista / especialista en fechas.** El manejo de fechas es el problema técnico más
  denso del corpus (`src/fechas.rs`, 573 líneas, cinco capas, seis formatos). Pero es
  **determinista**: reglas y regex, no juicio. Sigue siendo una herramienta, no un agente.
- **Advisor / abogado del diablo** (patrón oh-my-pi). Redundante: el Verificador en modo
  interpretativo ya evalúa suficiencia, cobertura e interpretaciones rivales.
- **Archivista de memoria longitudinal.** `memoria.rs` ya existe y el orquestador la
  consulta. Es una herramienta del orquestador, no un rol con criterio propio.

---

## 4. Integración en producción

### 4.1 Decisión: vista raíz propia, no un modo del chat

EntropIA-Agent entra en Lite/Pro como **vista raíz propia**, hermana de Colecciones,
Base de datos, Chat y Configuración. Reemplaza la "nueva solapa" de la Fase 4 de
`PLAN.md` y descarta la alternativa de convertirlo en un segundo modo del chat RAG.

**Argumento decisivo:** una página puede contener un panel conversacional; una
conversación no puede contener una página. La contención es asimétrica.

Razones concretas:

1. **El ciclo de vida no coincide.** El chat RAG es síncrono: pregunta, espera, respuesta
   con fuentes. Un job del agente dura horas, tiene checkpoints y sobrevive a que se mate
   el proceso. `loading: boolean` no representa "voy por el stage 3 de 7, gasté el 40 %
   del presupuesto y necesito aprobación para una pasada masiva".
2. **El artefacto no es un mensaje, es un informe** con secciones versionadas (v1→v2→v3),
   provenance y cobertura declarada. Requiere vista de documento, no scroll de burbujas.
3. **Trabajos concurrentes.** Una investigación pausada al 40 % del presupuesto mientras
   arranca otra no es una conversación archivada.
4. **La forma ya está en el código.** En `navigation.ts`, `collections → collection →
   item` es una jerarquía navegable con breadcrumb — exactamente la forma de
   investigaciones → investigación → claims/informe. `rag-chat` es plano y terminal.
5. **Lite/Pro ya separa efímero de durable.** Colecciones, Base de datos y Configuración
   son páginas porque manejan estado estructurado. El chat es la única parte efímera.

### 4.2 Alcance obligatorio de la integración

**Resolución de JD-002:** la omisión del adaptador desktop es bloqueante para cerrar
la Fase 7 y declarar producción lista, no para comenzar las Fases 5–6 del crate.
La ausencia actual de estas piezas no es un defecto de ejecución de una propuesta:
el defecto del plan era subcontarlas como una extensión de navegación. Se mantiene
la vista raíz propia y se incorpora explícitamente la integración de extremo a extremo.
Esta resolución de alcance no equivale a una aprobación del juicio anterior.

En `apps/desktop/src/lib/navigation.ts`, la unión `View` suma **dos miembros** y
`RootSectionView` incorpora `research`; el detalle conserva su breadcrumb:

```ts
export type View =
  | { name: 'collections' }
  | { name: 'collection'; id: string; collectionName: string }
  | { name: 'item'; /* ... */ }
  | { name: 'db-browser' }
  | { name: 'rag-chat' }
  | { name: 'settings' }
  | { name: 'research' }                                    // nueva raíz
  | { name: 'investigation'; jobId: string; title: string } // detalle con breadcrumb
```

El cambio de tipos es solo una parte. La Fase 7 incluye las siguientes entregas
en EntropIA-Pro-Lite, usando `src/api.rs` de EntropIA-Agent como seam existente:

| Entrega | Alcance y responsabilidad |
|---|---|
| Navegación y vistas | `navigation.ts`, `route-loader.ts`, `App.svelte` y controles de navegación del desktop: acceso a `research`, apertura del detalle `investigation`, breadcrumb y regreso a la lista |
| Adaptador Tauri | Dependencia del crate en el backend, registro de commands y contratos serializables de solicitudes, resultados y errores. Exponer inicio, estado, pasos, pausa/reanudación, cancelación, eventos, artefactos y apertura de fuentes; incorporar listado de investigaciones y resolución de gates de Fase 6 |
| Ejecución y persistencia | El desktop posee `estado.sqlite` y el directorio de artefactos, aplica migraciones y abre el corpus solo lectura. Conduce los pasos fuera del hilo de UI, sin ejecutar agentes en paralelo; una pausa se hace efectiva en el límite de stage y la UI distingue solicitud de pausa de checkpoint alcanzado |
| Progreso y recuperación | La UI proyecta `job_events` y el estado persistido; al volver a la vista o reiniciar la app reconstruye lista, detalle, artefactos y gates pendientes. La vida del job no depende del montaje de la vista |
| Puente RAG | Acción en `RagChatView.svelte` y contrato de transferencia de pregunta, contexto conversacional y referencias disponibles. El backend conserva esa entrada con el job; las respuestas del RAG son contexto, no claims verificados ni evidencia aprobada |

El API Rust actual no es todavía el contrato IPC completo: `research_events`
devuelve cadenas formateadas y no expone una operación de resolución de gates.
La Fase 7 debe completar esa superficie con datos estructurados y errores visibles,
sin reinterpretar texto de presentación en el frontend ni duplicar el motor en TypeScript.

### 4.3 El puente desde el chat

La intuición de "dos modos" se conserva como **gesto**, no como arquitectura: se agrega
al chat RAG la acción **"profundizar con el Agente"**, que transfiere la conversación
actual al flujo de investigación y navega al job creado. Conserva las referencias
disponibles en `sources`; no asume que todos los mensajes contienen fuentes ni omite
los gates de cobertura y diseño por proceder del chat.

La entrada sigue siendo conversacional. El trabajo vive donde puede respirar.

### 4.4 Frontera de datos

Sin cambios respecto de `PLAN.md`: `estado.sqlite` gestionado por el desktop en el
directorio de datos de la app; el corpus se abre solo lectura; la allowlist de 16 tablas
de sync de EntropIA-Cloud no se toca.

**Riesgo #4 de `PLAN.md` (WAL/`SQLITE_CANTOPEN` con la app cerrada): cerrado por
arquitectura.** En producción el agente corre dentro del proceso de Lite/Pro con la app
abierta, de modo que el `-shm` siempre existe. No se implementa el fallback `immutable=1`.

---

## 5. Fases

### Fase 5 — El equipo

- **Alcance:** convertir los workers genéricos en los seis roles de §3.2, cada uno con
  system prompt propio, toolset acotado y contexto propio. Yield tipado por rol (structs
  Rust, sin prosa interpretada). `especialistas/{zotero,bibliografia_web}.rs` pasan de
  funciones sueltas a herramientas **del** asistente de bibliografía. Nuevo agente de
  prospección con su gate de cobertura.
- **No incluye:** concurrencia, procesos, cambios en Lite/Pro.
- **Aceptación:** cada rol falla explícito si no puede llenar su struct; el asistente de
  redacción no tiene herramientas de búsqueda y no puede citar evidencia que no reciba;
  una pregunta sobre material no procesado es **frenada por prospección** antes de gastar
  una sola llamada al modelo; los tests adversariales de injection siguen verdes.

### Fase 6 — El cuaderno de campo

- **Alcance:** la cadena de artefactos de §2.3 como entidades persistidas con gate humano
  entre transiciones. Cada artefacto es versionado, revisable y reversible. La decisión
  del historiador en cada gate queda en `human_decisions`. Las etapas de volumen variable
  (archivo, verificación) avanzan **por lotes acotados con checkpoint persistido**: cada
  lote aceptado es un artefacto inmutable, un fallo conserva la salida cruda y nombra el
  claim y la referencia exactos que lo causaron, y reintentar no repite lotes aceptados.
  El presupuesto es ajustable por el historiador sobre el trabajo existente.
- **Aceptación:** una investigación se puede reconstruir íntegramente desde sus
  artefactos, sin el modelo; rechazar el diseño de investigación en su gate no consume
  presupuesto de ejecución; un cambio en el diseño invalida los artefactos derivados y lo
  declara.

### Fase 7 — Integración desktop y página de investigación

- **Dependencia:** contratos de roles de Fase 5 y artefactos/gates persistidos de Fase 6.
- **Alcance:** todas las entregas de §4.2: adaptador Tauri, gestión local del estado,
  conducción de jobs, navegación y dos vistas, progreso desde `job_events`, gates
  accionables, apertura del asset real (path + página) y puente desde el chat RAG.
- **Aceptación obligatoria en Lite y Pro, con backend real:**
  - Desde la raíz se listan investigaciones persistidas y se crea y abre una nueva;
    el detalle conserva breadcrumb y regreso a la lista.
  - Una investigación avanza sin bloquear la UI. Cambiar de vista no la cancela;
    pausar impide iniciar el siguiente stage y reanudar continúa desde el checkpoint.
  - Cerrar y reabrir la app recupera estado, artefactos y gates desde `estado.sqlite`.
    Un gate pendiente sigue bloqueando ejecución hasta una decisión persistida;
    reiniciar no lo auto-aprueba ni repite stages ya completados.
  - Los eventos reconstruyen el progreso sin duplicarse en el timeline al volver
    a la vista. Los errores del backend son visibles y no se muestran como éxito.
  - La acción del chat conserva pregunta, contexto y referencias disponibles,
    abre la investigación y respeta los mismos gates que el inicio desde la raíz.
  - `mostrar_fuente` abre el asset y página reales; una fuente no disponible se
    informa explícitamente.
- **Prueba de cierre:** recorrido end-to-end de esos escenarios en ambas variantes,
  incluyendo reinicio con gate pendiente. Agregar rutas, compilar o probar solo el
  crate no satisface la aceptación. No se declara producción lista sin esta evidencia.

### Fase 8 — Evaluación

- **Alcance:** EntropIA-Bench de 12 a 50–100 preguntas con el protocolo de tres etapas
  (screening → filtro LLM que descarta lo resoluble sin el material → revisión del
  historiador). Cadena de atribución de fallos: cobertura → retrieval recall → evidence
  recall → citation precision → claim support → answer quality.
- **Aceptación:** la cadena de atribución distingue "no recuperé el documento" de "razoné
  mal sobre el que recuperé" de "el documento no existía para mí".

---

## 6. Riesgos y decisiones abiertas

| # | Riesgo / decisión | Estado |
|---|---|---|
| 1 | **Prospección como agente separado.** Podría ser la primera etapa del asistente de archivo. Se separa por el argumento de conflicto de interés (§3.3), pero es la decisión menos firme de este plan | **Abierta a evaluación** |
| 2 | **Seis roles pueden ser demasiados.** Cada rol es un prompt que mantener y calibrar. Si la evaluación muestra que bibliografía y archivo no divergen en criterio, se fusionan | Se mide en Fase 8 |
| 3 | **Costo del cuaderno de campo.** Cuatro gates pueden volver tediosa una investigación corta. Mitigación prevista: gates que se auto-aprueban bajo umbrales de riesgo, como el `low risk → structural readback` de RDD | Diseño en Fase 6 |
| 4 | **Cobertura del corpus** (61 % de items sin chunks). Sigue siendo la fuente dominante de error y ahora tiene un agente que la declara, pero **no la resuelve**: eso es trabajo de Lite/Pro | Permanente |
| 5 | **Replicación de `estado.sqlite` entre dispositivos** | Abierta (§11#1 de `PLAN.md`) |
| 6 | **Secuencialidad.** Si la latencia resulta inaceptable con seis roles en serie, hay que revisar el descarte de concurrencia de §3.1 | Se mide en Fase 8 |
| 7 | **Alcance de integración Lite/Pro (JD-002).** No es solo navegación: requiere adaptador Tauri, conducción de jobs, persistencia y puente RAG (§4.2) | Alcance resuelto; bloquea cierre de Fase 7 y producción hasta demostrar su aceptación, no Fases 5–6 |

---

## 7. Fuera de alcance

- Procesamiento del corpus (OCR, NER, embeddings, chunking): es de Lite/Pro.
- Concurrencia entre agentes, worktrees, aislamiento de filesystem (§3.1).
- Reemplazar el chat RAG. El RAG se queda como está; el agente es otra cosa.
- Reimplementar RDD: ya está (§2.2).
- Fine-tuning, scraping de Scholar, VLM sobre escaneos.

---

## 8. Definición de listo

1. Los seis roles tienen prompt, herramientas y contexto propios, y devuelven structs
   tipados; ninguno improvisa fuera de su rol.
2. Ninguna investigación arranca sin un informe de cobertura aprobado.
3. Una investigación completa se reconstruye desde sus artefactos sin el modelo.
4. El agente es una vista raíz de Lite/Pro conectada al backend real con progreso en vivo,
   pausa/reanudación, recuperación tras reinicio y gates accionables; el chat RAG puede
   derivarle una conversación. Los escenarios de Fase 7 están demostrados en ambas variantes.
5. EntropIA-Bench mide sobre material efectivamente procesado y la cadena de atribución
   de fallos guía las iteraciones.

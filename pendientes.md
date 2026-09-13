# Pendientes de EntropIA-Agent

Estado verificado el 2026-09-11 sobre `main` (`7a8b336`), sincronizado con `origin/main`.
En EntropIA-Pro-Lite, `feat/investigacion-desktop` ya está mergeada en `main` y no hay
trabajo de Fase 7 sin commitear.

## Resumen y orden sugerido

| # | Pendiente | Tipo | Tamaño | Estado |
|---|---|---|---|---|
| 1 | Límites de recuperación: por búsqueda (promete 100, entrega 16) y por plan (20 fijo) | Defecto de contrato | Chico-mediano | **Resuelto** (`3ac3915`, `2e6f2f5`) |
| 2 | Gates de Fase 6 apagados | Fase del plan | Mediano | **Resuelto** con otro diseño (`bbc5457`, `a244f47`; UI en Pro-Lite `c7b59a3`) |
| 3 | Aceptación de Fase 7 con backend real | Fase del plan | Mediano | Primera pasada en Lite hecha; faltan gates y Pro |
| 4 | El bench mide mal antes de crecer | Fase del plan | Mediano | En curso: banco de 32 (22 miden); profundidad 16, 24 por pierna y fusión (1,1) confirmadas con datos; falta crecer el banco y medir con las consultas del plan |
| 5 | Informe sin regeneración por sección | Brecha funcional | Mediano | Abierto (ya no depende de nada) |
| 6 | Fechas parciales guardadas como `YYYY-00-00` | Deuda latente | Chico | Abierto |
| 7 | La consulta léxica usa solo las primeras 12 palabras | Defecto | Chico | **Resuelto**: palabras vacías y repetidos fuera antes del tope (léxica 0.48 → 0.68, híbrida 0.82 → 0.89) |
| 8 | La fusión RRF desempata al azar | Defecto | Chico | **Resuelto**: orden total (puntaje, mejor puesto de pierna, id) y chunks cargados por id |

**Estado al 2026-09-11 (tarde).**
- #1 y #2 están en `main` de los dos repos y publicados; el pin del motor en Pro-Lite apunta a
  `b53b21f` (`25a8635`, movido por el bot con CI).
- #2 no siguió la propuesta de abajo: quedaron **dos paradas** (la ronda muestra el diseño y
  permite editarlo, y un gate sobre el plan final), «editar es aprobar» y sin auto-aprobación.
  El diseño vigente está en `docs/superpowers/specs/2026-09-11-gates-fase6-design.md`. La UI de
  gates no se restauró: nunca había existido, se construyó.
- #3: los 8 escenarios de la primera pasada funcionan en Lite y quedaron verificados en
  `estado.sqlite` (pausa manual y reinicio sin repetir etapas terminadas). Falta la segunda pasada
  (gate del plan con reinicio, editar búsquedas, aprobar) y la variante Pro.

Orden propuesto para lo que queda: cerrar **#3** (segunda pasada con gates, después Pro);
luego **#4**, que además fija los números hoy provisorios (tope de consultas de
`trayectorias`, profundidad de recuperación, umbrales de una eventual auto-aprobación);
después **#5**. **#6** no tiene consecuencias hoy: se resuelve cuando aparezca el primer
consumidor de esa columna o junto con otra migración del ledger.

---

## 1. Límites de recuperación

### Por búsqueda: `retrieval_limit` promete 1..100 y la recuperación híbrida entrega 16

**Problema.** El contrato del plan acepta hasta 100 resultados por consulta, pero la
pierna híbrida corta en 16 sin avisar. La pierna léxica, en cambio, respeta el límite
**por colección**. Resultado: el mismo plan recupera cantidades distintas según haya o no
embeddings, y el evento registra lo pedido en lugar de lo efectivo.

**Evidencia.**
- `validate_plan` acepta `1..=100` (`src/investigacion.rs:1915-1916`) y los prompts lo
  anuncian (`:896`, `:1226`). El plan de respaldo usa 20 (`:909`).
- `recuperar_en_colecciones` hace `limite.clamp(1, RERANK_DEPTH)` con `RERANK_DEPTH = 16`
  (`src/recuperacion.rs:24`, `:200`): el pipeline no puede devolver más de lo que manda
  al reranker.
- `retrieve_lexico` aplica `LIMIT ?3` por colección (`src/investigacion.rs:1098-1102`).
- El evento `query` guarda `"limit": p.retrieval_limit` (`src/investigacion.rs:1073`).
- `PLAN.md:635-636` ya lo anota como deuda; no hay registro de por qué 16.
- Toda la evidencia recuperada pasa al asistente de archivo en lotes de 48 KB, y cada
  lote es una llamada al modelo (`src/investigacion.rs:1300-1313`). No hay tope total.
- Medido en `entropia.sqlite`: 1648 chunks en 17 colecciones, de ~800 bytes (mediana
  816, máximo 944). En modo léxico, con límite 100 por colección y 17 colecciones, una
  investigación puede recuperar **el corpus entero** (~45 lotes), justo en el modo de
  peor calidad.

**Propuesta.** Que el contrato diga la verdad, sin encarecer el rerank:
1. Techo de **16 por búsqueda**: una sola constante pública (`RERANK_DEPTH`), usada por
   `validate_plan`, por el texto de los prompts («límite 1..16») y por el plan de
   respaldo (que baja de 20 a 16). Es lo que la pierna híbrida ya entrega; 16 chunks con
   sus metadatos ocupan alrededor de medio lote.
2. La pierna léxica aplica el mismo techo **en total**, no por colección, para que ambas
   piernas tengan la misma semántica.
3. El evento `query` registra `limit_effective` además del pedido.

Alternativa descartada por ahora: subir `RERANK_DEPTH`. No alcanza con cambiar la
constante: dos piernas de `LEG_K = 24` (`src/recuperacion.rs:22`) dan como mucho 48
candidatos, así que también habría que subir `LEG_K`, y el reranker procesa más
documentos por búsqueda. No hay evidencia de que 16 sea insuficiente; se revisa con el
bench (#4).

### Cuántas búsquedas por plan

**Problema.** `validate_plan` acepta como mucho 20 búsquedas (`src/investigacion.rs:1913`)
y los prompts lo repiten (`:896`, `:1226`). El número entró con el workflow (`7a8d8fb`)
sin justificación en el commit, en `PLAN.md` ni en `plan-agent.md`: es un tope de
seguridad elegido a ojo. Tiene dos consecuencias:
- **Queda corto para `trayectorias`.** Su prompt pide consultas por cada variante del
  nombre de cada actor, por sus cargos y por sus organizaciones (`src/perfiles.rs:73-74`):
  3 actores × 3 variantes × 2 cargos ya son 18. Una trayectoria se pierde cuando la
  fuente nombra al actor de otra manera, así que el margen falta justo donde más cuenta.
- **El costo de cada búsqueda no se ve.** Cada búsqueda híbrida hace una llamada de
  embeddings y una de rerank. `recuperacion.rs` no las registra ni las descuenta de
  `max_llm_calls`, que es el presupuesto que fija el investigador
  (`src/investigacion.rs:562-565`). Solo se descuentan los lotes de archivo que produce
  la evidencia.

Contexto del corpus: 20 × 16 = 320 chunks, casi el 20 % del corpus. Más búsquedas se
solapan cada vez más con las anteriores (los chunks repetidos se descartan en
`src/investigacion.rs:1074-1077`), así que el rendimiento de cada búsqueda extra baja.

**Propuesta.**
1. El tope de búsquedas pasa a ser un campo del perfil (`src/perfiles.rs`), usado por
   `validate_plan` y por el texto de los prompts. `general` y `cronologia` quedan en 20;
   `trayectorias` sube, con el número concreto definido por el bench (#4).
2. Las llamadas de recuperación (embeddings y rerank) quedan registradas por job, y se
   decide si se descuentan del presupuesto o se informan aparte. Si no, ampliar el tope
   agranda un costo que el investigador no ve.

Es independiente del techo de 16 por búsqueda: uno controla la amplitud (cuántas
preguntas distintas) y el otro la profundidad (cuánto se baja en cada lista).

**Cómo se verifica.** `validate_plan` rechaza un límite de 17. La pierna léxica con 3
colecciones no devuelve más que el techo en total. El evento lleva el límite efectivo.
Un plan de `trayectorias` con más de 20 búsquedas (hasta su tope) se acepta y uno de
`general` con 21 se rechaza. Una investigación registra sus llamadas de recuperación.

---

## 2. Gates de Fase 6 apagados

**Problema.** `plan-agent.md` pide un gate humano entre transiciones del cuaderno de
campo, y que rechazar el diseño no consuma presupuesto de ejecución
(`plan-agent.md:271-283`). Hoy **ninguna etapa abre gate**: el historiador no puede frenar
un diseño malo antes de que se gaste la recuperación.

**Evidencia.**
- La tupla `(kind, output, gate)` de `src/investigacion.rs:858` devuelve `false` en todas
  las etapas (`:870`, `:890`, `:912`, `:915`, `:919`, `:943`, …).
- La maquinaria existe: `require_gates` bloquea `advance` y `resume`
  (`src/investigacion.rs:670-677`, `:848`, `:330`) y `decision` persiste la respuesta.
- Riesgo #3 del plan (`plan-agent.md:326`): cuatro gates vuelven tediosa una
  investigación corta; la mitigación prevista es la auto-aprobación bajo umbral de riesgo.
- En Pro-Lite, el commit `33986f9` quitó la UI de gates porque ninguna etapa los abría.

**Propuesta.**
1. Gate en `design` (paso 1). Es el que cumple la aceptación «rechazar el diseño no
   consume presupuesto».
2. Gate sobre el **plan final**, después de la ronda de clarificación, no sobre el plan
   del paso 2: la ronda replanifica (`src/investigacion.rs:1226`), así que aprobar el plan
   antes de ella es aprobar algo que se va a reescribir.
3. Auto-aprobación bajo umbral (riesgo #3): si la prospección fue suficiente, el plan
   está dentro del presupuesto y no hay huecos declarados, el gate se resuelve solo y
   queda en `human_decisions` con actor `auto` y el criterio aplicado. Así sigue siendo
   auditable y reversible.
4. Restaurar en Pro-Lite la UI de gates que quitó `33986f9`.

**Decisión abierta.** Los umbrales concretos de auto-aprobación. Conviene definirlos
con el bench (#4), no a ojo.

**Cómo se verifica.** Rechazar el diseño deja `llm_calls` de recuperación en cero.
Un gate pendiente bloquea `advance` y `resume`. La auto-aprobación deja una fila en
`human_decisions` con su criterio.

---

## 3. Aceptación de Fase 7 con backend real

**Problema.** La Fase 7 no se puede cerrar con tests: `plan-agent.md:305-307` exige un
recorrido end-to-end en Lite y Pro, «incluyendo reinicio con gate pendiente», y el
riesgo #7 bloquea producción hasta demostrarlo. La sesión anterior probó que hace falta:
dos defectos (`c493931`, `d184e5f`) aparecieron solo desde el desktop, con 204 tests en
verde.

**Evidencia.**
- Escenarios obligatorios en `plan-agent.md:291-304`: listar/crear desde la raíz,
  avance sin bloquear la UI, pausa/reanudación, reinicio que recupera estado y gates sin
  auto-aprobarlos, timeline sin duplicados, puente desde el chat, `mostrar_fuente` con el
  asset real.
- El puente existe: `deepenResearch` en `RagChatView.svelte:111-125`, que llega a
  `ResearchView` vía `research.ts:175-224`.
- El conductor (`src-tauri/src/research.rs:157-189`) solo itera mientras el job está en
  `running`; con `awaiting_human` lo saca del mapa de activos y lo re-agenda si una op
  devuelve `running` (`:219-226`).
- Hay una tensión: la aceptación pide «gates accionables», pero hoy no se abre ninguno
  (#2) y la UI de gates fue retirada.

**Propuesta.** Dividir la aceptación en dos pasadas:
1. **Ahora**, todo lo que no depende de #2. La ronda de clarificación ya estaciona el
   job en `awaiting_human`, así que sirve para probar el reinicio con una decisión
   humana pendiente. Recorrido: `npm run tauri:dev:isolated` en Lite y Pro, y los
   escenarios de arriba.
2. **Después de #2**, repetir solo los escenarios de gates (reinicio con gate de diseño
   pendiente, rechazo, auto-aprobación) y cerrar la fase.

**A verificar en el recorrido.**
- Que el motor persista el `context` del chat con el job (no confirmado en el código).
- Que al reabrir la app los jobs en `running` se retomen solos.

**Cómo se verifica.** Una lista de escenarios con resultado observado por el
historiador, en cada variante. Cada defecto que aparezca se corrige con un test en el
crate que lo reproduzca antes del arreglo.

---

## 4. El bench mide mal antes de crecer

**Problema.** La Fase 8 pide llevar el bench de 12 a 50–100 preguntas
(`plan-agent.md:309-316`). Pero con la medición actual, crecer el banco solo multiplica
números engañosos.

**Evidencia.**
- `recall` devuelve `1.0` cuando no hay chunks esperados (`src/bench.rs:112-114`), y la
  cobertura hace lo mismo sin items esperados (`:78`). Preguntas como `soip-001..003`
  suman recall perfecto sin medir nada.
- `retrieval_recall` mide solo la pierna léxica, con FTS5 top 50 (`src/bench.rs:84`),
  no el recuperador híbrido que usa el workflow, y con un techo distinto al real (#1).
- `claim_support` y `answer_quality` quedan en `None` (`src/bench.rs:107-108`).
- Solo corre dentro de `cargo test`; no hay un runner que produzca un reporte.

**Propuesta.**
1. Sin esperados, la métrica es `None` («no aplica»), no `1.0`, y el agregado la
   excluye.
2. `retrieval_recall` mide con `Recuperador::recuperar_en_colecciones`, con el mismo
   techo que el workflow.
3. Un runner (test `#[ignore]` contra `entropia.sqlite`, como `tests/corpus_real.rs`)
   que emita la cadena de atribución por pregunta en JSON.
4. Recién entonces, crecer el banco con el protocolo de tres etapas del plan.
5. `claim_support` puede salir del ledger: el veredicto de `verification_runs` ya existe.

**Cómo se verifica.** La aceptación del plan: la cadena distingue «no recuperé el
documento», «razoné mal sobre el que recuperé» y «el documento no existía para mí».

---

## 5. El informe no se regenera por sección

**Problema.** Un hallazgo tardío obliga a rehacer el informe completo. `revise` solo
acepta `design` o `plan` e invalida todo lo que sigue, y un job cerrado rechaza
cualquier op.

**Evidencia.**
- El paso 7 produce el informe entero con `informe_render::render`
  (`src/investigacion.rs:946-988`) y cierra el job (`:998-1009`).
- `revise` limita el tipo en `src/investigacion.rs:377`; los jobs cerrados rechazan
  ops en `:318`.
- `informe_secciones` ya resuelve el versionado por sección con diff, pero escribe
  archivos en disco además de filas en `artifacts` (`src/informe_secciones.rs:1-7`,
  `:54-59`). Lo usan `agente.rs:346-375`, con el job fijo en `"cli"`, y `paper.rs`.
  `investigacion.rs` no lo usa.

**Propuesta.** No cablear el módulo de disco en el workflow: la Fase 7 exige que todo se
recupere desde `estado.sqlite`, y los archivos serían una segunda fuente de verdad.
1. El paso 7 persiste cada sección del informe como artefacto `report_section` con la
   misma cadena de versiones y `padre` que el resto (`src/investigacion.rs:639-650`).
   El `report` completo pasa a ser el ensamblado de sus secciones vigentes.
2. Una op nueva, `revise_section { section }`, permitida también sobre jobs cerrados:
   regenera una sección, sube su versión y re-ensambla el informe sin tocar la
   evidencia ni las otras secciones.
3. `informe_secciones` queda para el CLI y `paper.rs`, o se retira si su uso se reduce
   a eso. Eso se decide después.

**Cómo se verifica.** Revisar una sección no cambia el `llm_calls` de las demás. El
informe re-ensamblado difiere solo en esa sección. La versión anterior sigue siendo
legible.

---

## 6. Fechas parciales guardadas como `YYYY-00-00`

**Problema.** Una fecha incompleta («1965», «marzo de 1965») se guarda en el ledger como
texto con forma de fecha completa, rellenando con `00` lo que no se sabe:
`1965-00-00`. Eso no es una fecha válida. **Hoy no rompe nada visible**: es un problema
latente que aparece el día que algo lea esa columna con un parser de fechas.

**Evidencia.**
- `Fecha::iso()` formatea `mes.unwrap_or(0)` y `dia.unwrap_or(0)` (`src/fechas.rs:52-58`).
- El valor se persiste en `source_temporal_metadata.date` (`src/investigacion.rs:1597-1605`),
  una columna `TEXT` que ya tiene al lado `precision` (`day|month|year|none`)
  (`src/estado.rs:239-246`).
- Nadie la consume hoy: citas e informe leen `display` (`src/investigacion.rs:2104`,
  `src/informe_render.rs:179-184`), la única lectura ordena por `rowid`
  (`src/dominio.rs:735`) y el filtro de rango compara tuplas en Rust
  (`src/puerta_lectura.rs:173-174`).
- Comportamiento medido de los parsers habituales:

  | Valor guardado | JavaScript `new Date` | SQLite `date()` |
  |---|---|---|
  | `1965-00-00` | Invalid Date | `NULL` |
  | `1965` | 1965-01-01 | `-4707-04-11` |
  | `1965-03` | 1965-03-01 | `NULL` |

**Alternativa descartada.** Emitir ISO 8601 de precisión reducida (`1965`, `1965-03`).
Parece la corrección obvia, pero SQLite interpreta un número suelto como día juliano y
devuelve una fecha del año -4707 **sin error**: se cambia un fallo visible por uno
silencioso. Tampoco sirve completar con `01-01`: inventa un día que el documento no
dice, y un lector que ignore `precision` lo tomaría por un dato.

**Propuesta.** Guardar la fecha como lo que es: un año seguro y mes/día opcionales. El
tipo en Rust ya tiene esa forma (`Fecha { anio, mes: Option, dia: Option }`,
`src/fechas.rs:44-48`); lo que falla es solo la persistencia.
1. Migración numerada nueva en `src/estado.rs` que agrega `year INTEGER`,
   `month INTEGER NULL` y `day INTEGER NULL` a `source_temporal_metadata`, y las
   completa desde `date` para las filas existentes.
2. `registrar_metadata_temporal` recibe la `Fecha` (o sus tres campos) en lugar del
   texto de `iso()`.
3. `date` queda como columna heredada sin escritores nuevos, o se elimina si la
   migración reconstruye la tabla. `precision` se mantiene: es consistente con qué
   campos están presentes y lleva `none` cuando no hay fecha.

**Cómo se verifica.** Test de la migración sobre una base con filas `1965-00-00`,
`1965-03-00` y `1965-03-12`: quedan como `(1965, NULL, NULL)`, `(1965, 3, NULL)` y
`(1965, 3, 12)`. Una consulta SQL `ORDER BY year, month, day` ordena bien las tres. Los
tests de `fechas.rs` que afirman `"1948-00-00"` (`:499`, `:517`, `:531`) se revisan
según `iso()` se conserve o se retire.

---

## 7. La consulta léxica usa solo las primeras 12 palabras

**Problema.** `fts5_query` arma la consulta FTS5 con los primeros 12 tokens válidos y
descarta el resto, sin filtrar antes las palabras vacías. En una pregunta larga, el
preámbulo consume el cupo y los términos que importan quedan afuera. Afecta a toda la
búsqueda léxica del agente: la pierna léxica del recuperador híbrido, el modo solo léxico
del workflow y la línea base del bench.

**Evidencia.**
- `src/repositorio.rs:410-419`: `.filter(es_token_valido).take(12)`; `es_token_valido`
  (`:423-426`) solo exige más de 2 caracteres, así que «según», «item», «qué» o «colección»
  cuentan como términos.
- Diagnóstico de `soip-006` (2026-09-12): con la redacción original, el preámbulo «Según el
  item «65-03-03» (Conflicto SOIP 1965-66)» consumía 9 de los 12 tokens; «plenario» y
  «ciudad» eran el 13 y el 14. El fragmento esperado no aparecía en la búsqueda léxica ni
  con límite 400. La pregunta se reformuló; el defecto sigue.

**Propuesta.**
1. Filtrar palabras vacías del español (interrogativos, artículos, preposiciones y
   términos del propio sistema como «ítem» o «colección») antes de aplicar el tope.
2. Revisar el tope con el bench: medir léxico e híbrido con 12, 24 y sin tope sobre el
   banco actual, que ya tiene preguntas difíciles para lo léxico (tanda 2).

**Cómo se verifica.** Un test de `fts5_query` con una pregunta cuyo término clave está
después de la palabra 12. La corrida del bench antes y después, comparando la línea base
léxica y el recall híbrido.

---

## 8. La fusión RRF desempata al azar

**Problema.** La recuperación híbrida no es determinista: la misma pregunta puede mandar
candidatos distintos al rerank en dos corridas. Para un motor que congela snapshots y
promete investigaciones reproducibles, es un defecto; también ensucia cualquier medición
del bench.

**Evidencia.**
- `rrf_fuse` (`src/recuperacion.rs:508-518`) acumula puntajes en un `HashMap` y ordena
  solo por puntaje. Los empates quedan en el orden de iteración del `HashMap`, que Rust
  aleatoriza por instancia.
- Los empates son frecuentes: un fragmento que solo trae la pierna vectorial en el puesto
  *r* suma exactamente lo mismo que uno que solo trae la léxica en el mismo puesto.
- Detectado al construir el diagnóstico de profundidad (`07054df`).

**Propuesta.** Orden total: puntaje descendente, luego el mejor puesto en alguna pierna,
luego el id del fragmento.

**Cómo se verifica.** Un test que llama la fusión muchas veces con empates exactos y exige
siempre el mismo orden esperado (falla con el código actual).

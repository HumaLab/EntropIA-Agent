# Pendientes de EntropIA-Agent

Estado verificado el 2026-09-11 sobre `main` (`7a8b336`), sincronizado con `origin/main`.
En EntropIA-Pro-Lite, `feat/investigacion-desktop` ya está mergeada en `main` y no hay
trabajo de Fase 7 sin commitear.

## Resumen y orden sugerido

| # | Pendiente | Tipo | Tamaño | Depende de |
|---|---|---|---|---|
| 1 | `Fecha::iso()` emite `YYYY-00-00` | Defecto | Chico | — |
| 2 | `retrieval_limit` promete 1..100 y entrega 16 | Defecto de contrato | Chico | — |
| 3 | Gates de Fase 6 apagados | Fase del plan | Mediano | — |
| 4 | Aceptación de Fase 7 con backend real | Fase del plan | Mediano | 3 (parcialmente) |
| 5 | El bench mide mal antes de crecer | Fase del plan | Mediano | — |
| 6 | Informe sin regeneración por sección | Brecha funcional | Mediano | 3 |

Orden propuesto: **1 y 2** primero (chicos, independientes, cierran promesas falsas
del contrato); después una **aceptación parcial de Fase 7** (#4, lo que no depende
de gates) para detectar temprano los defectos que solo ve el consumidor; luego **#3**
y el cierre de #4; **#5** y **#6** al final.

---

## 1. `Fecha::iso()` emite fechas ISO inválidas

**Problema.** Cuando la fecha del documento tiene precisión de año o de mes,
`Fecha::iso()` rellena los campos ausentes con `00`: `1965-00-00`. Eso no es ISO 8601
válido y además contradice el campo `precision` que viaja al lado.

**Evidencia.**
- `src/fechas.rs:52-58` formatea `mes.unwrap_or(0)` y `dia.unwrap_or(0)`.
- El valor se persiste en `source_temporal_metadata.date` (`src/investigacion.rs:1597-1605`
  → `src/dominio.rs:720`). La columna es `TEXT` sin `CHECK` (`src/estado.rs:242`).
- El formato recortado correcto ya existe: `segun_precision` arma `YYYY` / `YYYY-MM`
  (`src/investigacion.rs:2072-2077`), y citas e informe leen `display`, no `iso`
  (`src/investigacion.rs:2104`, `src/informe_render.rs:179-184`).
- Ningún SQL ordena ni compara por esa columna (`src/dominio.rs:735` ordena por `rowid`;
  el filtro de rango compara tuplas en Rust en `src/puerta_lectura.rs:173-174`).

**Propuesta.**
1. `iso()` emite precisión reducida ISO 8601 (`1965`, `1965-03`, `1965-03-12`) a partir
   de `mes`/`dia` presentes.
2. `segun_precision` deja de duplicar esa lógica y delega en `iso()`.
3. Migración idempotente al abrir `estado.sqlite` para las filas ya escritas:

   ```sql
   UPDATE source_temporal_metadata SET date = substr(date, 1, 4) WHERE date LIKE '____-00-00';
   UPDATE source_temporal_metadata SET date = substr(date, 1, 7) WHERE date LIKE '____-__-00';
   ```

**Cómo se verifica.** Los tres tests que hoy afirman `"1948-00-00"` (`src/fechas.rs:499,
517, 531`) pasan a afirmar `"1948"` (test en rojo primero). Un test de la migración
sobre una base con filas viejas. Los tests de precisión día no cambian.

---

## 2. `retrieval_limit` promete 1..100 y la recuperación híbrida entrega 16

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

**Propuesta.** Que el contrato diga la verdad, sin encarecer el rerank:
1. Una sola constante pública para el techo (`RERANK_DEPTH`), usada por `validate_plan`,
   por el texto de los prompts y por el plan de respaldo (que baja de 20 a 16).
2. La pierna léxica aplica el mismo techo **en total**, no por colección, para que ambas
   piernas tengan la misma semántica.
3. El evento `query` registra `limit_effective` además del pedido.

Alternativa descartada por ahora: subir `RERANK_DEPTH`. Aumenta costo y latencia del
reranker sin evidencia de que 16 sea insuficiente; se puede revisar con el bench (#5).

**Cómo se verifica.** `validate_plan` rechaza 17. La pierna léxica con 3 colecciones no
devuelve más que el techo. El evento lleva el límite efectivo.

---

## 3. Gates de Fase 6 apagados

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
con el bench (#5), no a ojo.

**Cómo se verifica.** Rechazar el diseño deja `llm_calls` de recuperación en cero.
Un gate pendiente bloquea `advance` y `resume`. La auto-aprobación deja una fila en
`human_decisions` con su criterio.

---

## 4. Aceptación de Fase 7 con backend real

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
  (#3) y la UI de gates fue retirada.

**Propuesta.** Dividir la aceptación en dos pasadas:
1. **Ahora**, todo lo que no depende de #3. La ronda de clarificación ya estaciona el
   job en `awaiting_human`, así que sirve para probar el reinicio con una decisión
   humana pendiente. Recorrido: `npm run tauri:dev:isolated` en Lite y Pro, y los
   escenarios de arriba.
2. **Después de #3**, repetir solo los escenarios de gates (reinicio con gate de diseño
   pendiente, rechazo, auto-aprobación) y cerrar la fase.

**A verificar en el recorrido.**
- Que el motor persista el `context` del chat con el job (no confirmado en el código).
- Que al reabrir la app los jobs en `running` se retomen solos.

**Cómo se verifica.** Una lista de escenarios con resultado observado por el
historiador, en cada variante. Cada defecto que aparezca se corrige con un test en el
crate que lo reproduzca antes del arreglo.

---

## 5. El bench mide mal antes de crecer

**Problema.** La Fase 8 pide llevar el bench de 12 a 50–100 preguntas
(`plan-agent.md:309-316`). Pero con la medición actual, crecer el banco solo multiplica
números engañosos.

**Evidencia.**
- `recall` devuelve `1.0` cuando no hay chunks esperados (`src/bench.rs:112-114`), y la
  cobertura hace lo mismo sin items esperados (`:78`). Preguntas como `soip-001..003`
  suman recall perfecto sin medir nada.
- `retrieval_recall` mide solo la pierna léxica, con FTS5 top 50 (`src/bench.rs:84`),
  no el recuperador híbrido que usa el workflow, y con un techo distinto al real (#2).
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

## 6. El informe no se regenera por sección

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

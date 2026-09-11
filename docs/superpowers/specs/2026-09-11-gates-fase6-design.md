# Gates de Fase 6: diseño editable en la ronda y gate del plan final

Fecha: 2026-09-11. Estado: aprobado en conversación, pendiente de revisión escrita.
Resuelve el pendiente #2 de `pendientes.md` y desbloquea la segunda pasada de la
aceptación de Fase 7.

## Problema

`plan-agent.md` (Fase 6, líneas 271-283) pide gates humanos entre transiciones del
cuaderno de campo, y que rechazar el diseño no consuma presupuesto de ejecución. Hoy
ninguna etapa abre gate: todas devuelven `gate = false` en `src/investigacion.rs`
(`:870`, `:890`, `:913`, `:916`, `:920`, `:940`, `:944`, `:989`). La única parada humana
es la ronda de preguntas (paso 3), que `decision` no puede cerrar (`:339-341`).

La maquinaria ya existe y funciona:

- `artifact(..., gate = true)` inserta una fila `pending` en `human_decisions`
  (`:651-653`) y deja el job en `awaiting_human` (`:997-1006`).
- `require_gates` bloquea `advance` antes de cualquier llamada al modelo (`:670-677`,
  `:848`) y también `resume` (`:330`).
- El snapshot de `procesar` ya expone `gates` (`:702-716`).

El desktop nunca tuvo interfaz de gates. El commit `33986f9` de Pro-Lite quitó funciones
y claves de i18n, pero no había markup. Hay que construirla.

## Decisiones

1. **Dos paradas humanas, no cuatro.** El plan original (§2.3) pone gates después de
   prospección, diseño, plan y verificación, y el riesgo #3 advierte que eso vuelve
   tediosa una investigación corta. Se eligen dos paradas, ambas antes de gastar
   presupuesto de búsqueda:
   1. **La ronda de preguntas**, que ahora muestra el diseño y permite editarlo.
      Editar el diseño cumple la función de rechazarlo.
   2. **Un gate sobre el plan final**, después de la ronda y antes de ir al corpus.
2. **Sin auto-aprobación por ahora.** Con una sola parada nueva, la mitigación del
   riesgo #3 no hace falta. Se revisa cuando el bench (#4) dé datos.
3. **Editar es aprobar.** Un artefacto editado por el historiador no deja un gate
   pendiente: su edición es la decisión.
4. **Sin «Rechazar» en la interfaz.** El gate ofrece «Aprobar y buscar» y «Editar
   búsquedas». Rechazar sin corregir no lleva a ningún estado útil, y para abandonar ya
   existe «Cancelar». La op `decision` con `approve: false` sigue disponible en el motor.

## Flujo

```
crear → prospección → diseño → plan → RONDA (preguntas + diseño editable)
      → replan → GATE DEL PLAN (búsquedas a la vista) → archivo → … → informe
```

## Motor (EntropIA-Agent)

1. **`answer` acepta `design` opcional.** Si viene, se valida con `validate_design`; si
   es inválido, `answer` devuelve error y la ronda sigue abierta sin registrar
   respuestas. Si es válido, se escribe como versión nueva del artefacto `design`
   (con `padre`) y se emite el evento `design_edited`. El replan recibe ese diseño junto
   con las respuestas. Sin `design`, la ronda funciona como hoy.
2. **Gate sobre el plan final.** Cuando el paso 3 cierra (haya replan o el plan quede
   igual porque las respuestas no lo cambiaron), el plan vigente queda con un gate
   pendiente y el job pasa a `awaiting_human`. El paso siguiente es el archivo (paso 4),
   que no corre hasta la decisión.
3. **`decision` con aprobar**: sin cambios. El job pasa a `running` y el conductor del
   desktop lo retoma.
4. **`revise` cambia de semántica: editar es aprobar.**
   - El artefacto editado se escribe sin gate pendiente y el job queda en `running`.
   - **Plan editado:** la ejecución retoma en el archivo (paso 4) con las búsquedas
     editadas. La ronda no se reabre. Se invalida solo lo posterior al gate (archivo,
     bibliografía, verificación, informe y sus checkpoints). La ronda respondida y el
     artefacto `clarification` se **conservan**: hoy `revise` los invalida porque en
     `KINDS` vienen después del plan (`src/investigacion.rs:22-33`, `:381-384`), y eso
     borraría las respuestas y el encuadre que el informe imprime.
   - **Diseño editado fuera de la ronda:** se rehace el plan (paso 2) y la ronda se
     vuelve a preguntar, porque las preguntas dependen del diseño. Termina de nuevo en
     el gate del plan.
   - Las validaciones actuales se mantienen: tope de consultas del perfil y
     `retrieval_limit` 1..16.
5. **Presupuesto.** Un gate pendiente frena el job antes de cualquier llamada. La ronda y
   el gate están antes de la recuperación, así que corregir diseño o búsquedas no
   consume presupuesto de búsqueda ni de lectura.
6. **Compatibilidad.** Las investigaciones que ya pasaron el paso 3 siguen sin gate. Las
   que están estacionadas en la ronda pasan por el gate al responderla.

En el gate se editan solo `queries` y `bibliography_queries`. El `retrieval_limit` se
conserva del plan vigente.

## Desktop (EntropIA-Pro-Lite)

1. **La ronda muestra el diseño.** Bloque «Diseño de la investigación» (hipótesis,
   alcance, criterios de cierre) de solo lectura, con «Editar diseño» que lo vuelve
   editable (criterios, uno por línea). «Responder y seguir» manda las respuestas y, si
   se editó, el diseño. Campos vacíos: error antes de enviar.
2. **Panel del gate del plan.** Visible con un gate `pending` de tipo `plan`. Título
   «Búsquedas antes de ir al corpus». Lista numerada de búsquedas y, si hay, de
   consultas bibliográficas. Aviso cuando las respuestas no cambiaron el plan (el motor
   ya lo registra desde `cbffd26`).
   - «Aprobar y buscar» → `decision { job_id, gate_id, approve: true }`.
   - «Editar búsquedas» → dos cuadros de texto, una línea por consulta. «Guardar y
     buscar» → `revise { job_id, artifact_id, content }` con `retrieval_limit` del plan
     vigente. Si el motor rechaza (por ejemplo, tope de consultas), se muestra su
     mensaje; la interfaz no conoce el tope.
3. **Cliente y adaptador.** `research.ts` suma `researchDecision`, `researchRevise`,
   `design` opcional en `researchAnswer` y un helper `pendingPlanGate()`. El adaptador
   Tauri (`src-tauri/src/research.rs`) tiene que dejar pasar `decision` y `revise`; el
   conductor ya re-agenda cualquier op que devuelva `running` (`:218-226`).
4. **i18n.** Se reusan `investigation.approve`, `investigation.revise`,
   `investigation.saveRevision` y se agregan las claves nuevas en español e inglés.
5. **Sin cambios:** la lista ya muestra «Esperando revisión humana», y al arrancar solo
   se pausan los jobs `running` (`research.rs:49-56`), así que un gate pendiente
   sobrevive al reinicio.

## Tests

Todos por las interfaces públicas, rojo antes que verde, una porción por vez.

**Motor** (`tests/investigacion.rs`, vía `procesar`):

1. Al cerrar la ronda, el job queda `awaiting_human` con un gate `pending` de tipo
   `plan`; `advance` no avanza y no hay llamadas de `asistente_archivo`.
2. Aprobar el gate → el job sigue y cierra con informe.
3. `revise` del plan en el gate → los eventos `query` usan las búsquedas editadas, no
   queda gate pendiente, la ronda no se reabre y el informe conserva el encuadre de la
   ronda respondida. Reemplaza a `revisar_el_plan_reabre_la_ronda_de_preguntas`.
4. `answer` con diseño editado → el replan recibe ese diseño y existe una versión nueva
   del artefacto `design`; un diseño con campos vacíos se rechaza y la ronda sigue
   abierta.
5. Respuestas que no cambian el plan → igual se abre el gate.
6. `revise` del diseño fuera de la ronda → se rehace el plan, se repite la ronda y el job
   termina en el gate del plan.

Se ajustan los tests y helpers que corren un ciclo completo (`correr`, `correr_con`, el
ciclo de `src/api.rs`, `tests/corpus_real.rs`) para aprobar el gate, y cualquier test que
afirme que `revise` deja un gate pendiente.

**Desktop** (`InvestigationView.test.ts`, vitest):

1. La ronda muestra el diseño; editarlo y responder manda `answer` con `design`.
2. El panel del gate lista las búsquedas; «Aprobar y buscar» manda `decision`.
3. «Guardar y buscar» manda `revise` con las búsquedas editadas; un error del motor se
   muestra.

**Verificación:** motor con `cargo test`, `cargo clippy --all-targets -- -D warnings` y
`cargo fmt --check`; desktop con `cargo check`, `svelte-check`, `eslint` y `vitest`.

**Aceptación manual (segunda pasada de Fase 7):** reiniciar la app con el gate pendiente
(sigue bloqueado y no se auto-aprueba), aprobar, y editar búsquedas en el gate.

## Entrega

- Motor: rama `feat/gates-fase6`, desde `fix/techo-recuperacion`.
- Desktop: rama `feat/gates-plan` en Pro-Lite, desde `main`.
- Un commit por unidad de trabajo con sus tests. Sin push.

## Fuera de alcance

- Auto-aprobación bajo umbral (riesgo #3): se revisa con datos del bench.
- Gates en prospección y verificación.
- Selector de modalidad en el desktop.

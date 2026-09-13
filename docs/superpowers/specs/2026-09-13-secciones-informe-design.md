# Secciones del informe: editar a mano y reescribir con indicación

Fecha: 2026-09-13. Estado: aprobado en conversación. Resuelve el pendiente #5 de
`pendientes.md` (opciones A y B; la C queda fuera).

## Problema

Un hallazgo o una corrección tardía obliga hoy a rehacer el informe entero, y en una
investigación cerrada ni siquiera eso es posible. El paso 7 redacta el informe con una
sola llamada al rol `asistente_redaccion` (`src/investigacion.rs:992-1035`); después el
job pasa a `done` y el guard de `:318` rechaza toda operación que modifique (`revise`,
`decision`, `answer`, …).

Lo que ya existe y se reusa:

- Todo lo que el redactor usó sigue disponible después del cierre vía `current()`: las
  afirmaciones verificadas (`archive` + `verification`), cobertura, bibliografía, encuadre
  de la ronda y perfil (`:994-1016`). Reescribir una sección con la misma evidencia no
  exige volver a verificar.
- `sanitize_report` (`:2061-2094`) descarta secciones con `claim_ids` fuera del conjunto
  verificado; `cite_report` (`:2105-2154`) arma las citas; `informe_render::render`
  produce el markdown. Ambos son deterministas y sin modelo.

Lo que se interpone:

1. El guard de investigaciones cerradas (`:318`).
2. La numeración de las citas es **global** en orden de aparición (`:2113`, `:2157-2171`):
   cambiar las citas de una sección renumera las siguientes. Rearmar = volver a correr
   `cite_report` y `render` sobre el informe completo.
3. Las secciones no tienen identidad (`Section`, `:167-179`).
4. El desktop elige el informe con `find(kind === 'report' && !obsolete)` sobre artefactos
   ordenados por versión ascendente (`InvestigationView.svelte:252-254`, `:400`, `:413`):
   con más de una versión muestra la **primera**.

## Decisiones

1. **A y B ahora, C después.** A: el historiador edita el texto a mano. B: pide al
   redactor que reescriba la sección con una indicación, usando la misma evidencia
   verificada. C (sumar evidencia nueva) cambia qué está verificado y merece su propio
   diseño.
2. **Versiones del informe completo, no artefactos por sección.** Cada edición escribe una
   versión nueva del artefacto `report` (con `padre` en la anterior) en la que cambia solo
   la sección tocada. Motivos: el desktop ya lee un único `report`; la numeración global
   obliga a rearmar el informe igual; dos fuentes de verdad (secciones sueltas e informe)
   se desincronizan. El historial de una sección sale de las versiones del informe.
   `informe_secciones` no se cablea al workflow.
3. **Presupuesto de B: cuenta pero no frena.** La llamada se registra en `llm_calls` con su
   costo, como cualquier otra de la investigación, pero no se bloquea aunque
   `max_llm_calls` o `max_cost` estén agotados: el presupuesto frena el gasto automático
   del agente, no un pedido explícito del historiador. La llamada queda marcada como
   pedida por el historiador.

## Motor (EntropIA-Agent)

1. **Identidad de sección.** `Section` suma `id` (`s1`, `s2`, … asignados al redactar,
   después de `sanitize_report`), `version` (1 al redactar) y `origen`
   (`redactor` | `historiador`), más `indicacion` opcional (la de B). Un informe anterior
   sin ids se numera por orden al leerlo, sin reescribirlo.
2. **`edit_section { job_id, section_id, title?, text }`** (A). Sin modelo. Valida que la
   sección exista y que `text` no esté vacío. Conserva los `claim_ids` de la sección.
   `origen = historiador`, `version + 1`. El render marca la sección: «Sección editada por
   el historiador: el texto no pasó por la verificación».
3. **`rewrite_section { job_id, section_id, instruction }`** (B). Una llamada a
   `asistente_redaccion` con contrato de sección `{title, text, claim_ids}`; entrada: las
   afirmaciones verificadas (el mismo conjunto del paso 7), la sección actual, la
   indicación, los títulos de las otras secciones (para no repetir), perfil y encuadre.
   Se valida como `sanitize_report`: `claim_ids` dentro del conjunto verificado y texto no
   vacío. Si no pasa, error y el informe queda como estaba (la llamada ya quedó
   registrada). Si pasa: `origen = redactor`, `version + 1`, `indicacion` guardada.
4. **Rearmado.** Tras A o B: `cite_report` + `render` sobre el informe completo, versión
   nueva del artefacto `report` con el mismo contenido restante, reescritura de
   `report.json` y `report.md` en disco, y evento `section_edited` o `section_rewritten`
   con `section_id`, versión y (en B) indicación y si la llamada quedó fuera del
   presupuesto.
5. **Investigaciones cerradas.** `edit_section` y `rewrite_section` quedan exentas del
   guard de `:318`, solo si la investigación tiene informe (`close_reason = completed`).
   Todas las demás operaciones siguen bloqueadas.

## Desktop (EntropIA-Pro-Lite)

1. El informe mostrado es la **última** versión no obsoleta.
2. Por sección: «Editar» (título y texto editables → «Guardar» → `edit_section`) y
   «Reescribir» (campo de indicación → `rewrite_section`, con indicador de espera). Los
   errores del motor se muestran en la sección.
3. Una sección editada a mano muestra «Editada por el historiador»; una reescrita muestra
   la indicación que la originó.
4. `research.ts` suma los tipos y funciones `researchEditSection` y `researchRewriteSection`
   y las ops en la unión.

## Tests

Por las interfaces públicas, rojo antes que verde, una porción por vez.

**Motor** (`tests/investigacion.rs`, vía `procesar`):

1. Al redactar, las secciones tienen `id`, `version = 1` y `origen = redactor`.
2. `edit_section` sobre una investigación cerrada crea una versión nueva del informe: cambia
   solo esa sección (texto, `origen`, `version`), el markdown trae el aviso, las demás
   secciones no cambian y la numeración de citas se rearma.
3. `edit_section` rechaza una sección inexistente, un texto vacío y una investigación sin
   informe.
4. `rewrite_section` hace una sola llamada al redactor con la indicación y crea una versión
   nueva; con `claim_ids` fuera de lo verificado se rechaza y el informe queda igual.
5. Con el presupuesto agotado, `rewrite_section` igual corre y la llamada queda registrada.
6. Un informe sin ids (anterior al cambio) admite `edit_section` por id asignado por orden.
7. En una investigación cerrada, las demás operaciones siguen rechazadas.

**Desktop** (`InvestigationView.test.ts`): muestra la última versión del informe;
«Guardar» manda `edit_section`; «Reescribir» manda `rewrite_section` con la indicación y
un error del motor se ve.

**Verificación:** motor con `cargo test`, `cargo clippy --all-targets -- -D warnings` y
`cargo fmt --check`; desktop con `cargo check`, `svelte-check`, `eslint` y `vitest`.

## Entrega

Todo en `main` de los dos repos, sin push. En Pro-Lite se commitea con
`git commit -- <archivos>`, porque la carpeta la comparte otra sesión.

## Fuera de alcance

- Opción C: sumar evidencia nueva a una sección.
- La sección «Limitaciones» puede salir duplicada: el prompt de `:1015` pide una al modelo
  y `informe_render` arma otra (`:211-264`). Se registra como pendiente aparte.
- `informe_secciones` escribe filas `seccion` sin `content_json`, que romperían `get` si
  se mezclaran con un job de investigación; no se toca aquí.

# Parámetros del agente

Todos los parámetros del motor de investigación se configuran en un solo lugar:
`ParametrosAgente` (`src/parametros.rs`). El desktop los edita en la solapa
«Agente» de Configuración y los manda al crear la investigación, en el campo
opcional `parameters`. El motor los valida, los **congela con el job** y los usa
en cada paso; el snapshot los expone en `job.parameters` para que la interfaz
muestre con qué valores corrió.

Un job sin `parameters` —los creados antes de existir esta estructura— corre con
los defaults, que son exactamente los valores que estaban hardcodeados. Por eso
`procesar_con` conserva su firma: los parámetros viajan en el pedido de `create`
y se leen del job, no por un argumento nuevo.

## Contrato JSON

`parameters` es un objeto JSON **opcional** en el pedido de `create`. Se puede
mandar completo, parcial o no mandarlo:

- ausente o `null` → todos los defaults;
- objeto parcial → lo que falta queda en su default (no en cero ni en nulo);
- `{}` → todos los defaults;
- un campo que no existe en el contrato se rechaza (no se ignora en silencio).

Forma completa, con los valores por defecto:

```json
{
  "op": "create",
  "question": "¿Qué pasó con la huelga de 1965?",
  "project": "historia-soip",
  "collection_ids": ["c-conflicto"],
  "max_llm_calls": 40,
  "parameters": {
    "max_tokens_salida": 16384,
    "temperatura": 0.3,
    "timeout_s": 180,
    "esperas_reintento_s": [2, 6],
    "lotes_archivo_bytes": [48000, 24000, 12000],
    "fragmento_archivo_bytes": 8000,
    "ventana_cita": 600,
    "preguntas_minimas": 4,
    "rerank_depth": 16,
    "leg_k": 24,
    "rrf_k": 60,
    "snippet_max": 1600,
    "max_consultas": {
      "general": 20,
      "trayectorias": 30,
      "cronologia": 20
    }
  }
}
```

`max_llm_calls` sigue siendo obligatorio y `max_cost` opcional: son el
presupuesto del job, no parámetros del agente, y no viajan en `parameters`.

Los parámetros congelados quedan en el artefacto `parameters` del job y en
`job.parameters` del snapshot. Para que el tope de salida, la temperatura, el
timeout y las esperas de reintento gobiernen las llamadas, el cliente se arma
con ellos:

```rust
let cliente = ClienteLlmOpenRouter::con_parametros(&api_key, &modelo, &job["parameters"]);
```

`new` y `con_ajustes` conservan su conducta (defaults del agente); el endpoint de
producción es el mismo de siempre.

## Tabla de parámetros

Cada fila es un campo de `parameters`. «Efecto de subirlo» y «efecto de bajarlo»
describen la consecuencia visible, no solo el número.

| Parámetro | Qué significa | Default | Rango | Efecto de subirlo | Efecto de bajarlo |
| --- | --- | --- | --- | --- | --- |
| `max_tokens_salida` | Cuántos tokens puede escribir el modelo en un turno (el razonamiento del modelo también cuenta). | `16384` | 1024 – 131072 | Entran respuestas más largas: lotes de archivo con muchas citas literales dejan de cortarse. Cuesta más por llamada. | Las respuestas largas se cortan en el tope y el lote de archivo falla; el motor lo detecta y reintenta con un lote más chico, gastando llamadas. |
| `temperatura` | Cuánto azar admite la generación del modelo. | `0.3` | 0.0 – 2.0 | Más variación y creatividad en la redacción; más riesgo de que el JSON del contrato derive o invente. | Salidas más repetitivas y literales; más riesgo de respuestas vacías o telegráficas. |
| `timeout_s` | Segundos máximos que el cliente espera cada intento antes de abortar. | `180` | 1 – 3600 | Se toleran proveedores lentos o respuestas grandes, pero un cuelgue tarda más en detectarse. | Un proveedor lento se reporta como timeout antes; con lotes grandes puede abortar respuestas que iban a llegar. |
| `esperas_reintento_s` | Espera, en segundos, antes de cada reintento de red. `esperas[i]` vale para el intento `i` que falló; la cantidad de intentos es `len + 1`. | `[2, 6]` | 0 a 8 valores, cada uno 0 – 120 | Más intentos (y más espaciados) ante 429/5xx o cortes: más chance de cerrar, más tiempo de espera y más costo si el proveedor cobra los intentos. | Menos reintentos: los fallos transitorios llegan al historiador como error más rápido. `[]` deja un solo intento. |
| `lotes_archivo_bytes` | Tamaño máximo, en bytes, del insumo de un lote de archivo. Es una escalera de mayor a menor: cada reintento del mismo lote usa el siguiente tope. | `[48000, 24000, 12000]` | 1 a 5 valores, cada uno 1000 – 1000000, estrictamente decreciente | Entran más documentos por lote: menos llamadas, pero más salida pedida y más chance de corte. | Lotes más chicos: más llamadas y más costo fijo, menos chance de que un lote no cierre. |
| `fragmento_archivo_bytes` | Tamaño, en bytes, con que se parte una evidencia que no entra en un fragmento. | `8000` | 200 – 1000000, nunca mayor que el primer lote | Fragmentos más grandes: menos pedazos por documento, cada lote cubre menos documentos distintos. | Más pedazos por documento: se conserva la granularidad de las citas, a costa de más lotes. |
| `ventana_cita` | Caracteres del fragmento de fuente que el informe reproduce por cita. | `600` | 100 – 100000 | Citas más largas: más contexto para el lector, informe más largo. | Citas más cortas: el recorte queda marcado y el lector tiene menos contexto. |
| `preguntas_minimas` | Preguntas mínimas de la ronda de encuadre. Si el modelo trae menos, el código completa con los ejes de la modalidad. | `4` | 1 – 8 | Un encuadre más fino antes de gastar en el archivo; una ronda más larga para el investigador. Por encima de la cantidad de ejes (4), las preguntas extra se piden como profundización y el evento lo declara. | Una ronda más corta: el plan depende de menos respuestas y el encuadre puede quedar pobre. |
| `rerank_depth` | Cuántos candidatos de la fusión entran al rerank; es también el techo del `retrieval_limit` de un plan. | `16` | 1 – 200 | Más candidatos evaluados: mejor selección, rerank más caro. | Menos candidatos: recuperación más barata y más gruesa. |
| `leg_k` | Cuántos candidatos trae cada pierna de recuperación (vectorial y léxica) antes de fusionar. | `24` | 1 – 1000 | Cada pierna busca más hondo: más recall potencial, más cómputo local y más resultados de FTS5. | Piernas más cortas: recuperación más rápida y más superficial. |
| `rrf_k` | Constante de suavizado de la fusión RRF: cuánto pesa estar primero en una pierna frente a estar en las dos. | `60` | 1 – 10000 | La fusión se aplana: aparecer en las dos piernas manda menos y pesa más la posición individual. | La fusión se agudiza: aparecer en una sola pierna casi no suma y manda la intersección. |
| `snippet_max` | Caracteres por fragmento en los documentos que se mandan al rerank y en la salida de la búsqueda léxica. | `1600` | 100 – 100000 | El reranker ve más texto por candidato: juzga mejor pasajes que empiezan al final del fragmento, y consume más tokens. | El reranker ve menos texto: más barato y más ciego a los pasajes que caen después del corte. |
| `max_consultas` | Cuántas consultas puede proponer el plan, por identificador de modalidad. Una modalidad sin entrada conserva su tope de tabla. | `{"general": 20, "trayectorias": 30, "cronologia": 20}` | cada valor 1 – 200; las claves tienen que ser modalidades existentes | Más consultas por plan: más cobertura del corpus, más llamadas de búsqueda (no se descuentan del presupuesto del modelo) y más evidencia para el archivo. | Menos consultas: búsqueda más barata y rápida, cobertura más angosta. |

## Reglas de validación

`create` valida antes de escribir nada: un `parameters` inválido devuelve `Err`
con el mensaje de la regla y **no crea job ni artefactos**. El mensaje nombra el
campo (con índice cuando es una lista), el rango admitido y el valor recibido:

```
max_tokens_salida debe estar entre 1024 y 131072 (recibido: 10)
lotes_archivo_bytes[0] debe estar entre 1000 y 1000000 (recibido: 500)
lotes_archivo_bytes debe ser estrictamente decreciente: lotes_archivo_bytes[1] (48000) no es menor que lotes_archivo_bytes[0] (48000)
fragmento_archivo_bytes (8000) no puede superar el primer lote de archivo (4000)
max_consultas nombra un perfil desconocido: «biografia»; disponibles: general, trayectorias, cronologia
Parámetros del agente: campo desconocido «temperaturaa». Campos válidos: max_tokens_salida, temperatura, timeout_s, esperas_reintento_s, lotes_archivo_bytes, fragmento_archivo_bytes, ventana_cita, preguntas_minimas, rerank_depth, leg_k, rrf_k, snippet_max, max_consultas
```

Las reglas que cruzan campos son dos:

- la escalera de `lotes_archivo_bytes` es estrictamente decreciente (un tope que
  no achica el lote es la misma llamada otra vez);
- `fragmento_archivo_bytes` no supera el primer tope de la escalera (un fragmento
  que no entra en ningún lote no se puede procesar nunca).

## Congelado y jobs anteriores

1. `create` valida `parameters` y, en la misma transacción del job, escribe el
   artefacto `parameters` con la estructura completa ya completada con defaults.
2. Cada paso (`advance`, `answer`, `resume`) lee ese artefacto. Cambiar la
   configuración del desktop no cambia un job en curso: para eso hay que crear
   otro.
3. Un job sin artefacto `parameters` usa `ParametrosAgente::default()`, así que
   los jobs anteriores a esta estructura siguen corriendo como siempre.
4. `procesar_con` no cambia de firma: los parámetros no son un argumento, son
   parte del pedido de `create` y del estado del job.

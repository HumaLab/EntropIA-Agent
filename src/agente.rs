//! Agente autónomo de investigación historiográfica con tool-calling.
//!
//! El agente recibe el pedido del investigador y decide sus pasos: entrevista,
//! busca fuentes en la base (RAG), lee fragmentos, redacta y guarda el
//! informe. El loop itera y llama a las herramientas hasta que el modelo emite su
//! respuesta final.
//!
//! Fase 0 (PLAN §9): sin base de datos el agente **aborta con error** en vez de
//! redactar un informe sin fuentes, y todo informe guardado abre con la tabla
//! de cobertura del recorte consultado.

use std::io::{self, BufRead, Write};
use std::path::Path;

use serde_json::{json, Value};

use crate::cliente_llm::{ClienteLlmOpenRouter, TurnoAgente};
use crate::dominio::{ClaseFuente, Ledger};
use crate::estado::EstadoDb;
use crate::informe;
use crate::memoria::{MemoriaDb, TipoMemoria};
use crate::prompts;
use crate::puerta_lectura;
use crate::recuperacion::Recuperador;
use crate::repositorio::RepositorioSqlite;
use crate::verificador::{EvidenciaConTexto, ModoVerificacion, Verificador};

/// Tope de iteraciones del loop para evitar bucles sin fin.
const MAX_PASOS: usize = 25;

/// Agente que ejecuta el loop de herramientas.
pub struct Agente {
    cliente: ClienteLlmOpenRouter,
    recuperador: Option<Recuperador>,
    repo: Option<RepositorioSqlite>,
    estado: Option<EstadoDb>,
}

impl Agente {
    pub fn new(
        cliente: ClienteLlmOpenRouter,
        recuperador: Option<Recuperador>,
        repo: Option<RepositorioSqlite>,
    ) -> Self {
        Self {
            cliente,
            recuperador,
            repo,
            estado: None,
        }
    }

    /// Conecta el estado persistente del agente (Fase 1/2: ledger epistémico
    /// y memoria longitudinal) para las herramientas que lo requieren.
    pub fn con_estado(mut self, estado: EstadoDb) -> Self {
        self.estado = Some(estado);
        self
    }

    /// Ejecuta el agente con el pedido del investigador.
    ///
    /// Sin base conectada devuelve un error en vez de redactar: un informe sin
    /// una sola fuente sería engañoso (PLAN §9, Fase 0).
    pub fn ejecutar<W: Write>(
        &self,
        pedido: &str,
        stdout: &mut W,
        stdin: &io::Stdin,
    ) -> Result<(), String> {
        if self.repo.is_none() {
            return Err(
                "No hay base de datos conectada: definí ENTROPIA_DB_PATH para usar la base de la app. \
                 Sin fuentes, el agente no produce informes."
                    .to_string(),
            );
        }

        let mut mensajes: Vec<Value> = vec![
            json!({ "role": "system", "content": prompts::PROMPT_AGENTE }),
            json!({ "role": "user", "content": pedido }),
        ];
        let defs = definiciones_herramientas();

        for paso in 1..=MAX_PASOS {
            writeln!(stdout, "\n[Agente · paso {paso}]").ok();
            let turno = self.cliente.turno_agente(&mensajes, &defs)?;

            match turno {
                TurnoAgente::Herramientas(llamadas) => {
                    for l in &llamadas {
                        writeln!(stdout, "  → herramienta: {} ({})", l.nombre, l.argumentos).ok();
                    }
                    let tool_calls: Vec<Value> = llamadas
                        .iter()
                        .map(|l| {
                            json!({
                                "id": l.id,
                                "type": "function",
                                "function": { "name": l.nombre, "arguments": l.argumentos.to_string() }
                            })
                        })
                        .collect();
                    mensajes.push(
                        json!({ "role": "assistant", "content": null, "tool_calls": tool_calls }),
                    );

                    for l in llamadas {
                        let resultado =
                            self.ejecutar_herramienta(&l.nombre, &l.argumentos, stdout, stdin);
                        mensajes.push(
                            json!({ "role": "tool", "tool_call_id": l.id, "content": resultado }),
                        );
                    }
                }
                TurnoAgente::Texto(texto) => {
                    if !texto.is_empty() {
                        writeln!(stdout, "\n{texto}").ok();
                    }
                    writeln!(stdout, "\n[Agente] Investigación concluida.").ok();
                    return Ok(());
                }
            }
        }

        writeln!(
            stdout,
            "\n[Agente] Se alcanzó el máximo de pasos sin conclusión."
        )
        .ok();
        Ok(())
    }

    /// Ejecuta una herramienta por nombre y devuelve su resultado como texto.
    fn ejecutar_herramienta<W: Write>(
        &self,
        nombre: &str,
        args: &Value,
        stdout: &mut W,
        stdin: &io::Stdin,
    ) -> String {
        match nombre {
            "buscar_fuentes" => {
                let consulta = args["consulta"].as_str().unwrap_or("").to_string();
                let limite = args["limite"].as_u64().unwrap_or(6) as usize;
                match (&self.recuperador, &self.repo) {
                    (Some(rec), Some(repo)) => {
                        let fuentes = rec.recuperar_consulta(repo, &consulta, limite);
                        let resumen: Vec<Value> = fuentes
                            .iter()
                            .enumerate()
                            .map(|(i, f)| {
                                json!({
                                    "n": i + 1,
                                    "id": f.id,
                                    "titulo": f.titulo,
                                    "coleccion": f.coleccion,
                                    "contenido": f.contenido,
                                })
                            })
                            .collect();
                        serde_json::to_string(&resumen).unwrap_or_else(|_| "[]".into())
                    }
                    _ => "No hay base de datos conectada (definí ENTROPIA_DB_PATH).".into(),
                }
            }
            "leer_fragmento" => {
                let id = args["chunk_id"].as_str().unwrap_or("");
                match &self.repo {
                    Some(repo) => repo
                        .texto_chunk(id)
                        .unwrap_or_else(|| format!("No se encontró el fragmento «{id}».")),
                    None => "No hay base de datos conectada.".into(),
                }
            }
            "listar_colecciones" => match &self.repo {
                Some(repo) => {
                    let cols = repo.listar_colecciones();
                    let arr: Vec<Value> = cols
                        .iter()
                        .map(|c| {
                            json!({
                                "coleccion": c.nombre,
                                "items": c.items,
                                "items_con_chunks": c.items_con_chunks,
                                "items_sin_procesar": c.items_sin_procesar(),
                                "chunks": c.chunks,
                            })
                        })
                        .collect();
                    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".into())
                }
                None => "No hay base de datos conectada.".into(),
            },
            "preguntar_al_investigador" => {
                let pregunta = args["pregunta"].as_str().unwrap_or("");
                write!(stdout, "\nInvestigador: {pregunta}\nTu respuesta: ").ok();
                stdout.flush().ok();
                let mut linea = String::new();
                let _ = stdin.lock().read_line(&mut linea);
                linea.trim().to_string()
            }
            "guardar_informe" => {
                let titulo = args["titulo"].as_str().unwrap_or("informe");
                let texto = args["texto"].as_str().unwrap_or("");
                // Todo informe abre con la tabla de cobertura del recorte
                // consultado (PLAN §6.5): sin ella, el informe es engañoso.
                let cobertura = match &self.repo {
                    Some(repo) => informe::tabla_cobertura(&repo.cobertura()),
                    None => String::new(),
                };
                let completo = format!("{cobertura}{texto}");
                match informe::guardar(Path::new("informes"), titulo, &completo) {
                    Ok(ruta) => format!("Informe guardado en: {}", ruta.display()),
                    Err(e) => format!("No se pudo guardar el informe: {e}"),
                }
            }
            // ── Herramientas del Modo 1 (PLAN §6.4, Fase 2) ───────────────
            "buscar_entidad" => {
                let nombre = args["nombre"].as_str().unwrap_or("");
                match &self.repo {
                    Some(repo) => match puerta_lectura::buscar_entidad(repo, nombre) {
                        Some(nodo) => serde_json::to_string(&json!({
                            "entidad": nodo.entidad,
                            "tipo": nodo.tipo,
                            "items": nodo.items,
                            "triples": nodo.triples,
                            "chunks": nodo.chunks.len(),
                        }))
                        .unwrap_or_else(|_| "{}".into()),
                        None => format!("No se encontró la entidad «{nombre}»."),
                    },
                    None => "No hay base de datos conectada.".into(),
                }
            }
            "leer_asset" => {
                let asset_id = args["asset_id"].as_str().unwrap_or("");
                match &self.repo {
                    Some(repo) => puerta_lectura::leer_asset(repo, asset_id)
                        .unwrap_or_else(|| format!("No se encontró el asset «{asset_id}».")),
                    None => "No hay base de datos conectada.".into(),
                }
            }
            "mostrar_fuente" => {
                let item_id = args["item_id"].as_str().unwrap_or("");
                match &self.repo {
                    Some(repo) => {
                        let assets = puerta_lectura::mostrar_fuente(repo, item_id);
                        if assets.is_empty() {
                            format!("El item «{item_id}» no tiene assets.")
                        } else {
                            let arr: Vec<Value> = assets
                                .iter()
                                .map(|(p, n)| json!({ "path": p, "pagina": n }))
                                .collect();
                            serde_json::to_string(&arr).unwrap_or_else(|_| "[]".into())
                        }
                    }
                    None => "No hay base de datos conectada.".into(),
                }
            }
            "verificar_afirmacion" => {
                let afirmacion = args["afirmacion"].as_str().unwrap_or("");
                let citas: Vec<String> = args["citas"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|c| c.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                let verificador = Verificador::nuevo(&self.cliente);
                let evidencias: Vec<EvidenciaConTexto> = citas
                    .iter()
                    .map(|cita| EvidenciaConTexto {
                        id: "cita".into(),
                        quote: cita.clone(),
                        span_start: 0,
                        span_end: cita.chars().count() as i64,
                        texto_fuente: cita.clone(),
                        relacion: "supports".into(),
                    })
                    .collect();
                match verificador.verificar(
                    afirmacion,
                    &evidencias,
                    ModoVerificacion::Factual,
                    None,
                ) {
                    Ok(r) => serde_json::to_string(&json!({
                        "estado": r.estado.as_str(),
                        "rationale": r.rationale,
                        "error_kind": r.error_kind,
                    }))
                    .unwrap_or_else(|_| "{}".into()),
                    Err(e) => format!("No se pudo verificar: {e}"),
                }
            }
            "registrar_evidencia" => {
                let chunk_id = args["chunk_id"].as_str().unwrap_or("");
                let cita = args["cita"].as_str().unwrap_or("");
                match (&self.estado, &self.repo) {
                    (Some(estado), Some(_repo)) => {
                        let ledger = Ledger::nuevo(estado);
                        let src = match ledger.registrar_fuente(
                            ClaseFuente::EntropiaChunk,
                            Some(chunk_id),
                            None,
                            None,
                            Some(chunk_id),
                            None,
                            "soip-conflictividad",
                            "soip",
                        ) {
                            Ok(s) => s,
                            Err(e) => return format!("No se pudo registrar la fuente: {e}"),
                        };
                        let fin = cita.chars().count() as i64;
                        match ledger.registrar_evidencia(&src, cita, 0, fin, None, Some(0.9)) {
                            Ok(ev) => format!("Evidencia registrada: {ev}"),
                            Err(e) => format!("No se pudo registrar la evidencia: {e}"),
                        }
                    }
                    _ => "Requiere estado persistente (estado.sqlite) y base conectada.".into(),
                }
            }
            "actualizar_informe" => {
                let seccion_id = args["seccion"].as_str().unwrap_or("seccion");
                let titulo = args["titulo"].as_str().unwrap_or("Sección");
                let texto = args["texto"].as_str().unwrap_or("");
                match &self.estado {
                    Some(estado) => {
                        let secciones = crate::informe_secciones::InformeSecciones::nuevo(
                            estado,
                            Path::new("informes"),
                        );
                        let seccion = crate::informe_secciones::SeccionInforme {
                            id: seccion_id.into(),
                            titulo: titulo.into(),
                            contenido: texto.into(),
                            version: 1,
                            provenance: vec![],
                        };
                        match secciones.guardar_seccion("cli", &seccion) {
                            Ok(ruta) => {
                                let version = secciones.version_actual("cli", seccion_id);
                                format!(
                                    "Sección «{seccion_id}» v{version} guardada en: {}",
                                    ruta.display()
                                )
                            }
                            Err(e) => format!("No se pudo guardar la sección: {e}"),
                        }
                    }
                    None => "Requiere estado persistente (estado.sqlite).".into(),
                }
            }
            "consultar_memoria" => {
                let project = args["project"].as_str().unwrap_or("soip-conflictividad");
                let texto = args["texto"].as_str().unwrap_or("");
                match &self.estado {
                    Some(estado) => {
                        let memoria = MemoriaDb::nuevo(estado);
                        let resultados = memoria.buscar(project, texto, 5);
                        let arr: Vec<Value> = resultados
                            .iter()
                            .map(|m| json!({ "titulo": m.title, "tipo": m.tipo, "contenido": m.content }))
                            .collect();
                        serde_json::to_string(&arr).unwrap_or_else(|_| "[]".into())
                    }
                    None => "Requiere estado persistente (estado.sqlite).".into(),
                }
            }
            "registrar_hallazgo" => {
                let project = args["project"].as_str().unwrap_or("soip-conflictividad");
                let titulo = args["titulo"].as_str().unwrap_or("Hallazgo");
                let contenido = args["contenido"].as_str().unwrap_or("");
                match &self.estado {
                    Some(estado) => {
                        let memoria = MemoriaDb::nuevo(estado);
                        let tipo = match args["tipo"].as_str() {
                            Some("decision") => TipoMemoria::Decision,
                            Some("question") => TipoMemoria::Question,
                            Some("hypothesis") => TipoMemoria::Hypothesis,
                            Some("interpretation") => TipoMemoria::Interpretation,
                            Some("learning") => TipoMemoria::Learning,
                            _ => TipoMemoria::Finding,
                        };
                        match memoria.guardar(project, titulo, tipo, contenido, None, None) {
                            Ok((id, candidatos)) => {
                                let pendientes = candidatos.len();
                                format!(
                                    "Hallazgo guardado: {id} ({pendientes} relación(es) pendiente(s) de juicio)"
                                )
                            }
                            Err(e) => format!("No se pudo guardar el hallazgo: {e}"),
                        }
                    }
                    None => "Requiere estado persistente (estado.sqlite).".into(),
                }
            }
            _ => format!("Herramienta desconocida: {nombre}"),
        }
    }
}

/// Definiciones de herramientas que se envían al modelo (esquema OpenAI).
fn definiciones_herramientas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "buscar_fuentes",
                "description": "Busca fragmentos documentales en la base por una consulta (semántico más léxico con rerank). Devuelve los más relevantes con su texto. El límite se acota a 16 (profundidad del pipeline).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "consulta": { "type": "string", "description": "Consulta con términos propios del campo" },
                        "limite": { "type": "integer", "description": "Cantidad máxima de fragmentos (máx. 16)", "default": 6 }
                    },
                    "required": ["consulta"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "leer_fragmento",
                "description": "Devuelve el texto completo de un fragmento por su identificador.",
                "parameters": {
                    "type": "object",
                    "properties": { "chunk_id": { "type": "string", "description": "Identificador del fragmento" } },
                    "required": ["chunk_id"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "listar_colecciones",
                "description": "Lista las colecciones documentales de la base (fuera de las de prueba) con su cobertura: items totales, items con chunks, items sin procesar y chunks. Es la herramienta de cobertura.",
                "parameters": { "type": "object", "properties": {} }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "preguntar_al_investigador",
                "description": "Hace una pregunta al investigador por la consola y devuelve su respuesta.",
                "parameters": {
                    "type": "object",
                    "properties": { "pregunta": { "type": "string", "description": "Pregunta clara y precisa" } },
                    "required": ["pregunta"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "guardar_informe",
                "description": "Guarda el informe historiográfico en un archivo Markdown. El informe abre con la tabla de cobertura del recorte consultado.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "titulo": { "type": "string", "description": "Título o tema del informe" },
                        "texto": { "type": "string", "description": "Texto completo del informe" }
                    },
                    "required": ["titulo", "texto"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "buscar_entidad",
                "description": "Recupera un nodo del grafo de entidades: la entidad, sus items, sus triples y los chunks ligados (traversal read-only sobre entities/triples).",
                "parameters": {
                    "type": "object",
                    "properties": { "nombre": { "type": "string", "description": "Nombre o fragmento de la entidad (persona, institución, lugar)" } },
                    "required": ["nombre"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "leer_asset",
                "description": "Devuelve el texto completo de un asset (todos sus chunks en orden) cuando un fragmento no alcanza.",
                "parameters": {
                    "type": "object",
                    "properties": { "asset_id": { "type": "string", "description": "Identificador del asset" } },
                    "required": ["asset_id"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "mostrar_fuente",
                "description": "Devuelve la ruta del escaneo original y la página de un item, para verificación humana.",
                "parameters": {
                    "type": "object",
                    "properties": { "item_id": { "type": "string", "description": "Identificador del item" } },
                    "required": ["item_id"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "verificar_afirmacion",
                "description": "Verifica una afirmación contra citas textuales con el Verificador de dos modos: span check determinista + entailment. Devuelve el estado epistémico.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "afirmacion": { "type": "string", "description": "Afirmación a verificar" },
                        "citas": { "type": "array", "items": { "type": "string" }, "description": "Citas textuales de las fuentes" }
                    },
                    "required": ["afirmacion", "citas"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "registrar_evidencia",
                "description": "Registra una cita como evidencia en el ledger epistémico, ligada al chunk de la fuente (trazabilidad afirmación → evidencia → fuente).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "chunk_id": { "type": "string", "description": "Identificador del chunk" },
                        "cita": { "type": "string", "description": "Cita textual exacta" }
                    },
                    "required": ["chunk_id", "cita"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "actualizar_informe",
                "description": "Construye el informe por secciones versionadas: cada llamada genera una versión nueva de la sección con provenance, sin regenerar las demás.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "seccion": { "type": "string", "description": "Id estable de la sección (p. ej. cronologia)" },
                        "titulo": { "type": "string", "description": "Título de la sección" },
                        "texto": { "type": "string", "description": "Contenido de la sección" }
                    },
                    "required": ["seccion", "titulo", "texto"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "consultar_memoria",
                "description": "Consulta la memoria longitudinal por similitud FTS5: hallazgos y decisiones previas de la línea de investigación.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "project": { "type": "string", "description": "Línea de investigación (p. ej. soip-conflictividad)" },
                        "texto": { "type": "string", "description": "Términos de búsqueda" }
                    },
                    "required": ["texto"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "registrar_hallazgo",
                "description": "Guarda un hallazgo en la memoria longitudinal. Si contradice uno previo, superficie un conflicto pendiente de juicio en vez de sobrescribir.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "project": { "type": "string", "description": "Línea de investigación" },
                        "titulo": { "type": "string", "description": "Título del hallazgo" },
                        "contenido": { "type": "string", "description": "Contenido del hallazgo" },
                        "tipo": { "type": "string", "description": "finding | decision | question | hypothesis | interpretation | learning", "default": "finding" }
                    },
                    "required": ["titulo", "contenido"]
                }
            }
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definiciones_tiene_las_trece_herramientas_del_modo_1() {
        let defs = definiciones_herramientas();
        assert_eq!(defs.len(), 13);
        let nombres: Vec<&str> = defs
            .iter()
            .map(|d| d["function"]["name"].as_str().unwrap())
            .collect();
        for esperada in [
            "buscar_fuentes",
            "leer_fragmento",
            "listar_colecciones",
            "preguntar_al_investigador",
            "guardar_informe",
            "buscar_entidad",
            "leer_asset",
            "mostrar_fuente",
            "verificar_afirmacion",
            "registrar_evidencia",
            "actualizar_informe",
            "consultar_memoria",
            "registrar_hallazgo",
        ] {
            assert!(
                nombres.contains(&esperada),
                "falta la herramienta {esperada}"
            );
        }
    }

    #[test]
    fn cada_definicion_es_tipo_function() {
        for d in definiciones_herramientas() {
            assert_eq!(d["type"], "function");
        }
    }

    #[test]
    fn sin_base_el_agente_aborta_con_error() {
        let cliente = ClienteLlmOpenRouter::new("clave-de-prueba", "modelo-de-prueba");
        let agente = Agente::new(cliente, None, None);
        let mut salida = Vec::new();
        let err = agente
            .ejecutar("pedido de prueba", &mut salida, &io::stdin())
            .unwrap_err();
        assert!(err.contains("ENTROPIA_DB_PATH"));
        // No llegó a llamar al modelo: sin mensajes de agente.
        assert!(String::from_utf8_lossy(&salida).is_empty());
    }
}

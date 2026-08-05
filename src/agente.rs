//! Agente autónomo de investigación historiográfica con tool-calling.
//!
//! El agente recibe el pedido del investigador y decide sus pasos: entrevista,
//! busca fuentes en la base (RAG), lee fragmentos, redacta y guarda el
//! informe. El loop itera y llama a las herramientas hasta que el modelo emite su
//! respuesta final.

use std::io::{self, BufRead, Write};

use serde_json::{json, Value};

use crate::cliente_llm::{ClienteLlmOpenRouter, TurnoAgente};
use crate::informe;
use crate::prompts;
use crate::recuperacion::Recuperador;
use crate::repositorio::RepositorioSqlite;

/// Tope de iteraciones del loop para evitar bucles sin fin.
const MAX_PASOS: usize = 25;

/// Agente que ejecuta el loop de herramientas.
pub struct Agente {
    cliente: ClienteLlmOpenRouter,
    recuperador: Option<Recuperador>,
    repo: Option<RepositorioSqlite>,
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
        }
    }

    /// Ejecuta el agente con el pedido del investigador.
    pub fn ejecutar<W: Write>(
        &self,
        pedido: &str,
        stdout: &mut W,
        stdin: &io::Stdin,
    ) -> Result<(), String> {
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
                        .map(|(nombre, cantidad)| json!({ "coleccion": nombre, "documentos": cantidad }))
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
                match informe::guardar(titulo, texto) {
                    Ok(ruta) => format!("Informe guardado en: {}", ruta.display()),
                    Err(e) => format!("No se pudo guardar el informe: {e}"),
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
                "description": "Busca fragmentos documentales en la base por una consulta (semántico más léxico con rerank). Devuelve los más relevantes con su texto.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "consulta": { "type": "string", "description": "Consulta con términos propios del campo" },
                        "limite": { "type": "integer", "description": "Cantidad máxima de fragmentos", "default": 6 }
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
                "description": "Lista las colecciones documentales de la base con su cantidad de documentos.",
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
                "description": "Guarda el informe historiográfico en un archivo Markdown.",
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
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definiciones_tiene_las_cinco_herramientas() {
        let defs = definiciones_herramientas();
        assert_eq!(defs.len(), 5);
        let nombres: Vec<&str> = defs
            .iter()
            .map(|d| d["function"]["name"].as_str().unwrap())
            .collect();
        assert!(nombres.contains(&"buscar_fuentes"));
        assert!(nombres.contains(&"leer_fragmento"));
        assert!(nombres.contains(&"listar_colecciones"));
        assert!(nombres.contains(&"preguntar_al_investigador"));
        assert!(nombres.contains(&"guardar_informe"));
    }

    #[test]
    fn cada_definicion_es_tipo_function() {
        for d in definiciones_herramientas() {
            assert_eq!(d["type"], "function");
        }
    }
}

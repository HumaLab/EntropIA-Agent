//! Doble de LLM para tests (solo se compila con `cargo test`).
//!
//! Responde guiones deterministas: el orquestador, los workers y el
//! verificador se prueban contra respuestas conocidas, sin red.

use serde_json::Value;

use crate::cliente_llm::{ClienteLlm, TurnoAgente};

/// Devuelve `texto` como respuesta final ante cualquier turno.
pub struct LlmTexto(pub String);

impl ClienteLlm for LlmTexto {
    fn turno_agente(
        &self,
        _mensajes: &[Value],
        _herramientas: &[Value],
    ) -> Result<TurnoAgente, String> {
        Ok(TurnoAgente::Texto(self.0.clone()))
    }

    fn modelo(&self) -> &str {
        "fake/texto"
    }
}

/// Devuelve `texto` y registra en `Arc<Mutex<Vec<String>>>` los mensajes que
/// recibió (para aserciones sobre el protocolo aislado del Verifier y la
/// frontera de confianza).
pub struct LlmGrabador {
    pub texto: String,
    pub registro: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl LlmGrabador {
    pub fn nuevo(texto: &str) -> Self {
        Self {
            texto: texto.to_string(),
            registro: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }
}

impl ClienteLlm for LlmGrabador {
    fn turno_agente(
        &self,
        mensajes: &[Value],
        _herramientas: &[Value],
    ) -> Result<TurnoAgente, String> {
        for m in mensajes {
            if let Some(rol) = m["role"].as_str() {
                if let Some(contenido) = m["content"].as_str() {
                    let mut reg = self.registro.lock().unwrap();
                    reg.push(format!("{rol}: {contenido}"));
                }
            }
        }
        Ok(TurnoAgente::Texto(self.texto.clone()))
    }

    fn modelo(&self) -> &str {
        "fake/grabador"
    }
}

/// Fake para el e2e del orquestador/paper: extrae del mensaje de usuario todas
/// las evidencias (`[ev-…]` + cita) y emite una síntesis con una afirmación por
/// evidencia, cuyo texto es exactamente la cita (entailment determinista →
/// supported). Ante un mensaje sin evidencia devuelve una síntesis sin claims.
pub struct LlmSintetizaEvidencia;

impl ClienteLlm for LlmSintetizaEvidencia {
    fn turno_agente(
        &self,
        mensajes: &[Value],
        _herramientas: &[Value],
    ) -> Result<TurnoAgente, String> {
        let contenido_usuario = mensajes
            .iter()
            .filter_map(|m| m["content"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let evidencias = todas_las_evidencias(&contenido_usuario);
        if evidencias.is_empty() {
            return Ok(TurnoAgente::Texto(
                "Resumen del stage sin afirmaciones.".into(),
            ));
        }
        let mut texto = String::from("Resumen del stage.\n");
        for (id, cita) in evidencias {
            texto.push_str(&format!("AFIRMACIÓN: {cita}\nEVIDENCIA: {id}\n"));
        }
        Ok(TurnoAgente::Texto(texto))
    }

    fn modelo(&self) -> &str {
        "fake/sintetiza"
    }
}

/// Extrae `(id, cita)` de todas las evidencias del bloque de datos.
fn todas_las_evidencias(contenido: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut lineas = contenido.lines();
    while let Some(l) = lineas.next() {
        if let Some(resto) = l.strip_prefix('[') {
            if let Some(id) = resto.split(']').next() {
                if id.starts_with("ev-") {
                    let cita = lineas.next().unwrap_or("").trim().to_string();
                    if !cita.is_empty() {
                        out.push((id.to_string(), cita));
                    }
                }
            }
        }
    }
    out
}

/// Doble del workflow durable de investigación: responde los contratos JSON de
/// cada rol de `investigacion.rs`. Permite ejercitar el ciclo completo de la
/// API sin red y sin depender del formato de prosa de ningún proveedor.
pub struct LlmWorkflow;

impl ClienteLlm for LlmWorkflow {
    fn modelo(&self) -> &str {
        "fake/workflow"
    }

    fn ultimo_costo(&self) -> Option<f64> {
        Some(0.01)
    }

    fn turno_agente(&self, mensajes: &[Value], _: &[Value]) -> Result<TurnoAgente, String> {
        let sistema = mensajes[0]["content"].as_str().unwrap_or_default();
        let usuario = mensajes[1]["content"].as_str().unwrap_or_default();
        // El verificador manda prosa, no JSON.
        let datos: Value = serde_json::from_str(usuario).unwrap_or(Value::Null);
        let salida = if sistema.contains("Rol: prospeccion.") {
            serde_json::json!({"sufficient":true,"rationale":"El recorte tiene material procesado","gaps":[]})
        } else if sistema.contains("{hypothesis:") {
            serde_json::json!({"hypothesis":"Hubo conflictividad","scope":"SOIP","closing_criteria":["Agotar los documentos recuperados"]})
        } else if sistema.contains("{questions:") {
            serde_json::json!({"questions":(1..=4).map(|i| serde_json::json!({
                "id": format!("q{i}"),
                "axis": "Período",
                "text": format!("Pregunta {i}"),
                "rationale": "Cambia el plan"
            })).collect::<Vec<_>>()})
        } else if sistema.contains("{queries:") {
            serde_json::json!({"queries":["huelga"],"bibliography_queries":[],"retrieval_limit":10})
        } else if sistema.contains("Rol: asistente_archivo.") {
            let evidencia = datos["evidence"][0]["id"].clone();
            let texto = datos["evidence"][0]["text"].as_str().unwrap_or_default();
            let pasaje: String = texto.chars().take(14).collect();
            serde_json::json!({"summary":"Síntesis","claims":[{"id":"c1","text":"Hubo conflictividad","evidence_ids":[evidencia.clone()],"quotes":[{"evidence_id":evidencia,"quote":pasaje}],"interpretative":false}]})
        } else if sistema.contains("Rol: asistente_bibliografia.") {
            serde_json::json!({"references":[],"synthesis":"Sin consultas bibliográficas"})
        } else if sistema.contains("Sos el Verificador de EntropIA.") {
            serde_json::json!({"estado":"supported","rationale":"El pasaje sostiene la afirmación","error_kind":null})
        } else {
            serde_json::json!({"title":"Informe","sections":[{"title":"Hechos","text":"Hubo conflictividad","claim_ids":["c1"]}]})
        };
        Ok(TurnoAgente::Texto(salida.to_string()))
    }
}

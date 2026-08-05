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

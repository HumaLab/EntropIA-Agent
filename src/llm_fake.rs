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

/// Fake para el e2e del orquestador: extrae del mensaje de usuario el primer
/// id de evidencia (`[ev-…]`) y su cita, y emite una síntesis cuyo claim es
/// exactamente la cita (entailment determinista → supported). Ante un mensaje
/// sin evidencia devuelve una síntesis sin claims.
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
        if let Some((id, cita)) = primera_evidencia(&contenido_usuario) {
            Ok(TurnoAgente::Texto(format!(
                "Resumen del stage.\nAFIRMACIÓN: {cita}\nEVIDENCIA: {id}"
            )))
        } else {
            Ok(TurnoAgente::Texto(
                "Resumen del stage sin afirmaciones.".into(),
            ))
        }
    }

    fn modelo(&self) -> &str {
        "fake/sintetiza"
    }
}

/// Extrae `(id, cita)` de la primera evidencia del bloque de datos
/// (`[ev-…] (fuente: …)` seguido de la línea de cita).
fn primera_evidencia(contenido: &str) -> Option<(String, String)> {
    let mut lineas = contenido.lines();
    while let Some(l) = lineas.next() {
        if let Some(resto) = l.strip_prefix('[') {
            if let Some(id) = resto.split(']').next() {
                if id.starts_with("ev-") {
                    let cita = lineas.next().unwrap_or("").trim().to_string();
                    if !cita.is_empty() {
                        return Some((id.to_string(), cita));
                    }
                }
            }
        }
    }
    None
}

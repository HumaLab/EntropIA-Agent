//! Cliente LLM de OpenRouter para el agente con tool-calling.
//!
//! Replica el patrón del desktop de EntropIA (endpoint, headers y modelo) y
//! añade soporte para herramientas (function calling).

use std::time::Duration;

use serde_json::{json, Value};

/// Cliente de OpenRouter.
pub struct ClienteLlmOpenRouter {
    client: reqwest::blocking::Client,
    api_key: String,
    model: String,
}

impl ClienteLlmOpenRouter {
    /// Endpoint de chat de OpenRouter.
    const CHAT_URL: &'static str = "https://openrouter.ai/api/v1/chat/completions";

    /// Modelo por defecto de la app EntropIA.
    pub const MODELO_DEFAULT: &'static str = "openai/gpt-5.6-luna-pro";

    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(180))
                .build()
                .unwrap_or_default(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }

    /// Un turno del agente con herramientas. Recibe el historial completo y
    /// las definiciones de herramientas; devuelve texto final o llamadas.
    pub fn turno_agente(
        &self,
        mensajes: &[Value],
        herramientas: &[Value],
    ) -> Result<TurnoAgente, String> {
        let mut request = json!({
            "model": self.model,
            "messages": mensajes,
            "temperature": 0.3,
            "max_tokens": 4096,
        });
        if !herramientas.is_empty() {
            request["tools"] = Value::Array(herramientas.to_vec());
            request["tool_choice"] = json!("auto");
        }

        let response = self
            .client
            .post(Self::CHAT_URL)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("HTTP-Referer", "https://hlab.com.ar/")
            .header("X-Title", "EntropIA")
            .json(&request)
            .send()
            .map_err(|e| format!("Error de conexión con OpenRouter: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let cuerpo = response.text().unwrap_or_default();
            return Err(format!("OpenRouter devolvió el estado {status}: {cuerpo}"));
        }

        // Algunos proveedores anteceden bytes no UTF-8 al JSON; se localiza el
        // primer '{' y se descarta el prefijo.
        let bytes = response
            .bytes()
            .map_err(|e| format!("No se pudo leer la respuesta de OpenRouter: {e}"))?;
        let inicio = bytes.iter().position(|&b| b == b'{').unwrap_or(0);
        let parsed: Value = serde_json::from_slice(&bytes[inicio..])
            .map_err(|e| format!("No se pudo leer la respuesta de OpenRouter: {e}"))?;

        let mensaje = &parsed["choices"][0]["message"];
        if let Some(tool_calls) = mensaje["tool_calls"].as_array() {
            let llamadas: Vec<Llamada> = tool_calls
                .iter()
                .filter_map(|tc| {
                    let id = tc["id"].as_str()?.to_string();
                    let nombre = tc["function"]["name"].as_str()?.to_string();
                    let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
                    let argumentos: Value =
                        serde_json::from_str(args_str).unwrap_or_else(|_| json!({}));
                    Some(Llamada {
                        id,
                        nombre,
                        argumentos,
                    })
                })
                .collect();
            if !llamadas.is_empty() {
                return Ok(TurnoAgente::Herramientas(llamadas));
            }
        }

        let texto = mensaje["content"].as_str().unwrap_or("").trim().to_string();
        Ok(TurnoAgente::Texto(texto))
    }
}

/// Resultado de un turno del agente: texto final o llamadas a herramientas.
pub enum TurnoAgente {
    Texto(String),
    Herramientas(Vec<Llamada>),
}

/// Una llamada a herramienta devuelta por el modelo.
pub struct Llamada {
    pub id: String,
    pub nombre: String,
    pub argumentos: Value,
}

/// Abstracción del cliente LLM para composiciones de rol (PLAN §5): cada rol
/// (orquestador, worker, verificador) es un prompt de sistema + un set de
/// herramientas + un tope de pasos sobre el mismo cliente. Los tests usan un
/// doble que responde guiones; producción usa `ClienteLlmOpenRouter`.
pub trait ClienteLlm {
    /// Un turno del rol: recibe el historial y las definiciones de
    /// herramientas; devuelve texto final o llamadas a herramientas.
    fn turno_agente(
        &self,
        mensajes: &[Value],
        herramientas: &[Value],
    ) -> Result<TurnoAgente, String>;

    /// Modelo activo (para el snapshot de reproducibilidad del job).
    fn modelo(&self) -> &str;
}

impl ClienteLlm for ClienteLlmOpenRouter {
    fn turno_agente(
        &self,
        mensajes: &[Value],
        herramientas: &[Value],
    ) -> Result<TurnoAgente, String> {
        ClienteLlmOpenRouter::turno_agente(self, mensajes, herramientas)
    }

    fn modelo(&self) -> &str {
        &self.model
    }
}

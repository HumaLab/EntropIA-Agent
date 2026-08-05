//! Cliente de embeddings de OpenRouter (BGE-M3, 1024 dimensiones).
//!
//! Replica el contrato del desktop de EntropIA: mismo endpoint, modelo y
//! formato de respuesta. La consulta se normaliza (L2) antes de compararla
//! con los vectores de los chunks.

use serde::{Deserialize, Serialize};

use crate::vector;

const ENDPOINT: &str = "https://openrouter.ai/api/v1/embeddings";
const MODELO: &str = "baai/bge-m3";

/// Cliente de embeddings vía OpenRouter.
pub struct ClienteEmbeddings {
    client: reqwest::blocking::Client,
    api_key: String,
}

impl ClienteEmbeddings {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .unwrap_or_default(),
            api_key: api_key.into(),
        }
    }

    /// Devuelve el vector normalizado de 1024 dimensiones para el texto.
    pub fn embed(&self, texto: &str) -> Result<Vec<f32>, String> {
        let request = EmbeddingRequest {
            model: MODELO,
            input: texto,
        };

        let response = self
            .client
            .post(ENDPOINT)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("HTTP-Referer", "https://hlab.com.ar/")
            .header("X-Title", "EntropIA")
            .json(&request)
            .send()
            .map_err(|e| format!("Embeddings: fallo de conexión: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let cuerpo = response.text().unwrap_or_default();
            return Err(format!("Embeddings: error {status}: {cuerpo}"));
        }

        let parsed: EmbeddingResponse = response
            .json()
            .map_err(|e| format!("Embeddings: respuesta ilegible: {e}"))?;

        parsed
            .data
            .into_iter()
            .next()
            .map(|entrada| vector::normalizar(&entrada.embedding))
            .ok_or_else(|| "Embeddings: la respuesta no trajo vectores.".to_string())
    }
}

#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a str,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingItem>,
}

#[derive(Deserialize)]
struct EmbeddingItem {
    embedding: Vec<f32>,
}

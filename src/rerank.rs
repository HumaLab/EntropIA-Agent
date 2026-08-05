//! Cliente de rerank de OpenRouter (cohere/rerank-4-fast).
//!
//! Reordena los candidatos por relevancia frente a la consulta. Replica el
//! contrato del desktop de EntropIA.

use serde::{Deserialize, Serialize};

const ENDPOINT: &str = "https://openrouter.ai/api/v1/rerank";
const MODELO: &str = "cohere/rerank-4-fast";

/// Cliente de rerank vía OpenRouter.
pub struct ClienteRerank {
    client: reqwest::blocking::Client,
    api_key: String,
}

impl ClienteRerank {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            api_key: api_key.into(),
        }
    }

    /// Reordena los documentos por relevancia frente a la consulta.
    ///
    /// Devuelve pares (índice original, puntaje) en orden descendente.
    pub fn rerank(
        &self,
        query: &str,
        documentos: &[String],
        top_n: usize,
    ) -> Result<Vec<(usize, f64)>, String> {
        let request = RerankRequest {
            model: MODELO,
            query,
            documents: documentos,
            top_n,
        };

        let response = self
            .client
            .post(ENDPOINT)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("HTTP-Referer", "https://hlab.com.ar/")
            .header("X-Title", "EntropIA")
            .json(&request)
            .send()
            .map_err(|e| format!("Rerank: fallo de conexión: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let cuerpo = response.text().unwrap_or_default();
            return Err(format!("Rerank: error {status}: {cuerpo}"));
        }

        let parsed: RerankResponse = response
            .json()
            .map_err(|e| format!("Rerank: respuesta ilegible: {e}"))?;

        let mut pares: Vec<(usize, f64)> = parsed
            .results
            .into_iter()
            .map(|r| (r.index, r.relevance_score))
            .collect();
        pares.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(pares)
    }
}

#[derive(Serialize)]
struct RerankRequest<'a> {
    model: &'a str,
    query: &'a str,
    documents: &'a [String],
    top_n: usize,
}

#[derive(Deserialize)]
struct RerankResponse {
    results: Vec<RerankResult>,
}

#[derive(Deserialize)]
struct RerankResult {
    index: usize,
    relevance_score: f64,
}

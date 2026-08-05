//! Búsqueda bibliográfica web (PLAN §6.4, Modo 2, Fase 3).
//!
//! OpenAlex/CrossRef devuelven **metadata estructurada** (Crossref es una API
//! de metadata, no de contenido). El full text se recupera desde la fuente
//! original respetando acceso y licencia; si el texto no es accesible, la
//! evidencia queda `unverifiable` (§6.1, §11#8). Las obras externas son
//! fuentes de **clase 4** (provenance, §3).

use std::time::Duration;

use serde_json::Value;

/// Una obra bibliográfica con metadata estructurada.
#[derive(Debug, Clone)]
pub struct Obra {
    pub titulo: String,
    pub autores: Vec<String>,
    pub anio: Option<i64>,
    pub doi: Option<String>,
    pub fuente: String,
    pub open_access: bool,
}

/// Busca metadata estructurada en OpenAlex.
pub fn buscar_openalex(query: &str) -> Result<Vec<Obra>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("OpenAlex: no se pudo crear el cliente: {e}"))?;
    let url = format!(
        "https://api.openalex.org/works?search={}&per-page=5",
        urlencode(query)
    );
    let response = client
        .get(&url)
        .send()
        .map_err(|e| format!("OpenAlex no respondió ({e}): se continúa sin bibliografía web."))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("OpenAlex devolvió el estado {status}."));
    }
    let texto = response
        .text()
        .map_err(|e| format!("OpenAlex: respuesta ilegible: {e}"))?;
    parse_openalex(&texto)
}

/// Parsea la respuesta de `api.openalex.org/works`.
pub fn parse_openalex(json: &str) -> Result<Vec<Obra>, String> {
    let valor: Value =
        serde_json::from_str(json).map_err(|e| format!("OpenAlex: JSON inválido: {e}"))?;
    let results = valor
        .get("results")
        .and_then(|r| r.as_array())
        .ok_or_else(|| "OpenAlex: respuesta sin «results».".to_string())?;
    let mut out = Vec::new();
    for r in results {
        let titulo = r["title"].as_str().unwrap_or("").to_string();
        if titulo.is_empty() {
            continue;
        }
        let autores: Vec<String> = r["authorships"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|a| a["author"]["display_name"].as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let anio = r["publication_year"].as_i64();
        let doi = r["doi"].as_str().map(|s| s.to_string());
        let open_access = r["open_access"]["is_oa"].as_bool().unwrap_or(false);
        out.push(Obra {
            titulo,
            autores,
            anio,
            doi,
            fuente: "openalex".into(),
            open_access,
        });
    }
    Ok(out)
}

/// El texto completo solo es accesible si la obra es open access (OpenAlex lo
/// reporta). Si no lo es, la evidencia queda `unverifiable` y se eleva al
/// historiador (PLAN §11#8): no se elude la licencia.
pub fn texto_accesible(obra: &Obra) -> bool {
    obra.open_access
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b' ' => out.push_str("%20"),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsea_works_de_openalex() {
        let json = r#"{
          "results": [
            {"title":"La conflictividad obrera en Mar del Plata","publication_year":2020,
             "doi":"https://doi.org/10.1000/abc","open_access":{"is_oa":true},
             "authorships":[{"author":{"display_name":"María López"}}]},
            {"title":"Sin texto abierto","publication_year":1990,"open_access":{"is_oa":false},
             "authorships":[]}
          ]
        }"#;
        let obras = parse_openalex(json).unwrap();
        assert_eq!(obras.len(), 2);
        assert_eq!(obras[0].titulo, "La conflictividad obrera en Mar del Plata");
        assert!(obras[0].open_access);
        assert_eq!(obras[0].doi.as_deref(), Some("https://doi.org/10.1000/abc"));
        assert!(!obras[1].open_access);
    }

    #[test]
    fn el_texto_inaccesible_queda_unverifiable() {
        let obra = Obra {
            titulo: "Sin acceso".into(),
            autores: vec![],
            anio: Some(1990),
            doi: Some("10.1000/xyz".into()),
            fuente: "openalex".into(),
            open_access: false,
        };
        assert!(!texto_accesible(&obra));
    }

    #[test]
    fn degradacion_sin_red() {
        // API inexistente (puerto cerrado): el flujo continúa reportando la
        // degradación en vez de fallar la investigación.
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(500))
            .build()
            .unwrap();
        let res = client.get("http://127.0.0.1:9/works?search=x").send();
        assert!(res.is_err());
    }
}

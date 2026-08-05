//! Consulta a la API local de Zotero (PLAN §6.4, Modo 2, Fase 3).
//!
//! Read-only sobre `localhost:23119`, con **degradación elegante**: si Zotero
//! no está abierto, la consulta devuelve un error claro y el flujo continúa sin
//! bibliografía de Zotero (reportándolo). Los items de Zotero son fuentes de
//! **clase 3** (provenance, §3).

use std::time::Duration;

use serde_json::Value;

/// URL por defecto de la API local de Zotero (solo lectura).
pub const ZOTERO_API_DEFAULT: &str = "http://127.0.0.1:23119/api/users/0/items";

/// Un item de la biblioteca de Zotero.
#[derive(Debug, Clone)]
pub struct ItemZotero {
    pub key: String,
    pub item_type: String,
    pub title: String,
    pub creators: Vec<String>,
    pub date: Option<String>,
    pub doi: Option<String>,
}

/// Consulta la biblioteca local de Zotero por texto.
///
/// Degradación elegante: si Zotero no responde (no está abierto), devuelve un
/// error que describe el estado y permite seguir sin la bibliografía.
pub fn consultar(api_url: &str, query: &str) -> Result<Vec<ItemZotero>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|e| format!("Zotero: no se pudo crear el cliente: {e}"))?;
    let url = format!("{api_url}?q={}", urlencode(query));
    let response = client.get(&url).send().map_err(|e| {
        format!(
            "Zotero no está abierto ({e}). Degradación elegante: se continúa sin bibliografía de Zotero."
        )
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("Zotero devolvió el estado {status}."));
    }
    let texto = response
        .text()
        .map_err(|e| format!("Zotero: respuesta ilegible: {e}"))?;
    parse_items(&texto)
}

/// Parsea la respuesta JSON de la API de Zotero (`items`).
///
/// La API local devuelve un arreglo de items con `key` y `data` (título,
/// itemType, creators, date, DOI).
pub fn parse_items(json: &str) -> Result<Vec<ItemZotero>, String> {
    let valor: Value =
        serde_json::from_str(json).map_err(|e| format!("Zotero: JSON inválido: {e}"))?;
    let items = valor
        .as_array()
        .ok_or_else(|| "Zotero: la respuesta no es un arreglo de items.".to_string())?;
    let mut out = Vec::new();
    for item in items {
        let data = item.get("data").unwrap_or(item);
        let title = data["title"].as_str().unwrap_or("").to_string();
        let item_type = data["itemType"].as_str().unwrap_or("").to_string();
        let key = item["key"].as_str().unwrap_or("").to_string();
        if title.is_empty() && key.is_empty() {
            continue;
        }
        let creators: Vec<String> = data["creators"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|c| {
                        let nombre = c["firstName"].as_str().unwrap_or("");
                        let apellido = c["lastName"].as_str().unwrap_or("");
                        let completo = format!("{} {}", nombre, apellido).trim().to_string();
                        if completo.is_empty() {
                            None
                        } else {
                            Some(completo)
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push(ItemZotero {
            key,
            item_type,
            title,
            creators,
            date: data["date"].as_str().map(|s| s.to_string()),
            doi: data["DOI"].as_str().map(|s| s.to_string()),
        });
    }
    Ok(out)
}

/// Percent-encode mínimo para el query de la URL (espacios y caracteres de
/// riesgo).
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
    fn parsea_items_de_la_api_local() {
        let json = r#"[
          {"key":"ABCD1234","data":{"itemType":"journalArticle","title":"La pesca en Mar del Plata","creators":[{"firstName":"Ana","lastName":"González"},{"lastName":"Pérez"}],"date":"1985-04-01","DOI":"10.1000/xyz"}},
          {"key":"EFGH5678","data":{"itemType":"book","title":"Historia del SOIP","creators":[],"date":"1990"}}
        ]"#;
        let items = parse_items(json).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].key, "ABCD1234");
        assert_eq!(items[0].title, "La pesca en Mar del Plata");
        assert_eq!(items[0].creators, vec!["Ana González", " Pérez".trim()]);
        assert_eq!(items[0].doi.as_deref(), Some("10.1000/xyz"));
        assert_eq!(items[1].item_type, "book");
    }

    #[test]
    fn json_invalido_devuelve_error() {
        assert!(parse_items("no es json").is_err());
    }

    #[test]
    fn degradacion_elegante_cuando_zotero_no_esta_abierto() {
        // Puerto 9 (discard): la conexión se rechaza rápido.
        let err = consultar("http://127.0.0.1:9/api/users/0/items", "huelga").unwrap_err();
        assert!(
            err.contains("Zotero no está abierto"),
            "el error debe describir la degradación: {err}"
        );
    }
}

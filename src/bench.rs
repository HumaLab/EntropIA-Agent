//! EntropIA-Bench (PLAN, transversal desde Fase 2): banco de preguntas
//! ancladas al corpus SOIP y **cadena de atribución de fallos**.
//!
//! La cadena distingue «no recuperé el documento» de «razoné mal sobre el que
//! recuperé» — y, con 61 % de items sin procesar, de «el documento no existía
//! para mí». Las preguntas se anclan solo a items con chunks: preguntar por
//! material no procesado mide la brecha de Lite/Pro, no al agente.

use serde::Deserialize;

/// Una pregunta del banco, anclada a evidencia procesada.
#[derive(Debug, Clone, Deserialize)]
pub struct PreguntaBench {
    pub id: String,
    pub nivel: u8,
    pub tipo: String,
    pub pregunta: String,
    pub items_esperados: Vec<String>,
    pub chunk_ids_esperados: Vec<String>,
    pub respuesta_referencia: String,
}

/// Banco de preguntas (archivo JSON).
#[derive(Debug, Clone, Deserialize)]
pub struct BancoBench {
    pub banco: String,
    pub preguntas: Vec<PreguntaBench>,
}

/// Carga el banco desde un archivo JSON.
pub fn cargar_banco(path: &str) -> Result<BancoBench, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("Banco ilegible: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("Banco inválido: {e}"))
}

/// Cadena de atribución de fallos para una respuesta del agente.
///
/// Encabeza la cobertura: si el item esperado no tenía chunks, el fallo es de
/// Lite/Pro (o del recorte), no del agente.
#[derive(Debug, Clone, Default)]
pub struct CadenaAtribucion {
    pub pregunta_id: String,
    /// Fracción de items esperados que tienen chunks (lo que el agente podía ver).
    pub cobertura_items_esperados: f64,
    /// Fracción de chunks esperados que recupera el gateway (pierna léxica).
    pub retrieval_recall: f64,
    /// Fracción de chunks esperados citados en la respuesta.
    pub evidence_recall: f64,
    /// Fracción de citas de la respuesta que están entre los esperados.
    pub citation_precision: f64,
    /// Fracción de claims supported por el Verifier (opcional).
    pub claim_support: Option<f64>,
    /// Calidad de respuesta por LLM-as-judge (opcional, no determinista).
    pub answer_quality: Option<f64>,
}

impl CadenaAtribucion {
    /// Completa con el soporte de claims medido por el Verifier.
    pub fn con_soporte(mut self, soporte: f64) -> Self {
        self.claim_support = Some(soporte);
        self
    }
}

/// Evalúa la respuesta del agente contra la pregunta y la evidencia esperada.
pub fn evaluar(
    repo: &crate::repositorio::RepositorioSqlite,
    pregunta: &PreguntaBench,
    chunks_citados: &[String],
) -> CadenaAtribucion {
    // Cobertura: ¿los items esperados tienen chunks?
    let con_chunks = pregunta
        .items_esperados
        .iter()
        .filter(|id| item_tiene_chunks(repo, id))
        .count();
    let cobertura = if pregunta.items_esperados.is_empty() {
        1.0 // sin items esperados, la cobertura no acota el juicio
    } else {
        con_chunks as f64 / pregunta.items_esperados.len() as f64
    };

    // Retrieval: pierna léxica del gateway sobre el texto de la pregunta.
    let recuperados = repo.buscar_fts5(&pregunta.pregunta, 50);
    let retrieval_recall = recall(&pregunta.chunk_ids_esperados, &recuperados);

    // Evidence recall y citation precision sobre las citas de la respuesta.
    let evidence_recall = recall(&pregunta.chunk_ids_esperados, chunks_citados);
    let citation_precision = if chunks_citados.is_empty() {
        0.0
    } else {
        let esperados: std::collections::HashSet<&String> =
            pregunta.chunk_ids_esperados.iter().collect();
        let citas_validas = chunks_citados
            .iter()
            .filter(|c| esperados.contains(c))
            .count();
        citas_validas as f64 / chunks_citados.len() as f64
    };

    CadenaAtribucion {
        pregunta_id: pregunta.id.clone(),
        cobertura_items_esperados: cobertura,
        retrieval_recall,
        evidence_recall,
        citation_precision,
        claim_support: None,
        answer_quality: None,
    }
}

fn recall(esperados: &[String], obtenidos: &[String]) -> f64 {
    if esperados.is_empty() {
        return 1.0; // sin esperados, no hay recall que medir
    }
    let set: std::collections::HashSet<&String> = obtenidos.iter().collect();
    let encontrados = esperados.iter().filter(|e| set.contains(e)).count();
    encontrados as f64 / esperados.len() as f64
}

/// ¿El item tiene al menos un chunk en el corpus (recorte real)?
pub fn item_tiene_chunks(repo: &crate::repositorio::RepositorioSqlite, item_id: &str) -> bool {
    let Ok(mut stmt) =
        repo.prepare_pub("SELECT EXISTS(SELECT 1 FROM rag_chunks WHERE item_id = ?1)")
    else {
        return false;
    };
    stmt.query_row(rusqlite::params![item_id], |r| r.get::<_, bool>(0))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repositorio::RepositorioSqlite;

    fn repo() -> RepositorioSqlite {
        RepositorioSqlite::abrir(crate::tests_comunes::corpus_sintetico().to_str().unwrap())
            .unwrap()
    }

    fn pregunta(chunk_ids: Vec<String>) -> PreguntaBench {
        PreguntaBench {
            id: "test-1".into(),
            nivel: 2,
            tipo: "contenido".into(),
            pregunta: "¿Qué dice el fragmento?".into(),
            items_esperados: vec!["item-1".into()],
            chunk_ids_esperados: chunk_ids,
            respuesta_referencia: "x".into(),
        }
    }

    #[test]
    fn el_banco_real_carga_y_tiene_tres_niveles() {
        let banco = cargar_banco("bench/preguntas.json").expect("el banco debe existir en el repo");
        assert_eq!(banco.banco, "entropia-bench-v1");
        assert!(banco.preguntas.len() >= 10);
        let niveles: std::collections::HashSet<u8> =
            banco.preguntas.iter().map(|p| p.nivel).collect();
        assert!(niveles.contains(&1) && niveles.contains(&2) && niveles.contains(&3));
    }

    #[test]
    fn la_cadena_mide_recall_y_precision_de_citas() {
        let r = repo();
        let p = pregunta(vec!["chunk-1".into(), "chunk-2".into()]);
        // La respuesta cita chunk-1 (correcto) y chunk-stress-1 (de prueba, no
        // esperado): evidence_recall 0.5, citation_precision 0.5.
        let cadena = evaluar(
            &r,
            &p,
            &["chunk-1".to_string(), "chunk-stress-1".to_string()],
        );
        assert_eq!(cadena.evidence_recall, 0.5);
        assert_eq!(cadena.citation_precision, 0.5);
        assert_eq!(cadena.cobertura_items_esperados, 1.0); // item-1 tiene chunks
    }

    #[test]
    fn la_cobertura_distingue_el_fallo_de_brecha() {
        let r = repo();
        let mut p = pregunta(vec![]);
        p.items_esperados = vec!["item-3".into()]; // sin chunks en el corpus sintético
        let cadena = evaluar(&r, &p, &[]);
        assert_eq!(cadena.cobertura_items_esperados, 0.0);
        // Sin esperados de chunk, recall no penaliza.
        assert_eq!(cadena.retrieval_recall, 1.0);
    }

    #[test]
    fn las_citas_de_colecciones_excluidas_no_cuentan_como_precision() {
        let r = repo();
        let p = pregunta(vec!["chunk-1".into()]);
        let cadena = evaluar(&r, &p, &["chunk-stress-1".to_string()]);
        assert_eq!(cadena.evidence_recall, 0.0);
        assert_eq!(cadena.citation_precision, 0.0);
    }
}

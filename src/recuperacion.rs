//! Recuperación híbrida por consulta (embeddings + FTS5 + RRF + rerank).
//!
//! Réplica del pipeline del desktop de EntropIA: para una consulta del agente,
//! combina búsqueda semántica (coseno sobre embeddings BGE-M3) con búsqueda
//! léxica (FTS5 con BM25), fusiona por Reciprocal Rank Fusion y reranquea con
//! cohere/rerank-4-fast.

use std::collections::HashMap;

use crate::embeddings::ClienteEmbeddings;
use crate::repositorio::{ChunkRag, Fuente, RepositorioSqlite};
use crate::rerank::ClienteRerank;
use crate::vector;

/// Candidatos por pierna de recuperación.
const LEG_K: usize = 24;
/// Candidatos que entran al rerank.
const RERANK_DEPTH: usize = 16;
/// Constante de suavizado de RRF.
const RRF_K: usize = 60;
/// Tope de caracteres por fragmento.
const SNIPPET_MAX: usize = 1600;

/// Orquestador de recuperación híbrida.
pub struct Recuperador {
    embeddings: ClienteEmbeddings,
    rerank: ClienteRerank,
}

impl Recuperador {
    pub fn new(embeddings: ClienteEmbeddings, rerank: ClienteRerank) -> Self {
        Self { embeddings, rerank }
    }

    /// Recupera los fragmentos más relevantes para una consulta del agente.
    pub fn recuperar_consulta(
        &self,
        repo: &RepositorioSqlite,
        consulta: &str,
        limite: usize,
    ) -> Vec<Fuente> {
        let Ok(chunks) = repo.cargar_chunks() else {
            return Vec::new();
        };
        if chunks.is_empty() {
            return Vec::new();
        }
        let indice: HashMap<String, usize> = chunks
            .iter()
            .enumerate()
            .map(|(i, c)| (c.id.clone(), i))
            .collect();

        let Some(q_emb) = self.embeddings.embed(consulta).ok() else {
            return Vec::new();
        };
        let vectorial = knn(&chunks, &q_emb);
        let lexical_ids = repo.buscar_fts5(consulta, LEG_K);
        let lexical: Vec<usize> = lexical_ids
            .iter()
            .filter_map(|id| indice.get(id).copied())
            .collect();
        let top: Vec<usize> = rrf_fuse(&vectorial, &lexical)
            .into_iter()
            .take(RERANK_DEPTH)
            .map(|(i, _)| i)
            .collect();
        if top.is_empty() {
            return Vec::new();
        }

        let documentos: Vec<String> = top
            .iter()
            .map(|&i| snippet(&chunks[i].text_content, SNIPPET_MAX))
            .collect();
        let orden = self
            .rerank
            .rerank(consulta, &documentos, limite)
            .unwrap_or_else(|_| (0..top.len()).map(|i| (i, 0.0)).collect());

        let mut fuentes = Vec::new();
        for (local, _) in orden.into_iter().take(limite) {
            if let Some(&idx) = top.get(local) {
                let c = &chunks[idx];
                fuentes.push(Fuente {
                    id: c.id.clone(),
                    titulo: c.item_titulo.clone(),
                    coleccion: c.coleccion.clone(),
                    contenido: snippet(&c.text_content, SNIPPET_MAX),
                });
            }
        }
        fuentes
    }
}

/// kNN por similitud coseno: índices de los LEG_K chunks más cercanos.
fn knn(chunks: &[ChunkRag], q_emb: &[f32]) -> Vec<usize> {
    let mut sim: Vec<(usize, f32)> = chunks
        .iter()
        .enumerate()
        .map(|(i, c)| (i, vector::similitud_coseno(q_emb, &c.embedding)))
        .collect();
    sim.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    sim.into_iter().take(LEG_K).map(|(i, _)| i).collect()
}

/// Fusión por Reciprocal Rank Fusion de dos listas de índices rankeadas.
fn rrf_fuse(vectorial: &[usize], lexical: &[usize]) -> Vec<(usize, f64)> {
    let mut scores: HashMap<usize, f64> = HashMap::new();
    for (rank, &idx) in vectorial.iter().enumerate() {
        *scores.entry(idx).or_insert(0.0) += 1.0 / (RRF_K as f64 + rank as f64 + 1.0);
    }
    for (rank, &idx) in lexical.iter().enumerate() {
        *scores.entry(idx).or_insert(0.0) += 1.0 / (RRF_K as f64 + rank as f64 + 1.0);
    }
    let mut v: Vec<(usize, f64)> = scores.into_iter().collect();
    v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    v
}

fn snippet(texto: &str, max: usize) -> String {
    let recortado: String = texto.chars().take(max).collect();
    if recortado.chars().count() < texto.chars().count() {
        format!("{recortado}[...]")
    } else {
        recortado
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repositorio::ChunkRag;

    #[test]
    fn rrf_suma_puntaje_si_un_chunk_aparece_en_ambas_piernas() {
        let vec_leg = vec![0, 1];
        let lex_leg = vec![1, 2];
        let fusion = rrf_fuse(&vec_leg, &lex_leg);
        let por_idx: HashMap<usize, f64> = fusion.iter().cloned().collect();
        assert!(por_idx[&1] > por_idx[&0]);
        assert!(por_idx[&1] > por_idx[&2]);
    }

    #[test]
    fn rrf_ordena_de_mayor_a_menor() {
        let fusion = rrf_fuse(&[5, 7], &[]);
        let scores: Vec<f64> = fusion.iter().map(|(_, s)| *s).collect();
        assert!(scores.windows(2).all(|w| w[0] >= w[1]));
    }

    #[test]
    fn snippet_trunca_y_agrega_marcador() {
        let s = snippet("abcdefghij", 5);
        assert!(s.starts_with("abcde"));
        assert!(s.ends_with("[...]"));
    }

    #[test]
    fn snippet_corto_sin_marcador() {
        assert_eq!(snippet("abc", 10), "abc");
    }

    #[test]
    fn knn_ordena_los_chunks_por_cercania() {
        let chunks = vec![
            ChunkRag {
                id: "a".into(),
                item_titulo: "".into(),
                coleccion: "".into(),
                text_content: "".into(),
                embedding: vec![1.0, 0.0],
            },
            ChunkRag {
                id: "b".into(),
                item_titulo: "".into(),
                coleccion: "".into(),
                text_content: "".into(),
                embedding: vec![0.0, 1.0],
            },
            ChunkRag {
                id: "c".into(),
                item_titulo: "".into(),
                coleccion: "".into(),
                text_content: "".into(),
                embedding: vec![0.9, 0.1],
            },
        ];
        let cerca = knn(&chunks, &[1.0, 0.0]);
        assert_eq!(cerca.len(), 3);
        assert_eq!(cerca[0], 0);
        assert_eq!(cerca[1], 2);
    }
}

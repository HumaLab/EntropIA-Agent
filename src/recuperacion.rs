//! Recuperación híbrida por consulta (embeddings + FTS5 + RRF + rerank).
//!
//! Réplica del pipeline del desktop de EntropIA: para una consulta del agente,
//! combina búsqueda semántica (coseno sobre embeddings BGE-M3) con búsqueda
//! léxica (FTS5 con BM25), fusiona por Reciprocal Rank Fusion y reranquea con
//! cohere/rerank-4-fast.
//!
//! Fase 0 (PLAN §9): los chunks se cargan **una vez por proceso** (antes
//! `cargar_chunks()` movía 7,6 MB y 1.648 decodificaciones `Vec<f32>` en cada
//! `buscar_fuentes`), y `limite` se acota explícitamente a `RERANK_DEPTH` en
//! vez de topar en silencio.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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

/// Corpus cargado y su índice id → posición (ver `CacheChunks`).
type CorpusCargado = (Arc<Vec<ChunkRag>>, Arc<HashMap<String, usize>>);

/// Caché de chunks del proceso: carga el corpus una única vez y lo reutiliza
/// entre consultas (PLAN §9, Fase 0). Los `Arc` hacen que cada consulta pague
/// dos clonados de puntero, no 7,6 MB de copia.
pub struct CacheChunks {
    chunks: Option<Arc<Vec<ChunkRag>>>,
    indice: Option<Arc<HashMap<String, usize>>>,
}

impl CacheChunks {
    pub fn nueva() -> Self {
        Self {
            chunks: None,
            indice: None,
        }
    }

    /// Devuelve los chunks (y su índice), cargándolos con `cargar` solo la
    /// primera vez. La carga ocurre bajo el mutex del `Recuperador`.
    pub fn obtener_o_cargar(
        &mut self,
        repo: &RepositorioSqlite,
        cargar: impl FnOnce(&RepositorioSqlite) -> Result<Vec<ChunkRag>, String>,
    ) -> Result<CorpusCargado, String> {
        if self.chunks.is_none() {
            let chunks = cargar(repo)?;
            let indice: HashMap<String, usize> = chunks
                .iter()
                .enumerate()
                .map(|(i, c)| (c.id.clone(), i))
                .collect();
            self.chunks = Some(Arc::new(chunks));
            self.indice = Some(Arc::new(indice));
        }
        let chunks = self
            .chunks
            .clone()
            .ok_or_else(|| "Sin chunks".to_string())?;
        let indice = self
            .indice
            .clone()
            .ok_or_else(|| "Sin índice".to_string())?;
        Ok((chunks, indice))
    }
}

/// Orquestador de recuperación híbrida.
pub struct Recuperador {
    embeddings: ClienteEmbeddings,
    rerank: ClienteRerank,
    cache: Mutex<CacheChunks>,
}

impl Recuperador {
    pub fn new(embeddings: ClienteEmbeddings, rerank: ClienteRerank) -> Self {
        Self {
            embeddings,
            rerank,
            cache: Mutex::new(CacheChunks::nueva()),
        }
    }

    /// Recupera los fragmentos más relevantes para una consulta del agente.
    ///
    /// El `limite` se acota a `RERANK_DEPTH` de forma explícita: es la
    /// profundidad real del pipeline (candidatos que entran al rerank).
    pub fn recuperar_consulta(
        &self,
        repo: &RepositorioSqlite,
        consulta: &str,
        limite: usize,
    ) -> Vec<Fuente> {
        let limite = limite.min(RERANK_DEPTH);
        let (chunks, indice) = {
            let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
            match cache.obtener_o_cargar(repo, |r| r.cargar_chunks()) {
                Ok(par) => par,
                Err(_) => return Vec::new(),
            }
        };
        if chunks.is_empty() {
            return Vec::new();
        }

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

    #[test]
    fn cache_carga_una_sola_vez_por_proceso() {
        let mut cache = CacheChunks::nueva();
        let llamadas = std::cell::Cell::new(0);
        let cargar = |_repo: &RepositorioSqlite| {
            llamadas.set(llamadas.get() + 1);
            Ok(Vec::<ChunkRag>::new())
        };
        let repo = repo_fantasma();
        let _ = cache.obtener_o_cargar(&repo, cargar);
        let _ = cache.obtener_o_cargar(&repo, cargar);
        assert_eq!(llamadas.get(), 1);
    }

    fn repo_fantasma() -> RepositorioSqlite {
        // Base vacía en un archivo temporal: el cierre de carga no la consulta.
        let path =
            std::env::temp_dir().join(format!("entropia-cache-test-{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let _ = rusqlite::Connection::open(&path).unwrap();
        RepositorioSqlite::abrir_con_denylist(path.to_str().unwrap(), vec![]).unwrap()
    }
}

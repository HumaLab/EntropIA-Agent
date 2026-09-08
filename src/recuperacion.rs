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

/// Fuente del vector de la consulta.
///
/// Es un trait y no el cliente concreto para que la recuperación se pueda
/// probar sin red: una pierna semántica que solo se ejercita contra OpenRouter
/// no se ejercita nunca.
pub trait Embedder {
    fn embed(&self, texto: &str) -> Result<Vec<f32>, String>;
}

impl Embedder for ClienteEmbeddings {
    fn embed(&self, texto: &str) -> Result<Vec<f32>, String> {
        ClienteEmbeddings::embed(self, texto)
    }
}

/// Reordenamiento de candidatos por relevancia. Trait por el mismo motivo que
/// `Embedder`: un test que sale a la red no es hermético.
pub trait Reranker {
    fn rerank(
        &self,
        consulta: &str,
        documentos: &[String],
        limite: usize,
    ) -> Result<Vec<(usize, f64)>, String>;
}

impl Reranker for ClienteRerank {
    fn rerank(
        &self,
        consulta: &str,
        documentos: &[String],
        limite: usize,
    ) -> Result<Vec<(usize, f64)>, String> {
        ClienteRerank::rerank(self, consulta, documentos, limite)
    }
}

/// Fragmento recuperado con su identidad completa y su **texto entero**.
///
/// El texto no se recorta acá a propósito: quien cite decide el recorte. Un
/// snippet ya truncado con marcador no se puede reproducir como cita literal,
/// porque la cadena resultante no existe en la fuente.
#[derive(Clone, Debug)]
pub struct FragmentoRecuperado {
    pub chunk_id: String,
    pub item_id: String,
    pub item_titulo: String,
    pub collection_id: String,
    pub coleccion: String,
    pub asset_id: String,
    pub texto: String,
    pub start: i64,
    pub end: i64,
}

/// Resultado de una recuperación. La degradación se devuelve, no se esconde:
/// un informe armado sobre búsqueda léxica sola tiene que poder declararlo.
#[derive(Debug, Default)]
pub struct Recuperacion {
    pub fragmentos: Vec<FragmentoRecuperado>,
    /// `None` cuando corrieron las dos piernas y el rerank.
    pub degradacion: Option<String>,
}

impl Recuperacion {
    fn cortada(motivo: String) -> Self {
        Self {
            fragmentos: Vec::new(),
            degradacion: Some(motivo),
        }
    }
}

/// Orquestador de recuperación híbrida.
pub struct Recuperador {
    embeddings: Box<dyn Embedder + Send + Sync>,
    rerank: Box<dyn Reranker + Send + Sync>,
    cache: Mutex<CacheChunks>,
}

// El recuperador se comparte entre hilos: el desktop lo construye una vez por
// credencial y lo usa desde el conductor de jobs. Los objetos de trait le
// habían sacado esa propiedad en silencio —los clientes concretos sí la
// tenían— y el consumidor se enteró al compilar. Esta aserción lo fija.
const _: fn() = || {
    fn compartible<T: Send + Sync>() {}
    compartible::<Recuperador>();
};

impl Recuperador {
    pub fn new(embeddings: ClienteEmbeddings, rerank: ClienteRerank) -> Self {
        Self::con_clientes(Box::new(embeddings), Box::new(rerank))
    }

    /// Constructor por trait: permite ejercitar la recuperación completa sin
    /// red, y sustituir proveedores sin tocar el pipeline.
    pub fn con_clientes(
        embeddings: Box<dyn Embedder + Send + Sync>,
        rerank: Box<dyn Reranker + Send + Sync>,
    ) -> Self {
        Self {
            embeddings,
            rerank,
            cache: Mutex::new(CacheChunks::nueva()),
        }
    }

    /// Recupera fragmentos para una consulta, **acotado al recorte del job**.
    ///
    /// El recorte no es una preferencia: es parte del contrato de la
    /// investigación, que declaró su cobertura y congeló su snapshot. Traer
    /// evidencia de una colección que el investigador no eligió invalida el
    /// informe aunque la cita sea literal.
    ///
    /// `colecciones` vacío significa sin filtro.
    pub fn recuperar_en_colecciones(
        &self,
        repo: &RepositorioSqlite,
        consulta: &str,
        colecciones: &[String],
        limite: usize,
    ) -> Recuperacion {
        let limite = limite.clamp(1, RERANK_DEPTH);
        let (chunks, indice) = {
            let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
            match cache.obtener_o_cargar(repo, |r| r.cargar_chunks()) {
                Ok(par) => par,
                Err(e) => {
                    return Recuperacion::cortada(format!("no se pudo cargar el corpus: {e}"))
                }
            }
        };
        let permitidos: Vec<usize> = chunks
            .iter()
            .enumerate()
            .filter(|(_, c)| colecciones.is_empty() || colecciones.contains(&c.collection_id))
            .map(|(i, _)| i)
            .collect();
        if permitidos.is_empty() {
            return Recuperacion::cortada(
                "el recorte seleccionado no tiene fragmentos cargados".into(),
            );
        }
        let mut degradaciones: Vec<String> = Vec::new();

        let vectorial = match self.embeddings.embed(consulta) {
            Ok(q) => knn_en(&chunks, &permitidos, &q),
            Err(e) => {
                degradaciones.push(format!(
                    "recuperación sin pierna semántica ({e}): solo búsqueda léxica, \
                     los documentos que no coinciden por vocabulario quedan fuera"
                ));
                Vec::new()
            }
        };

        // La pierna léxica se pide ancha y después se acota al recorte: filtrar
        // un top-K global dejaría casi nada cuando el recorte es chico.
        let en_recorte: std::collections::HashSet<usize> = permitidos.iter().copied().collect();
        let lexical: Vec<usize> = repo
            .buscar_fts5(consulta, LEG_K * 8)
            .iter()
            .filter_map(|id| indice.get(id).copied())
            .filter(|i| en_recorte.contains(i))
            .take(LEG_K)
            .collect();

        let top: Vec<usize> = rrf_fuse(&vectorial, &lexical)
            .into_iter()
            .take(RERANK_DEPTH)
            .map(|(i, _)| i)
            .collect();
        if top.is_empty() {
            return Recuperacion {
                fragmentos: Vec::new(),
                degradacion: degradaciones.first().cloned(),
            };
        }

        let documentos: Vec<String> = top
            .iter()
            .map(|&i| snippet(&chunks[i].text_content, SNIPPET_MAX))
            .collect();
        let orden = match self.rerank.rerank(consulta, &documentos, limite) {
            Ok(orden) => orden,
            Err(e) => {
                degradaciones.push(format!(
                    "resultados sin rerank ({e}): se conserva el orden de la fusión"
                ));
                (0..top.len()).map(|i| (i, 0.0)).collect()
            }
        };

        let fragmentos = orden
            .into_iter()
            .take(limite)
            .filter_map(|(local, _)| top.get(local).map(|&i| &chunks[i]))
            .map(|c| FragmentoRecuperado {
                chunk_id: c.id.clone(),
                item_id: c.item_id.clone(),
                item_titulo: c.item_titulo.clone(),
                collection_id: c.collection_id.clone(),
                coleccion: c.coleccion.clone(),
                asset_id: c.asset_id.clone(),
                texto: c.text_content.clone(),
                start: c.start_char,
                end: c.end_char,
            })
            .collect();
        Recuperacion {
            fragmentos,
            degradacion: if degradaciones.is_empty() {
                None
            } else {
                Some(degradaciones.join("; "))
            },
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

/// kNN por similitud coseno restringido a los índices habilitados.
fn knn_en(chunks: &[ChunkRag], permitidos: &[usize], q_emb: &[f32]) -> Vec<usize> {
    let mut sim: Vec<(usize, f32)> = permitidos
        .iter()
        .map(|&i| (i, vector::similitud_coseno(q_emb, &chunks[i].embedding)))
        .collect();
    sim.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    sim.into_iter().take(LEG_K).map(|(i, _)| i).collect()
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

    fn chunk(id: &str, coleccion: &str, embedding: Vec<f32>) -> ChunkRag {
        ChunkRag {
            id: id.into(),
            item_id: format!("item-{id}"),
            item_titulo: format!("título de {id}"),
            collection_id: coleccion.into(),
            coleccion: coleccion.into(),
            asset_id: format!("asset-{id}"),
            text_content: format!("texto de {id}"),
            start_char: 0,
            end_char: 10,
            embedding,
        }
    }

    #[test]
    fn knn_ordena_los_chunks_por_cercania() {
        let chunks = vec![
            chunk("a", "c1", vec![1.0, 0.0]),
            chunk("b", "c1", vec![0.0, 1.0]),
            chunk("c", "c1", vec![0.9, 0.1]),
        ];
        let cerca = knn(&chunks, &[1.0, 0.0]);
        assert_eq!(cerca.len(), 3);
        assert_eq!(cerca[0], 0);
        assert_eq!(cerca[1], 2);
    }

    #[test]
    fn knn_en_ignora_lo_que_esta_fuera_del_recorte() {
        let chunks = vec![
            chunk("a", "c1", vec![1.0, 0.0]),
            chunk("b", "c2", vec![1.0, 0.0]),
            chunk("c", "c1", vec![0.9, 0.1]),
        ];
        // El más cercano de todos es `a`, pero si el recorte solo habilita el
        // índice 2, `a` no puede aparecer.
        let cerca = knn_en(&chunks, &[2], &[1.0, 0.0]);
        assert_eq!(cerca, vec![2]);
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

    /// Embedder de prueba: devuelve el vector fijo, o falla si se lo pide.
    struct EmbedderFalso {
        vector: Vec<f32>,
        falla: bool,
    }
    impl Embedder for EmbedderFalso {
        fn embed(&self, _: &str) -> Result<Vec<f32>, String> {
            if self.falla {
                Err("sin clave de API".into())
            } else {
                Ok(self.vector.clone())
            }
        }
    }

    /// Reranker de prueba: conserva el orden que recibe.
    struct RerankIdentidad {
        falla: bool,
    }
    impl Reranker for RerankIdentidad {
        fn rerank(
            &self,
            _: &str,
            docs: &[String],
            limite: usize,
        ) -> Result<Vec<(usize, f64)>, String> {
            if self.falla {
                return Err("sin clave de API".into());
            }
            Ok((0..docs.len().min(limite)).map(|i| (i, 1.0_f64)).collect())
        }
    }

    fn recuperador(embed_falla: bool, rerank_falla: bool) -> Recuperador {
        Recuperador::con_clientes(
            Box::new(EmbedderFalso {
                vector: vec![1.0, 0.0],
                falla: embed_falla,
            }),
            Box::new(RerankIdentidad {
                falla: rerank_falla,
            }),
        )
    }

    /// Corpus completo, sin denylist: así conviven colecciones reales y de
    /// prueba y el filtro por recorte se puede verificar de verdad.
    fn repo_completo() -> RepositorioSqlite {
        RepositorioSqlite::abrir_con_denylist(
            crate::tests_comunes::corpus_sintetico().to_str().unwrap(),
            vec![],
        )
        .unwrap()
    }

    #[test]
    fn la_recuperacion_se_acota_al_recorte_del_job() {
        let repo = repo_completo();
        let r = recuperador(false, false);
        let salida = r.recuperar_en_colecciones(&repo, "huelga", &["c-conflicto".into()], 16);
        assert!(!salida.fragmentos.is_empty(), "{salida:?}");
        for f in &salida.fragmentos {
            assert_eq!(
                f.collection_id, "c-conflicto",
                "«{}» está fuera del recorte declarado",
                f.chunk_id
            );
        }
        // `chunk-stress-1` también dice «huelga», pero vive en otra colección.
        assert!(!salida
            .fragmentos
            .iter()
            .any(|f| f.chunk_id.starts_with("chunk-stress")));
    }

    #[test]
    fn un_recorte_sin_material_no_devuelve_evidencia_ajena() {
        let repo = repo_completo();
        let r = recuperador(false, false);
        let salida = r.recuperar_en_colecciones(&repo, "huelga", &["c-inexistente".into()], 16);
        assert!(salida.fragmentos.is_empty());
        assert!(salida.degradacion.unwrap().contains("no tiene fragmentos"));
    }

    #[test]
    fn sin_pierna_semantica_cae_a_lexica_y_lo_declara() {
        let repo = repo_completo();
        let r = recuperador(true, false);
        let salida = r.recuperar_en_colecciones(&repo, "huelga", &["c-conflicto".into()], 16);
        // La búsqueda léxica sigue trayendo material: la degradación no es un
        // corte, es una pérdida de alcance que hay que declarar.
        assert!(!salida.fragmentos.is_empty());
        let motivo = salida.degradacion.expect("la degradación tiene que viajar");
        assert!(motivo.contains("sin pierna semántica"), "{motivo}");
    }

    #[test]
    fn sin_rerank_se_conserva_el_orden_de_fusion_y_se_declara() {
        let repo = repo_completo();
        let r = recuperador(false, true);
        let salida = r.recuperar_en_colecciones(&repo, "huelga", &["c-conflicto".into()], 16);
        assert!(!salida.fragmentos.is_empty());
        assert!(salida.degradacion.unwrap().contains("sin rerank"));
    }

    #[test]
    fn el_pipeline_completo_no_declara_degradacion() {
        let repo = repo_completo();
        let r = recuperador(false, false);
        let salida = r.recuperar_en_colecciones(&repo, "huelga", &["c-conflicto".into()], 16);
        assert_eq!(salida.degradacion, None, "{salida:?}");
    }

    #[test]
    fn el_fragmento_llega_entero_con_su_identidad_y_offsets() {
        let repo = repo_completo();
        let r = recuperador(false, false);
        let salida = r.recuperar_en_colecciones(&repo, "huelga", &["c-conflicto".into()], 16);
        let f = salida
            .fragmentos
            .iter()
            .find(|f| f.chunk_id == "chunk-1")
            .expect("chunk-1 responde a «huelga»");
        // Sin recorte ni marcador: una cita literal sobre un snippet truncado
        // reproduce una cadena que no existe en la fuente.
        assert_eq!(f.texto, "huelga general de la pesca en marzo");
        assert!(!f.texto.contains("[...]"));
        assert_eq!(f.item_id, "item-1");
        assert_eq!(f.item_titulo, "65-03-17-a");
        assert_eq!(f.coleccion, "Conflicto SOIP 1965-66");
        assert_eq!(f.asset_id, "chunk-1");
        assert_eq!((f.start, f.end), (0, 100));
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

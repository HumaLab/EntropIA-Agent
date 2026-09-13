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
pub const LEG_K: usize = 24;
/// Candidatos que entran al rerank. Es también el techo del `retrieval_limit`
/// de un plan: la recuperación no entrega más fragmentos por consulta.
pub const RERANK_DEPTH: usize = 16;
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

/// Llamadas a servicios externos que hizo una recuperación. Se informan, nunca
/// se descuentan del presupuesto de llamadas al modelo.
///
/// Cuenta envíos, no éxitos: un cliente que devuelve error igual recibió la
/// llamada y puede facturarla.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LlamadasRecuperacion {
    pub embeddings: usize,
    pub rerank: usize,
}

/// Resultado de una recuperación. La degradación se devuelve, no se esconde:
/// un informe armado sobre búsqueda léxica sola tiene que poder declararlo.
#[derive(Debug, Default)]
pub struct Recuperacion {
    pub fragmentos: Vec<FragmentoRecuperado>,
    /// `None` cuando corrieron las dos piernas y el rerank.
    pub degradacion: Option<String>,
    /// Llamadas externas que hizo esta consulta.
    pub llamadas: LlamadasRecuperacion,
}

impl Recuperacion {
    /// Salida temprana: no llega a ningún cliente externo.
    fn cortada(motivo: String) -> Self {
        Self {
            fragmentos: Vec::new(),
            degradacion: Some(motivo),
            llamadas: LlamadasRecuperacion::default(),
        }
    }
}

/// Rankings de una consulta por pierna y de su fusión, en ids de chunk y en
/// orden de posición (ver `Recuperador::ranking_diagnostico`).
#[derive(Debug, Default)]
pub struct RankingDiagnostico {
    pub vectorial: Vec<String>,
    pub lexica: Vec<String>,
    pub fusion: Vec<String>,
    /// Fusión tal como la arma el flujo: piernas cortadas en `LEG_K`, sin
    /// truncar (a lo sumo 2·`LEG_K`), sea cual sea `profundidad`. Sus primeros
    /// `RERANK_DEPTH` son los candidatos que el flujo manda al rerank.
    pub fusion_flujo: Vec<String>,
    /// Llamadas externas que hizo el diagnóstico.
    pub llamadas: LlamadasRecuperacion,
    /// `None` cuando corrieron las dos piernas: un ranking solo léxico tiene
    /// que declararlo.
    pub degradacion: Option<String>,
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

        // El intento cuenta aunque falle: ver `LlamadasRecuperacion`.
        let mut llamadas = LlamadasRecuperacion {
            embeddings: 1,
            rerank: 0,
        };
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
                llamadas,
            };
        }

        let documentos: Vec<String> = top
            .iter()
            .map(|&i| snippet(&chunks[i].text_content, SNIPPET_MAX))
            .collect();
        llamadas.rerank = 1;
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
            llamadas,
            degradacion: if degradaciones.is_empty() {
                None
            } else {
                Some(degradaciones.join("; "))
            },
        }
    }

    /// Rankings de las dos piernas y de la fusión RRF, **sin rerank**, para
    /// diagnosticar en qué posición aparece la evidencia (calibración de
    /// `RERANK_DEPTH`). Solo lectura: no toca el flujo de producción.
    ///
    /// Hace una llamada de embeddings y ninguna de rerank: el rerank solo
    /// reordena el top de la fusión y no puede rescatar lo que no llegó.
    ///
    /// `vectorial`, `lexica` y `fusion` se cortan en `profundidad`. Con
    /// `profundidad > LEG_K` las piernas simulan subir `LEG_K`, y `fusion` deja
    /// de ser la del flujo aun en sus primeras posiciones: un chunk hondo en las
    /// dos piernas puede sumar más que uno alto en una sola. `fusion_flujo` es
    /// siempre la del flujo (piernas de `LEG_K`), sacada de las mismas piernas:
    /// sus primeros `RERANK_DEPTH` son exactamente los candidatos que
    /// `recuperar_en_colecciones` manda al rerank.
    pub fn ranking_diagnostico(
        &self,
        repo: &RepositorioSqlite,
        consulta: &str,
        colecciones: &[String],
        profundidad: usize,
    ) -> RankingDiagnostico {
        let (chunks, indice) = {
            let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
            match cache.obtener_o_cargar(repo, |r| r.cargar_chunks()) {
                Ok(par) => par,
                Err(e) => {
                    return RankingDiagnostico {
                        degradacion: Some(format!("no se pudo cargar el corpus: {e}")),
                        ..RankingDiagnostico::default()
                    }
                }
            }
        };
        let permitidos: Vec<usize> = chunks
            .iter()
            .enumerate()
            .filter(|(_, c)| colecciones.is_empty() || colecciones.contains(&c.collection_id))
            .map(|(i, _)| i)
            .collect();
        // Las piernas se miden al menos a LEG_K: la fusión del flujo las
        // necesita enteras aunque se pida una profundidad menor.
        let medida = profundidad.max(LEG_K);
        let mut degradacion = None;
        let vectorial_medida = match self.embeddings.embed(consulta) {
            Ok(q) => knn_en_hasta(&chunks, &permitidos, &q, medida),
            Err(e) => {
                degradacion = Some(format!(
                    "diagnóstico sin pierna semántica ({e}): solo búsqueda léxica"
                ));
                Vec::new()
            }
        };
        // Pedido ancho como el del flujo. Sus primeros `LEG_K * 8` crudos son
        // exactamente los que `recuperar_en_colecciones` filtra y corta en
        // `LEG_K`: de ahí sale la pierna léxica del flujo.
        let en_recorte: std::collections::HashSet<usize> = permitidos.iter().copied().collect();
        let crudos = repo.buscar_fts5(consulta, medida * 8);
        let lexica_hasta = |crudos: &[String], k: usize| -> Vec<usize> {
            crudos
                .iter()
                .filter_map(|id| indice.get(id).copied())
                .filter(|i| en_recorte.contains(i))
                .take(k)
                .collect()
        };
        let lexical_medida = lexica_hasta(&crudos, medida);
        let lexical_flujo = lexica_hasta(&crudos[..crudos.len().min(LEG_K * 8)], LEG_K);
        let fusion_flujo = fusion_del_flujo(&vectorial_medida, &lexical_flujo);

        let vectorial = &vectorial_medida[..vectorial_medida.len().min(profundidad)];
        let lexical = &lexical_medida[..lexical_medida.len().min(profundidad)];
        let fusion: Vec<usize> = rrf_fuse(vectorial, lexical)
            .into_iter()
            .take(profundidad)
            .map(|(i, _)| i)
            .collect();
        let ids = |orden: &[usize]| orden.iter().map(|&i| chunks[i].id.clone()).collect();
        RankingDiagnostico {
            vectorial: ids(vectorial),
            lexica: ids(lexical),
            fusion: ids(&fusion),
            fusion_flujo: ids(&fusion_flujo),
            llamadas: LlamadasRecuperacion {
                embeddings: 1,
                rerank: 0,
            },
            degradacion,
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
    knn_en_hasta(chunks, permitidos, q_emb, LEG_K)
}

/// `knn_en` con profundidad explícita: los `k` habilitados más cercanos.
fn knn_en_hasta(chunks: &[ChunkRag], permitidos: &[usize], q_emb: &[f32], k: usize) -> Vec<usize> {
    let mut sim: Vec<(usize, f32)> = permitidos
        .iter()
        .map(|&i| (i, vector::similitud_coseno(q_emb, &chunks[i].embedding)))
        .collect();
    sim.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    sim.into_iter().take(k).map(|(i, _)| i).collect()
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
///
/// El orden es total: puntaje descendente, después la mejor posición del chunk
/// en alguna pierna y por último el índice, que sigue el orden por id con que
/// `cargar_chunks` entrega el corpus. El orden de un HashMap cambia en cada
/// instancia, y un empate —frecuente: un chunk solo en una pierna empata con
/// otro solo en la otra a la misma posición— mandaría candidatos distintos al
/// rerank en cada corrida.
fn rrf_fuse(vectorial: &[usize], lexical: &[usize]) -> Vec<(usize, f64)> {
    // Por chunk: puntaje acumulado y mejor posición en alguna pierna.
    let mut scores: HashMap<usize, (f64, usize)> = HashMap::new();
    for pierna in [vectorial, lexical] {
        for (rank, &idx) in pierna.iter().enumerate() {
            let entrada = scores.entry(idx).or_insert((0.0, rank));
            entrada.0 += 1.0 / (RRF_K as f64 + rank as f64 + 1.0);
            entrada.1 = entrada.1.min(rank);
        }
    }
    let mut v: Vec<(usize, f64, usize)> = scores
        .into_iter()
        .map(|(idx, (puntaje, mejor))| (idx, puntaje, mejor))
        .collect();
    v.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.2.cmp(&b.2))
            .then_with(|| a.0.cmp(&b.0))
    });
    v.into_iter()
        .map(|(idx, puntaje, _)| (idx, puntaje))
        .collect()
}

/// Fusión tal como la arma el flujo: solo las primeras `LEG_K` de cada pierna,
/// sin truncar (a lo sumo 2·`LEG_K`). Las piernas pueden venir más hondas.
fn fusion_del_flujo(vectorial: &[usize], lexical: &[usize]) -> Vec<usize> {
    let corte = |pierna: &[usize]| pierna.len().min(LEG_K);
    rrf_fuse(&vectorial[..corte(vectorial)], &lexical[..corte(lexical)])
        .into_iter()
        .map(|(i, _)| i)
        .collect()
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
    fn los_empates_de_rrf_salen_siempre_en_el_mismo_orden() {
        // 0 y 2 empatan en 1/61 (primeros en una sola pierna); 1 y 3 en 1/62.
        // Con igual puntaje e igual mejor posición decide el índice, que sigue
        // el orden por id del corpus. Un orden que depende de la iteración de
        // un HashMap manda candidatos distintos al rerank en cada corrida.
        for _ in 0..64 {
            let orden: Vec<usize> = rrf_fuse(&[0, 1], &[2, 3])
                .into_iter()
                .map(|(i, _)| i)
                .collect();
            assert_eq!(orden, [0, 2, 1, 3]);
        }
    }

    #[test]
    fn en_un_empate_de_rrf_gana_la_mejor_posicion_en_alguna_pierna() {
        // A queda 3.º en la vectorial y 24.º en la léxica; B, 12.º en las dos.
        // Los puntajes empatan exactamente: decide la mejor posición de cada
        // uno, no el índice (B tiene el menor).
        let (a, b) = (100, 1);
        let mut vectorial: Vec<usize> = (200..224).collect();
        vectorial[2] = a;
        vectorial[11] = b;
        let mut lexica: Vec<usize> = (300..324).collect();
        lexica[11] = b;
        lexica[23] = a;
        let fusion = rrf_fuse(&vectorial, &lexica);
        let puntaje = |i: usize| fusion.iter().find(|(j, _)| *j == i).unwrap().1;
        assert_eq!(puntaje(a), puntaje(b), "el ejemplo tiene que empatar");
        let posicion = |i: usize| fusion.iter().position(|(j, _)| *j == i).unwrap();
        assert!(posicion(a) < posicion(b), "{fusion:?}");
    }

    #[test]
    fn la_fusion_del_flujo_solo_ve_las_primeras_leg_k_de_cada_pierna() {
        // Piernas de 30: X (índice 7) queda 27.º en las dos, fuera de LEG_K.
        let x = 7;
        let mut vectorial: Vec<usize> = (200..230).collect();
        vectorial[26] = x;
        let mut lexica: Vec<usize> = (300..330).collect();
        lexica[26] = x;
        // Fusionando las piernas enteras, X suma 2/87 y le gana a cualquier
        // chunk que esté primero en una sola pierna (1/61).
        assert_eq!(rrf_fuse(&vectorial, &lexica)[0].0, x);

        let flujo = fusion_del_flujo(&vectorial, &lexica);
        // El flujo solo trae LEG_K por pierna: X no llega, y la fusión son
        // los 2·LEG_K candidatos distintos, encabezados por los dos primeros
        // de cada pierna (empate de 1/61, desempata el índice).
        assert!(!flujo.contains(&x));
        assert_eq!(flujo.len(), 2 * LEG_K);
        assert_eq!(flujo[..2], [200, 300]);
    }

    #[test]
    fn a_cualquier_profundidad_la_fusion_del_flujo_es_la_que_va_al_rerank() {
        let repo = repo_completo();
        let r = recuperador(false, false);
        let recorte = ["c-conflicto".to_string()];
        let flujo: Vec<String> = r
            .recuperar_en_colecciones(&repo, "huelga segundo", &recorte, RERANK_DEPTH)
            .fragmentos
            .into_iter()
            .map(|f| f.chunk_id)
            .collect();
        assert_eq!(flujo, ["chunk-1", "chunk-3", "chunk-2"]);
        // Con piernas más cortas que LEG_K o más hondas, la fusión del flujo
        // se arma con LEG_K por pierna, como en producción.
        for profundidad in [2, 2 * LEG_K] {
            let d = r.ranking_diagnostico(&repo, "huelga segundo", &recorte, profundidad);
            assert_eq!(d.fusion_flujo, flujo, "profundidad {profundidad}");
        }
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

    #[test]
    fn con_profundidad_leg_k_la_fusion_trae_lo_que_el_flujo_manda_al_rerank() {
        let repo = repo_completo();
        // `RerankIdentidad` conserva el orden: la salida del flujo es el top de
        // la fusión que entró al rerank, truncado al límite.
        let r = recuperador(false, false);
        let recorte = ["c-conflicto".to_string()];
        let flujo: Vec<String> = r
            .recuperar_en_colecciones(&repo, "huelga segundo", &recorte, RERANK_DEPTH)
            .fragmentos
            .into_iter()
            .map(|f| f.chunk_id)
            .collect();
        // chunk-1 suma en las dos piernas (1/61 + 1/62), chunk-3 también
        // (1/63 + 1/61) y chunk-2 solo en la vectorial (1/62).
        assert_eq!(flujo, ["chunk-1", "chunk-3", "chunk-2"]);

        let d = r.ranking_diagnostico(&repo, "huelga segundo", &recorte, LEG_K);
        let candidatos: Vec<String> = d.fusion.into_iter().take(RERANK_DEPTH).collect();
        assert_eq!(candidatos, flujo);
    }

    /// Embedder de prueba que cuenta cuántas veces se lo llamó.
    struct EmbedderContado {
        llamadas: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl Embedder for EmbedderContado {
        fn embed(&self, _: &str) -> Result<Vec<f32>, String> {
            self.llamadas
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(vec![1.0, 0.0])
        }
    }

    /// Reranker de prueba que no se debe llamar nunca.
    struct RerankProhibido;
    impl Reranker for RerankProhibido {
        fn rerank(&self, _: &str, _: &[String], _: usize) -> Result<Vec<(usize, f64)>, String> {
            panic!("el diagnóstico no llama al rerank");
        }
    }

    #[test]
    fn el_diagnostico_ordena_y_trunca_piernas_y_fusion_dentro_del_recorte() {
        let repo = repo_completo();
        let embeddings = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let r = Recuperador::con_clientes(
            Box::new(EmbedderContado {
                llamadas: embeddings.clone(),
            }),
            Box::new(RerankProhibido),
        );
        let recorte = ["c-conflicto".to_string()];

        let d = r.ranking_diagnostico(&repo, "huelga segundo", &recorte, 2);
        // Los tres chunks del recorte empatan en coseno: la pierna vectorial
        // los ordena por posición y se corta en 2.
        assert_eq!(d.vectorial, ["chunk-1", "chunk-2"]);
        // chunk-stress-1 también dice «huelga», pero vive fuera del recorte.
        assert_eq!(d.lexica, ["chunk-3", "chunk-1"]);
        // chunk-1 suma en las dos piernas; chunk-3 (1/61) le gana a chunk-2
        // (1/62), que queda fuera de la fusión cortada en 2.
        assert_eq!(d.fusion, ["chunk-1", "chunk-3"]);
        assert_eq!(
            d.llamadas,
            LlamadasRecuperacion {
                embeddings: 1,
                rerank: 0
            }
        );
        assert_eq!(embeddings.load(std::sync::atomic::Ordering::SeqCst), 1);

        let d = r.ranking_diagnostico(&repo, "huelga segundo", &recorte, 1);
        assert_eq!(d.vectorial, ["chunk-1"]);
        assert_eq!(d.lexica, ["chunk-3"]);
        assert_eq!(d.fusion.len(), 1);
    }

    #[test]
    fn sin_pierna_semantica_el_diagnostico_lo_declara() {
        let repo = repo_completo();
        let r = recuperador(true, false);
        let d = r.ranking_diagnostico(&repo, "huelga segundo", &["c-conflicto".into()], 3);
        // Un ranking solo léxico que no lo dice mide otra cosa en silencio.
        assert!(d.vectorial.is_empty());
        assert_eq!(d.lexica, ["chunk-3", "chunk-1"]);
        let motivo = d.degradacion.expect("la degradación tiene que viajar");
        assert!(motivo.contains("sin pierna semántica"), "{motivo}");
        // El intento cuenta aunque falle: ver `LlamadasRecuperacion`.
        assert_eq!(d.llamadas.embeddings, 1);
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

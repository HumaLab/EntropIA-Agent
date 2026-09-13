//! EntropIA-Bench (PLAN, transversal desde Fase 2): banco de preguntas
//! ancladas al corpus SOIP y **cadena de atribución de fallos**.
//!
//! La cadena distingue «no recuperé el documento» de «razoné mal sobre el que
//! recuperé» — y, con 61 % de items sin procesar, de «el documento no existía
//! para mí». Las preguntas se anclan solo a items con chunks: preguntar por
//! material no procesado mide la brecha de Lite/Pro, no al agente.

use serde::{Deserialize, Serialize};

use crate::recuperacion::{Recuperador, RERANK_DEPTH};

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
    /// Grupos de evidencia: cada grupo reúne chunks alternativos que aportan la
    /// misma pieza (copias del mismo documento en otra colección); basta uno.
    /// Todos los grupos son necesarios. Vacío: un grupo por chunk esperado.
    #[serde(default)]
    pub grupos_esperados: Vec<Vec<String>>,
}

impl PreguntaBench {
    /// Grupos de evidencia efectivos: los declarados o, si no hay, uno por chunk
    /// esperado (así se mide el banco que no declara grupos).
    fn grupos_evidencia(&self) -> Vec<Vec<String>> {
        if self.grupos_esperados.is_empty() {
            self.chunk_ids_esperados
                .iter()
                .map(|c| vec![c.clone()])
                .collect()
        } else {
            self.grupos_esperados.clone()
        }
    }
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
///
/// Cada métrica es `None` cuando no aplica (la pregunta no declara esperados)
/// o no se midió: un 1.0 por defecto infla el promedio con preguntas que no
/// prueban nada.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CadenaAtribucion {
    pub pregunta_id: String,
    /// Fracción de items esperados que tienen chunks (lo que el agente podía ver).
    pub cobertura_items_esperados: Option<f64>,
    /// Fracción de grupos de evidencia esperados que cubre la recuperación
    /// híbrida (un grupo se cubre con cualquiera de sus chunks).
    pub retrieval_recall: Option<f64>,
    /// Línea base léxica (FTS5) a la misma profundidad que la híbrida.
    pub retrieval_recall_lexico: Option<f64>,
    /// Fracción de grupos de evidencia esperados cubiertos por las citas.
    pub evidence_recall: Option<f64>,
    /// Fracción de citas de la respuesta que están entre los esperados.
    pub citation_precision: Option<f64>,
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

/// Alcance que recibe un usuario que marca «seleccionar todo»: cada colección
/// fuera de la denylist. No sale de los items esperados: eso filtraría la
/// respuesta al recuperador.
pub fn alcance_seleccionar_todo(repo: &crate::repositorio::RepositorioSqlite) -> Vec<String> {
    repo.listar_colecciones()
        .into_iter()
        .map(|c| c.id)
        .collect()
}

/// Evalúa una pregunta: la recuperación siempre y, si se pasa la respuesta
/// (sus chunks citados), también la evidencia de esa respuesta.
pub fn evaluar(
    repo: &crate::repositorio::RepositorioSqlite,
    recuperador: Option<&Recuperador>,
    pregunta: &PreguntaBench,
    respuesta: Option<&[String]>,
) -> CadenaAtribucion {
    // Cobertura: ¿los items esperados tienen chunks?
    let con_chunks = pregunta
        .items_esperados
        .iter()
        .filter(|id| item_tiene_chunks(repo, id))
        .count();
    let cobertura = if pregunta.items_esperados.is_empty() {
        None // sin items esperados, la cobertura no acota el juicio
    } else {
        Some(con_chunks as f64 / pregunta.items_esperados.len() as f64)
    };

    // Los tres recall se miden sobre grupos de evidencia: dos copias del mismo
    // documento no exigen recuperar las dos.
    let grupos = pregunta.grupos_evidencia();

    // Recuperación híbrida del flujo, a su profundidad y en su alcance. Si la
    // pregunta no declara chunks esperados no se consulta: sería una llamada
    // paga para una métrica que no aplica.
    let retrieval_recall = recuperador.filter(|_| !grupos.is_empty()).and_then(|rec| {
        let alcance = alcance_seleccionar_todo(repo);
        let recuperados: Vec<String> = rec
            .recuperar_en_colecciones(repo, &pregunta.pregunta, &alcance, RERANK_DEPTH)
            .fragmentos
            .into_iter()
            .map(|f| f.chunk_id)
            .collect();
        recall(&grupos, &recuperados)
    });

    // Línea base léxica a la profundidad del flujo, no a una más generosa.
    let lexicos = repo.buscar_fts5(&pregunta.pregunta, RERANK_DEPTH);
    let retrieval_recall_lexico = recall(&grupos, &lexicos);

    // Evidence recall y citation precision sobre las citas de la respuesta;
    // sin respuesta (solo recuperación) no hay citas que medir.
    let evidence_recall = respuesta.and_then(|citados| recall(&grupos, citados));
    let citation_precision = respuesta.map(|chunks_citados| {
        if chunks_citados.is_empty() {
            return 0.0;
        }
        let esperados: std::collections::HashSet<&String> =
            pregunta.chunk_ids_esperados.iter().collect();
        let citas_validas = chunks_citados
            .iter()
            .filter(|c| esperados.contains(c))
            .count();
        citas_validas as f64 / chunks_citados.len() as f64
    });

    CadenaAtribucion {
        pregunta_id: pregunta.id.clone(),
        cobertura_items_esperados: cobertura,
        retrieval_recall,
        retrieval_recall_lexico,
        evidence_recall,
        citation_precision,
        claim_support: None,
        answer_quality: None,
    }
}

/// Fracción de grupos de evidencia con al menos un chunk entre los obtenidos.
fn recall(grupos: &[Vec<String>], obtenidos: &[String]) -> Option<f64> {
    if grupos.is_empty() {
        return None; // sin esperados, no hay recall que medir
    }
    let set: std::collections::HashSet<&String> = obtenidos.iter().collect();
    let cubiertos = grupos
        .iter()
        .filter(|grupo| grupo.iter().any(|c| set.contains(c)))
        .count();
    Some(cubiertos as f64 / grupos.len() as f64)
}

/// Promedio de una métrica sobre las preguntas donde aplica.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Agregado {
    /// `None` si ninguna pregunta aplica.
    pub media: Option<f64>,
    /// Preguntas donde la métrica aplica (entran al promedio).
    pub aplicables: usize,
    /// Preguntas evaluadas.
    pub total: usize,
}

/// Resumen del banco: cada métrica con su cantidad de preguntas aplicables, para
/// que «1 de 12» se vea en vez de esconderse en un promedio.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResumenBench {
    pub cobertura_items_esperados: Agregado,
    pub retrieval_recall: Agregado,
    pub retrieval_recall_lexico: Agregado,
    pub evidence_recall: Agregado,
    pub citation_precision: Agregado,
}

/// Resume las cadenas promediando cada métrica solo donde aplica.
pub fn resumir(cadenas: &[CadenaAtribucion]) -> ResumenBench {
    let agregar = |metrica: fn(&CadenaAtribucion) -> Option<f64>| {
        let valores: Vec<f64> = cadenas.iter().filter_map(metrica).collect();
        Agregado {
            media: (!valores.is_empty())
                .then(|| valores.iter().sum::<f64>() / valores.len() as f64),
            aplicables: valores.len(),
            total: cadenas.len(),
        }
    };
    ResumenBench {
        cobertura_items_esperados: agregar(|c| c.cobertura_items_esperados),
        retrieval_recall: agregar(|c| c.retrieval_recall),
        retrieval_recall_lexico: agregar(|c| c.retrieval_recall_lexico),
        evidence_recall: agregar(|c| c.evidence_recall),
        citation_precision: agregar(|c| c.citation_precision),
    }
}

/// Corrida del banco solo sobre la recuperación, tal como se guarda en
/// `bench/resultados/`. Declara con qué pipeline, a qué profundidad y en qué
/// alcance se midió: sin eso dos corridas no se pueden comparar.
#[derive(Debug, Clone, Serialize)]
pub struct ResultadoBench {
    pub banco: String,
    /// `hibrido` o `solo_lexico`.
    pub pipeline: String,
    /// Qué quedó sin medir, si algo quedó.
    pub nota: Option<String>,
    /// Profundidad de recuperación por pregunta.
    pub k: usize,
    /// Colecciones consultadas: «seleccionar todo» menos la denylist.
    pub alcance: Vec<String>,
    pub cadenas: Vec<CadenaAtribucion>,
    pub resumen: ResumenBench,
}

/// Evalúa el banco entero solo sobre la recuperación (sin respuesta del
/// agente). Sin recuperador mide únicamente la línea base léxica, y lo declara.
pub fn correr_recuperacion(
    repo: &crate::repositorio::RepositorioSqlite,
    recuperador: Option<&Recuperador>,
    banco: &BancoBench,
) -> ResultadoBench {
    let cadenas: Vec<CadenaAtribucion> = banco
        .preguntas
        .iter()
        .map(|p| evaluar(repo, recuperador, p, None))
        .collect();
    let resumen = resumir(&cadenas);
    ResultadoBench {
        banco: banco.banco.clone(),
        pipeline: if recuperador.is_some() {
            "hibrido"
        } else {
            "solo_lexico"
        }
        .into(),
        nota: recuperador.is_none().then(|| {
            "sin recuperador híbrido: solo se midió la línea base léxica (FTS5); \
             retrieval_recall no se midió"
                .to_string()
        }),
        k: RERANK_DEPTH,
        alcance: alcance_seleccionar_todo(repo),
        cadenas,
        resumen,
    }
}

/// Mejor posición (desde 1) de un grupo de evidencia en cada ordenamiento del
/// diagnóstico: la de cualquiera de sus chunks. `None` si ninguno aparece
/// dentro de la profundidad medida.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PosicionGrupo {
    pub chunks: Vec<String>,
    pub vectorial: Option<usize>,
    pub lexica: Option<usize>,
    /// En la fusión del flujo (piernas de `LEG_K`, a lo sumo 2·`LEG_K`).
    pub fusion: Option<usize>,
    /// En la fusión de piernas medidas a la profundidad del diagnóstico:
    /// simula subir `LEG_K` a esa profundidad.
    pub fusion_leg_k_simulado: Option<usize>,
}

/// Recall de grupos de evidencia sobre los primeros `k` de la fusión. `None`
/// si la pregunta no declara evidencia.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecallEnK {
    pub k: usize,
    pub recall: Option<f64>,
}

/// Diagnóstico de profundidad de una pregunta: en qué posición de cada pierna
/// y de la fusión RRF aparece su evidencia, antes del rerank.
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticoProfundidad {
    pub pregunta_id: String,
    /// Profundidad medida: el mayor `k` pedido.
    pub profundidad: usize,
    pub grupos: Vec<PosicionGrupo>,
    /// Curva de recall de la fusión del flujo, un punto por cada `k` pedido:
    /// la que mide el pool de candidatos real.
    pub recall_fusion: Vec<RecallEnK>,
    /// Curva de recall de la fusión que simula subir `LEG_K` a `profundidad`.
    pub recall_fusion_leg_k_simulado: Vec<RecallEnK>,
    /// `None` cuando corrieron las dos piernas (ver `RankingDiagnostico`).
    pub degradacion: Option<String>,
}

/// Diagnostica a qué profundidad aparece la evidencia de una pregunta, en el
/// mismo alcance que `evaluar` («seleccionar todo» menos la denylist).
pub fn diagnosticar_profundidad(
    repo: &crate::repositorio::RepositorioSqlite,
    recuperador: &Recuperador,
    pregunta: &PreguntaBench,
    ks: &[usize],
) -> DiagnosticoProfundidad {
    let profundidad = ks.iter().copied().max().unwrap_or(0);
    let evidencia = pregunta.grupos_evidencia();
    if evidencia.is_empty() {
        // Sin evidencia no hay posición ni recall que medir: no se paga una
        // llamada de embeddings para una métrica que no aplica.
        return DiagnosticoProfundidad {
            pregunta_id: pregunta.id.clone(),
            profundidad,
            grupos: Vec::new(),
            recall_fusion: ks.iter().map(|&k| RecallEnK { k, recall: None }).collect(),
            recall_fusion_leg_k_simulado: ks
                .iter()
                .map(|&k| RecallEnK { k, recall: None })
                .collect(),
            degradacion: None,
        };
    }
    let alcance = alcance_seleccionar_todo(repo);
    let ranking = recuperador.ranking_diagnostico(repo, &pregunta.pregunta, &alcance, profundidad);
    let posicion = |orden: &[String], grupo: &[String]| {
        orden
            .iter()
            .position(|id| grupo.contains(id))
            .map(|p| p + 1)
    };
    let grupos = evidencia
        .iter()
        .map(|chunks| PosicionGrupo {
            chunks: chunks.clone(),
            vectorial: posicion(&ranking.vectorial, chunks),
            lexica: posicion(&ranking.lexica, chunks),
            fusion: posicion(&ranking.fusion_flujo, chunks),
            fusion_leg_k_simulado: posicion(&ranking.fusion, chunks),
        })
        .collect();
    let curva = |fusion: &[String]| -> Vec<RecallEnK> {
        ks.iter()
            .map(|&k| RecallEnK {
                k,
                recall: recall(&evidencia, &fusion[..k.min(fusion.len())]),
            })
            .collect()
    };
    DiagnosticoProfundidad {
        pregunta_id: pregunta.id.clone(),
        profundidad,
        grupos,
        recall_fusion: curva(&ranking.fusion_flujo),
        recall_fusion_leg_k_simulado: curva(&ranking.fusion),
        degradacion: ranking.degradacion,
    }
}

/// Punto de la curva media: recall de la fusión a `k`, promediado sobre las
/// preguntas donde aplica.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecallMedioEnK {
    pub k: usize,
    pub recall: Agregado,
}

/// Curvas medias de recall de la fusión, promediadas en cada `k` sobre las
/// preguntas que declaran evidencia (las demás no entran al promedio).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResumenProfundidad {
    /// Sobre la fusión del flujo (piernas de `LEG_K`): la curva principal.
    pub recall_fusion: Vec<RecallMedioEnK>,
    /// Sobre la fusión que simula subir `LEG_K` a la profundidad medida.
    pub recall_fusion_leg_k_simulado: Vec<RecallMedioEnK>,
}

/// Resume los diagnósticos en sus dos curvas medias.
pub fn resumir_profundidad(
    diagnosticos: &[DiagnosticoProfundidad],
    ks: &[usize],
) -> ResumenProfundidad {
    let media = |curva: fn(&DiagnosticoProfundidad) -> &[RecallEnK]| -> Vec<RecallMedioEnK> {
        ks.iter()
            .map(|&k| {
                let valores: Vec<f64> = diagnosticos
                    .iter()
                    .filter_map(|d| curva(d).iter().find(|r| r.k == k).and_then(|r| r.recall))
                    .collect();
                RecallMedioEnK {
                    k,
                    recall: Agregado {
                        media: (!valores.is_empty())
                            .then(|| valores.iter().sum::<f64>() / valores.len() as f64),
                        aplicables: valores.len(),
                        total: diagnosticos.len(),
                    },
                }
            })
            .collect()
    };
    ResumenProfundidad {
        recall_fusion: media(|d| d.recall_fusion.as_slice()),
        recall_fusion_leg_k_simulado: media(|d| d.recall_fusion_leg_k_simulado.as_slice()),
    }
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
            grupos_esperados: vec![],
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
    fn los_grupos_del_banco_real_cubren_exactamente_sus_chunks_esperados() {
        let banco = cargar_banco("bench/preguntas.json").expect("el banco debe existir en el repo");
        let mut ids = std::collections::HashSet::new();
        for p in &banco.preguntas {
            assert!(
                ids.insert(p.id.as_str()),
                "id repetido en el banco: {}",
                p.id
            );
            if p.grupos_esperados.is_empty() {
                continue;
            }
            // Un chunk puede estar en varios grupos: si sostiene dos partes de
            // la respuesta, encontrarlo satisface a las dos.
            let mut en_grupos = std::collections::HashSet::new();
            for grupo in &p.grupos_esperados {
                assert!(!grupo.is_empty(), "{}: grupo vacío", p.id);
                en_grupos.extend(grupo);
            }
            let esperados: std::collections::HashSet<&String> =
                p.chunk_ids_esperados.iter().collect();
            assert_eq!(
                en_grupos, esperados,
                "{}: los grupos deben cubrir exactamente los chunks esperados",
                p.id
            );
        }
    }

    #[test]
    fn la_cadena_mide_recall_y_precision_de_citas() {
        let r = repo();
        let p = pregunta(vec!["chunk-1".into(), "chunk-2".into()]);
        // La respuesta cita chunk-1 (correcto) y chunk-stress-1 (de prueba, no
        // esperado): evidence_recall 0.5, citation_precision 0.5.
        let citas = ["chunk-1".to_string(), "chunk-stress-1".to_string()];
        let cadena = evaluar(&r, None, &p, Some(citas.as_slice()));
        assert_eq!(cadena.evidence_recall, Some(0.5));
        assert_eq!(cadena.citation_precision, Some(0.5));
        assert_eq!(cadena.cobertura_items_esperados, Some(1.0)); // item-1 tiene chunks
    }

    #[test]
    fn la_cobertura_distingue_el_fallo_de_brecha() {
        let r = repo();
        let mut p = pregunta(vec![]);
        p.items_esperados = vec!["item-3".into()]; // sin chunks en el corpus sintético
        let cadena = evaluar(&r, None, &p, None);
        assert_eq!(cadena.cobertura_items_esperados, Some(0.0));
        // Sin esperados de chunk, el recall no aplica: ni penaliza ni suma.
        assert_eq!(cadena.retrieval_recall_lexico, None);
    }

    #[test]
    fn sin_chunks_esperados_el_recall_no_aplica_y_el_resumen_lo_excluye() {
        let r = repo();
        let mut sin_esperados = pregunta(vec![]);
        sin_esperados.items_esperados = vec![];
        let mut con_esperados = pregunta(vec!["chunk-1".into()]);
        con_esperados.id = "test-2".into();
        con_esperados.pregunta = "huelga".into();

        let sin = evaluar(&r, None, &sin_esperados, None);
        // Sin esperados no hay nada que medir: «no aplica», no un 1.0 regalado.
        assert_eq!(sin.retrieval_recall, None);
        assert_eq!(sin.retrieval_recall_lexico, None);
        assert_eq!(sin.cobertura_items_esperados, None);

        let con = evaluar(&r, None, &con_esperados, None);
        assert_eq!(con.retrieval_recall_lexico, Some(1.0));

        let resumen = resumir(&[sin, con]);
        assert_eq!(resumen.retrieval_recall_lexico.media, Some(1.0));
        assert_eq!(resumen.retrieval_recall_lexico.aplicables, 1);
        assert_eq!(resumen.retrieval_recall_lexico.total, 2);
    }

    /// Embedder de prueba: vector alineado con los embeddings del corpus
    /// sintético, para ejercitar la pierna semántica sin salir a la red.
    struct EmbedFijo;
    impl crate::recuperacion::Embedder for EmbedFijo {
        fn embed(&self, _: &str) -> Result<Vec<f32>, String> {
            Ok(vec![1.0, 0.0])
        }
    }

    /// Reranker de prueba: conserva el orden de la fusión.
    struct RerankIdentidad;
    impl crate::recuperacion::Reranker for RerankIdentidad {
        fn rerank(
            &self,
            _: &str,
            docs: &[String],
            limite: usize,
        ) -> Result<Vec<(usize, f64)>, String> {
            Ok((0..docs.len().min(limite)).map(|i| (i, 1.0)).collect())
        }
    }

    #[test]
    fn el_recall_de_recuperacion_sale_del_pipeline_hibrido_dentro_del_alcance() {
        let r = repo();
        let rec = Recuperador::con_clientes(Box::new(EmbedFijo), Box::new(RerankIdentidad));

        // Ningún token de la pregunta está en el corpus: la línea léxica no
        // trae nada y solo la pierna semántica puede encontrar chunk-1.
        let mut p = pregunta(vec!["chunk-1".into()]);
        p.pregunta = "conflicto portuario".into();
        let cadena = evaluar(&r, Some(&rec), &p, None);
        assert_eq!(cadena.retrieval_recall, Some(1.0));
        assert_eq!(cadena.retrieval_recall_lexico, Some(0.0));

        // El alcance es «seleccionar todo» menos la denylist: un chunk de una
        // colección de prueba nunca cuenta como recuperado.
        let alcance = alcance_seleccionar_todo(&r);
        assert!(alcance.contains(&"c-conflicto".to_string()));
        assert!(!alcance.iter().any(|c| c.starts_with("c-stress")));
        let mut de_prueba = pregunta(vec!["chunk-stress-1".into()]);
        de_prueba.pregunta = "huelga simulada de stress".into();
        let cadena = evaluar(&r, Some(&rec), &de_prueba, None);
        assert_eq!(cadena.retrieval_recall, Some(0.0));
    }

    #[test]
    fn el_diagnostico_da_la_mejor_posicion_de_cada_grupo_en_cada_ordenamiento() {
        let r = repo();
        let rec = Recuperador::con_clientes(Box::new(EmbedFijo), Box::new(RerankIdentidad));
        let mut p = pregunta(vec!["chunk-3".into(), "chunk-1".into(), "chunk-2".into()]);
        p.pregunta = "huelga segundo".into();
        // El primer grupo tiene dos copias; la peor va primero para que se vea
        // que cuenta la mejor posición de cualquiera de ellas.
        p.grupos_esperados = vec![
            vec!["chunk-3".into(), "chunk-1".into()],
            vec!["chunk-2".into()],
        ];
        // A profundidad 2: vectorial [chunk-1, chunk-2] (empate de coseno,
        // orden de carga) y léxica [chunk-3, chunk-1]. La fusión del flujo usa
        // piernas de LEG_K: [chunk-1, chunk-3, chunk-2]. La que simula
        // LEG_K = 2 fusiona las piernas de 2 y se corta en 2:
        // [chunk-1 (1/61 + 1/62), chunk-3 (1/61)]; chunk-2 (1/62) queda fuera.
        let d = diagnosticar_profundidad(&r, &rec, &p, &[1, 2]);
        assert_eq!(d.pregunta_id, "test-1");
        assert_eq!(d.profundidad, 2);
        assert_eq!(
            d.grupos,
            vec![
                PosicionGrupo {
                    chunks: vec!["chunk-3".into(), "chunk-1".into()],
                    vectorial: Some(1),
                    lexica: Some(1),
                    fusion: Some(1),
                    fusion_leg_k_simulado: Some(1),
                },
                PosicionGrupo {
                    chunks: vec!["chunk-2".into()],
                    vectorial: Some(2),
                    lexica: None,
                    fusion: Some(3),
                    fusion_leg_k_simulado: None,
                },
            ]
        );
    }

    #[test]
    fn el_recall_de_la_fusion_cuenta_la_evidencia_solo_desde_su_posicion() {
        let r = repo();
        let rec = Recuperador::con_clientes(Box::new(EmbedFijo), Box::new(RerankIdentidad));
        let mut p = pregunta(vec!["chunk-3".into(), "chunk-2".into()]);
        p.pregunta = "huelga segundo".into();
        // A profundidad 3 la fusión es [chunk-1 (1/61 + 1/62),
        // chunk-3 (1/63 + 1/61), chunk-2 (1/62)].
        let d = diagnosticar_profundidad(&r, &rec, &p, &[1, 2, 3]);
        assert_eq!(d.grupos[0].fusion, Some(2));
        assert_eq!(d.grupos[1].fusion, Some(3));
        assert_eq!(
            d.recall_fusion,
            vec![
                RecallEnK {
                    k: 1,
                    recall: Some(0.0)
                },
                RecallEnK {
                    k: 2,
                    recall: Some(0.5)
                },
                RecallEnK {
                    k: 3,
                    recall: Some(1.0)
                },
            ]
        );
        // Las piernas del corpus sintético caben enteras a profundidad 3: la
        // fusión que simula LEG_K = 3 coincide con la del flujo.
        assert_eq!(d.recall_fusion_leg_k_simulado, d.recall_fusion);
    }

    /// Embedder de prueba que no se debe llamar: la llamada sería paga.
    struct EmbedProhibido;
    impl crate::recuperacion::Embedder for EmbedProhibido {
        fn embed(&self, _: &str) -> Result<Vec<f32>, String> {
            panic!("sin evidencia esperada no hay nada que diagnosticar");
        }
    }

    #[test]
    fn sin_evidencia_el_diagnostico_no_aplica_y_no_consulta_al_recuperador() {
        let r = repo();
        let rec = Recuperador::con_clientes(Box::new(EmbedProhibido), Box::new(RerankIdentidad));
        let p = pregunta(vec![]);
        let d = diagnosticar_profundidad(&r, &rec, &p, &[8, 16]);
        assert!(d.grupos.is_empty());
        assert_eq!(
            d.recall_fusion,
            vec![
                RecallEnK { k: 8, recall: None },
                RecallEnK {
                    k: 16,
                    recall: None
                },
            ]
        );
    }

    /// Embedder de prueba que falla como sin clave de API.
    struct EmbedFalla;
    impl crate::recuperacion::Embedder for EmbedFalla {
        fn embed(&self, _: &str) -> Result<Vec<f32>, String> {
            Err("sin clave de API".into())
        }
    }

    #[test]
    fn si_falla_la_pierna_semantica_el_diagnostico_lo_declara() {
        let r = repo();
        let rec = Recuperador::con_clientes(Box::new(EmbedFalla), Box::new(RerankIdentidad));
        let mut p = pregunta(vec!["chunk-1".into()]);
        p.pregunta = "huelga".into();
        let d = diagnosticar_profundidad(&r, &rec, &p, &[8]);
        // Sin la declaración, «v=-» en todas las preguntas se leería como un
        // hallazgo sobre la pierna vectorial.
        assert_eq!(d.grupos[0].vectorial, None);
        assert_eq!(d.grupos[0].lexica, Some(1));
        let motivo = d.degradacion.expect("la degradación tiene que viajar");
        assert!(motivo.contains("sin pierna semántica"), "{motivo}");
    }

    type Curva<'a> = &'a [(usize, Option<f64>)];

    fn diagnostico(id: &str, flujo: Curva, simulada: Curva) -> DiagnosticoProfundidad {
        let puntos = |curva: Curva| {
            curva
                .iter()
                .map(|&(k, recall)| RecallEnK { k, recall })
                .collect()
        };
        DiagnosticoProfundidad {
            pregunta_id: id.into(),
            profundidad: 16,
            grupos: vec![],
            recall_fusion: puntos(flujo),
            recall_fusion_leg_k_simulado: puntos(simulada),
            degradacion: None,
        }
    }

    #[test]
    fn las_dos_curvas_medias_promedian_cada_k_solo_sobre_las_preguntas_con_evidencia() {
        let sin_evidencia = [(8, None), (16, None)];
        let diagnosticos = [
            diagnostico(
                "a",
                &[(8, Some(0.5)), (16, Some(1.0))],
                &[(8, Some(1.0)), (16, Some(1.0))],
            ),
            diagnostico(
                "b",
                &[(8, Some(0.0)), (16, Some(1.0))],
                &[(8, Some(0.5)), (16, Some(1.0))],
            ),
            diagnostico("sin-evidencia", &sin_evidencia, &sin_evidencia),
        ];
        let medio = |k: usize, media: f64| RecallMedioEnK {
            k,
            recall: Agregado {
                media: Some(media),
                aplicables: 2,
                total: 3,
            },
        };
        let resumen = resumir_profundidad(&diagnosticos, &[8, 16]);
        assert_eq!(resumen.recall_fusion, vec![medio(8, 0.25), medio(16, 1.0)]);
        assert_eq!(
            resumen.recall_fusion_leg_k_simulado,
            vec![medio(8, 0.75), medio(16, 1.0)]
        );
    }

    #[test]
    fn sin_respuesta_la_evidencia_no_se_mide() {
        let r = repo();
        let p = pregunta(vec!["chunk-1".into()]);
        // Solo recuperación: no hay respuesta que haya citado nada, y un 0.0
        // la acusaría de no citar.
        let cadena = evaluar(&r, None, &p, None);
        assert_eq!(cadena.evidence_recall, None);
        assert_eq!(cadena.citation_precision, None);
    }

    #[test]
    fn el_resultado_declara_pipeline_profundidad_y_resumen_en_json() {
        let r = repo();
        let mut sin_esperados = pregunta(vec![]);
        sin_esperados.items_esperados = vec![];
        let mut con_esperados = pregunta(vec!["chunk-1".into()]);
        con_esperados.id = "test-2".into();
        con_esperados.pregunta = "huelga".into();
        let banco = BancoBench {
            banco: "banco-prueba".into(),
            preguntas: vec![sin_esperados, con_esperados],
        };

        // Sin recuperador: solo la línea base léxica, y el resultado lo dice.
        let json = serde_json::to_value(correr_recuperacion(&r, None, &banco)).unwrap();
        assert_eq!(json["banco"], "banco-prueba");
        assert_eq!(json["pipeline"], "solo_lexico");
        assert!(json["nota"].as_str().unwrap().contains("línea base léxica"));
        assert_eq!(json["k"], RERANK_DEPTH);
        assert!(json["alcance"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "c-conflicto"));
        assert_eq!(json["cadenas"].as_array().unwrap().len(), 2);
        assert_eq!(json["resumen"]["retrieval_recall"]["aplicables"], 0);
        assert_eq!(json["resumen"]["retrieval_recall_lexico"]["aplicables"], 1);
        assert_eq!(json["resumen"]["retrieval_recall_lexico"]["total"], 2);
        assert_eq!(json["resumen"]["retrieval_recall_lexico"]["media"], 1.0);

        let rec = Recuperador::con_clientes(Box::new(EmbedFijo), Box::new(RerankIdentidad));
        let json = serde_json::to_value(correr_recuperacion(&r, Some(&rec), &banco)).unwrap();
        assert_eq!(json["pipeline"], "hibrido");
        assert_eq!(json["resumen"]["retrieval_recall"]["aplicables"], 1);
    }

    #[test]
    fn dos_copias_del_mismo_documento_en_un_grupo_basta_recuperar_una() {
        let r = repo();
        // chunk-1 y chunk-2 son copias del mismo documento (defecto del corpus):
        // un solo grupo de evidencia, cualquiera de las dos lo satisface.
        let mut p = pregunta(vec!["chunk-1".into(), "chunk-2".into()]);
        p.pregunta = "huelga".into(); // la línea léxica solo trae chunk-1
        p.grupos_esperados = vec![vec!["chunk-1".into(), "chunk-2".into()]];
        let citas = ["chunk-1".to_string()];
        let cadena = evaluar(&r, None, &p, Some(citas.as_slice()));
        assert_eq!(cadena.retrieval_recall_lexico, Some(1.0));
        assert_eq!(cadena.evidence_recall, Some(1.0));
    }

    #[test]
    fn dos_grupos_de_evidencia_con_un_solo_acierto_miden_la_mitad() {
        let r = repo();
        // Dos piezas distintas de la respuesta: la primera tiene dos copias
        // (chunk-1, chunk-3), la segunda una (chunk-2). La línea léxica solo
        // trae chunk-1: cubre un grupo de dos, no un chunk de tres.
        let mut p = pregunta(vec!["chunk-1".into(), "chunk-3".into(), "chunk-2".into()]);
        p.pregunta = "huelga".into();
        p.grupos_esperados = vec![
            vec!["chunk-1".into(), "chunk-3".into()],
            vec!["chunk-2".into()],
        ];
        let cadena = evaluar(&r, None, &p, None);
        assert_eq!(cadena.retrieval_recall_lexico, Some(0.5));
    }

    #[test]
    fn un_chunk_que_sostiene_dos_partes_satisface_los_dos_grupos() {
        let r = repo();
        // chunk-1 trae las dos piezas de la respuesta; chunk-2 solo la
        // primera. Recuperar chunk-1 cubre la respuesta entera.
        let mut p = pregunta(vec!["chunk-1".into(), "chunk-2".into()]);
        p.pregunta = "huelga".into(); // la línea léxica solo trae chunk-1
        p.grupos_esperados = vec![
            vec!["chunk-2".into(), "chunk-1".into()],
            vec!["chunk-1".into()],
        ];
        let cadena = evaluar(&r, None, &p, None);
        assert_eq!(cadena.retrieval_recall_lexico, Some(1.0));
    }

    #[test]
    fn sin_grupos_declarados_cada_chunk_esperado_es_su_propio_grupo() {
        let r = repo();
        // Banco sin grupos: se mide como antes, un grupo por chunk esperado.
        let mut p = pregunta(vec!["chunk-1".into(), "chunk-2".into()]);
        p.pregunta = "huelga".into(); // la línea léxica solo trae chunk-1
        let citas = ["chunk-1".to_string()];
        let cadena = evaluar(&r, None, &p, Some(citas.as_slice()));
        assert_eq!(cadena.retrieval_recall_lexico, Some(0.5));
        assert_eq!(cadena.evidence_recall, Some(0.5));
    }

    #[test]
    fn las_citas_de_colecciones_excluidas_no_cuentan_como_precision() {
        let r = repo();
        let p = pregunta(vec!["chunk-1".into()]);
        let citas = ["chunk-stress-1".to_string()];
        let cadena = evaluar(&r, None, &p, Some(citas.as_slice()));
        assert_eq!(cadena.evidence_recall, Some(0.0));
        assert_eq!(cadena.citation_precision, Some(0.0));
    }
}

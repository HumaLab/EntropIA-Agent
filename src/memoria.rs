//! Memoria longitudinal del agente (PLAN §7.2, enfoque de engram sobre SQLite
//! nativo).
//!
//! `memories` + FTS5 (con triggers que mantienen el índice) + `memory_relations`
//! para el conflict judgment: al guardar un hallazgo se buscan candidatos por
//! similitud y se **superficia la relación** en vez de sobrescribir en silencio.
//! Un hallazgo que contradice uno previo no se pisa: se juzga (historiador o
//! agente vía `preguntar_al_investigador`).

use rusqlite::{params, OptionalExtension};

use crate::estado::{ahora, nuevo_id, EstadoDb};

/// Tipo de observación de memoria (PLAN §7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipoMemoria {
    Decision,
    Finding,
    Question,
    Hypothesis,
    Interpretation,
    Learning,
}

impl TipoMemoria {
    pub fn as_str(&self) -> &'static str {
        match self {
            TipoMemoria::Decision => "decision",
            TipoMemoria::Finding => "finding",
            TipoMemoria::Question => "question",
            TipoMemoria::Hypothesis => "hypothesis",
            TipoMemoria::Interpretation => "interpretation",
            TipoMemoria::Learning => "learning",
        }
    }
}

/// Una observación persistida en la memoria longitudinal.
#[derive(Debug, Clone)]
pub struct Memoria {
    pub id: String,
    pub title: String,
    pub tipo: String,
    pub content: String,
    pub project: String,
    pub topic_key: Option<String>,
    pub session_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Relación pendiente de juicio entre una memoria nueva y un candidato previo.
#[derive(Debug, Clone)]
pub struct ConflictoCandidato {
    pub relation_id: String,
    pub candidate_id: String,
    pub candidate_title: String,
    pub relation_sugerida: String,
}

/// Memoria longitudinal sobre `EstadoDb`.
pub struct MemoriaDb<'a> {
    db: &'a EstadoDb,
}

impl<'a> MemoriaDb<'a> {
    pub fn nuevo(db: &'a EstadoDb) -> Self {
        Self { db }
    }

    /// Guarda un hallazgo. Si `topic_key` ya existe en el proyecto, actualiza
    /// (upsert estable, §7.2); si no, inserta. Devuelve el id y los candidatos
    /// similares con relación pendiente de juicio.
    pub fn guardar(
        &self,
        project: &str,
        title: &str,
        tipo: TipoMemoria,
        content: &str,
        topic_key: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<(String, Vec<ConflictoCandidato>), String> {
        let t = ahora();
        let id =
            if let Some(key) = topic_key {
                match self.buscar_por_topic_key(project, key) {
                    Some(existente) => {
                        // Upsert: actualiza la fila existente.
                        self.db
                            .conn()
                            .execute(
                                "UPDATE memories SET title = ?1, type = ?2, content = ?3, \
                             updated_at = ?4 WHERE id = ?5",
                                params![title, tipo.as_str(), content, t, existente.id],
                            )
                            .map_err(|e| e.to_string())?;
                        existente.id
                    }
                    None => {
                        let id = nuevo_id("mem");
                        self.db
                        .conn()
                        .execute(
                            "INSERT INTO memories (id, title, type, content, project, topic_key, \
                             session_id, created_at, updated_at) \
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                            params![id, title, tipo.as_str(), content, project, key, session_id, t],
                        )
                        .map_err(|e| e.to_string())?;
                        id
                    }
                }
            } else {
                let id = nuevo_id("mem");
                self.db
                    .conn()
                    .execute(
                        "INSERT INTO memories (id, title, type, content, project, topic_key, \
                     session_id, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?7)",
                        params![id, title, tipo.as_str(), content, project, session_id, t],
                    )
                    .map_err(|e| e.to_string())?;
                id
            };

        // Conflict judgment: superficie de candidatos similares en el mismo
        // proyecto (nunca se sobrescribe en silencio).
        let candidatos = self.superficiar_conflictos(&id, project, content)?;
        Ok((id, candidatos))
    }

    /// Busca una memoria por su topic_key dentro del proyecto.
    pub fn buscar_por_topic_key(&self, project: &str, key: &str) -> Option<Memoria> {
        let mut stmt = self
            .db
            .conn()
            .prepare(
                "SELECT id, title, type, content, project, topic_key, session_id, \
                 created_at, updated_at FROM memories WHERE project = ?1 AND topic_key = ?2",
            )
            .ok()?;
        stmt.query_row(params![project, key], |r| {
            Ok(Memoria {
                id: r.get(0)?,
                title: r.get(1)?,
                tipo: r.get(2)?,
                content: r.get(3)?,
                project: r.get(4)?,
                topic_key: r.get(5)?,
                session_id: r.get(6)?,
                created_at: r.get(7)?,
                updated_at: r.get(8)?,
            })
        })
        .optional()
        .ok()
        .flatten()
    }

    /// Busca memorias por similitud FTS5 (título + contenido) en un proyecto.
    pub fn buscar(&self, project: &str, texto: &str, limite: usize) -> Vec<Memoria> {
        let consulta = crate::repositorio::fts5_query(texto);
        if consulta.is_empty() {
            return Vec::new();
        }
        let Ok(mut stmt) = self.db.conn().prepare(
            "SELECT m.id, m.title, m.type, m.content, m.project, m.topic_key, m.session_id, \
             m.created_at, m.updated_at \
             FROM memories_fts f JOIN memories m ON m.rowid = f.rowid \
             WHERE memories_fts MATCH ?1 AND m.project = ?2 \
             ORDER BY bm25(memories_fts) LIMIT ?3",
        ) else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map(params![consulta, project, limite as i64], |r| {
            Ok(Memoria {
                id: r.get(0)?,
                title: r.get(1)?,
                tipo: r.get(2)?,
                content: r.get(3)?,
                project: r.get(4)?,
                topic_key: r.get(5)?,
                session_id: r.get(6)?,
                created_at: r.get(7)?,
                updated_at: r.get(8)?,
            })
        }) else {
            return Vec::new();
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// Superficia relaciones pendientes entre la memoria `id` y los candidatos
    /// similares (FTS5) del mismo proyecto. Devuelve las relaciones creadas.
    fn superficiar_conflictos(
        &self,
        id: &str,
        project: &str,
        content: &str,
    ) -> Result<Vec<ConflictoCandidato>, String> {
        let candidatos = self.buscar(project, content, 5);
        let mut resultado = Vec::new();
        for c in candidatos {
            if c.id == id {
                continue;
            }
            let relation_id = nuevo_id("rel");
            let sugerida = relacion_sugerida(content, &c.content);
            self.db
                .conn()
                .execute(
                    "INSERT INTO memory_relations (id, source_id, target_id, relation, \
                     judgment_status) VALUES (?1, ?2, ?3, ?4, 'pending')",
                    params![relation_id, id, c.id, sugerida],
                )
                .map_err(|e| e.to_string())?;
            resultado.push(ConflictoCandidato {
                relation_id,
                candidate_id: c.id,
                candidate_title: c.title,
                relation_sugerida: sugerida.into(),
            });
        }
        Ok(resultado)
    }

    /// Juzga una relación pendiente (el juez: historiador o agente).
    pub fn juzgar(&self, relation_id: &str, relacion_final: &str) -> Result<(), String> {
        self.db
            .conn()
            .execute(
                "UPDATE memory_relations SET relation = ?1, judgment_status = 'judged', \
                 judged_at = ?2 WHERE id = ?3",
                params![relacion_final, ahora(), relation_id],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Relaciones pendientes de un proyecto (para la UI o `preguntar_al_investigador`).
    pub fn relaciones_pendientes(&self, project: &str) -> Vec<(String, String, String, String)> {
        let Ok(mut stmt) = self.db.conn().prepare(
            "SELECT mr.id, m.title, mr.relation, c.title \
             FROM memory_relations mr \
             JOIN memories m ON m.id = mr.source_id \
             JOIN memories c ON c.id = mr.target_id \
             WHERE mr.judgment_status = 'pending' AND m.project = ?1",
        ) else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map(params![project], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        }) else {
            return Vec::new();
        };
        rows.filter_map(|r| r.ok()).collect()
    }
}

/// Heurística liviana de superficie (PLAN §7.2): si la polaridad de los dos
/// textos difiere —uno afirma y el otro niega— la relación sugerida es un
/// conflicto pendiente. El juez decide; esto solo evita sobrescribir en
/// silencio.
pub fn relacion_sugerida(nuevo: &str, existente: &str) -> &'static str {
    fn niega(t: &str) -> bool {
        let t = t.to_lowercase();
        [
            "no fue",
            "no hubo",
            "no existió",
            "contradice",
            "niega",
            "falso",
            "no comenzó",
        ]
        .iter()
        .any(|p| t.contains(p))
    }
    if niega(nuevo) != niega(existente) {
        "conflicts_with"
    } else {
        "related"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> EstadoDb {
        EstadoDb::abrir_en_memoria().unwrap()
    }

    #[test]
    fn guarda_y_recupera_por_fts5() {
        let db = base();
        let m = MemoriaDb::nuevo(&db);
        m.guardar(
            "soip-conflictividad",
            "Huelga de marzo",
            TipoMemoria::Finding,
            "La huelga de la pesca comenzó el 17 de marzo de 1965 en Mar del Plata.",
            None,
            None,
        )
        .unwrap();
        let resultados = m.buscar("soip-conflictividad", "huelga pesca marzo", 5);
        assert_eq!(resultados.len(), 1);
        assert_eq!(resultados[0].title, "Huelga de marzo");
        // Un proyecto distinto no ve la memoria.
        assert!(m.buscar("otro-proyecto", "huelga", 5).is_empty());
    }

    #[test]
    fn el_upsert_por_topic_key_no_duplica() {
        let db = base();
        let m = MemoriaDb::nuevo(&db);
        let (id1, _) = m
            .guardar(
                "soip-conflictividad",
                "Título A",
                TipoMemoria::Finding,
                "Contenido A",
                Some("tema-huelga-65"),
                None,
            )
            .unwrap();
        let (id2, _) = m
            .guardar(
                "soip-conflictividad",
                "Título B",
                TipoMemoria::Finding,
                "Contenido B",
                Some("tema-huelga-65"),
                None,
            )
            .unwrap();
        assert_eq!(id1, id2);
        let filas: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE project = 'soip-conflictividad'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(filas, 1);
        let memoria = m
            .buscar_por_topic_key("soip-conflictividad", "tema-huelga-65")
            .unwrap();
        assert_eq!(memoria.title, "Título B");
    }

    #[test]
    fn un_hallazgo_contradictorio_superficia_un_conflicto_pendiente() {
        let db = base();
        let m = MemoriaDb::nuevo(&db);
        m.guardar(
            "soip-conflictividad",
            "Inicio de la huelga",
            TipoMemoria::Finding,
            "La huelga comenzó el 17 de marzo de 1965.",
            None,
            None,
        )
        .unwrap();
        let (_, candidatos) = m
            .guardar(
                "soip-conflictividad",
                "Fecha corregida",
                TipoMemoria::Finding,
                "La huelga no comenzó el 17 de marzo: la evidencia indica el 20.",
                None,
                None,
            )
            .unwrap();
        assert!(!candidatos.is_empty());
        let pendientes = m.relaciones_pendientes("soip-conflictividad");
        assert!(!pendientes.is_empty());
        // El nuevo hallazgo no pisó al anterior: ambos existen.
        let filas: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE project = 'soip-conflictividad'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(filas, 2);
        // El juez resuelve la relación pendiente.
        m.juzgar(&candidatos[0].relation_id, "conflicts_with")
            .unwrap();
        assert!(m.relaciones_pendientes("soip-conflictividad").is_empty());
    }

    #[test]
    fn relacion_sugerida_detecta_polaridad() {
        assert_eq!(
            relacion_sugerida(
                "La huelga no comenzó en marzo.",
                "La huelga comenzó en marzo."
            ),
            "conflicts_with"
        );
        assert_eq!(
            relacion_sugerida(
                "Los volantes circularon en mayo.",
                "Los volantes circularon en mayo y junio."
            ),
            "related"
        );
    }
}

//! Acceso a la base común de la app EntropIA (modo solo lectura).
//!
//! Expone la carga de chunks con embeddings, la búsqueda léxica FTS5 y la
//! lectura de fragmentos y colecciones para el agente.
//!
//! Fase 0 (PLAN §6.5): todas las consultas excluyen la denylist de colecciones
//! de prueba (`configuracion::COLECCIONES_EXCLUIDAS`) y reportan la cobertura
//! del recorte consultado (items totales / con chunks / sin procesar).

use rusqlite::{params, params_from_iter, Connection, OpenFlags, ToSql};

use crate::configuracion::colecciones_excluidas;

/// Una fuente documental (fragmento) con su texto, para el agente.
#[derive(Debug, Clone)]
pub struct Fuente {
    pub id: String,
    pub titulo: String,
    pub coleccion: String,
    pub contenido: String,
}

/// Fragmento de texto con su embedding, para la búsqueda semántica.
pub struct ChunkRag {
    pub id: String,
    /// Item al que pertenece el fragmento: sin esto no se puede abrir la
    /// fuente real ni referenciarla en «Fuentes citadas».
    pub item_id: String,
    pub item_titulo: String,
    /// Identificador de la colección, no su nombre: es lo que permite acotar
    /// la recuperación al recorte congelado del job.
    pub collection_id: String,
    pub coleccion: String,
    pub asset_id: String,
    pub text_content: String,
    /// Rango de caracteres del fragmento dentro del texto del asset. La cita
    /// lo declara para que el investigador pueda ir a buscarlo.
    pub start_char: i64,
    pub end_char: i64,
    pub embedding: Vec<f32>,
}

/// Información de cobertura de una colección.
#[derive(Debug, Clone)]
pub struct ColeccionInfo {
    pub id: String,
    pub nombre: String,
    pub items: i64,
    pub items_con_chunks: i64,
    pub chunks: i64,
}

impl ColeccionInfo {
    /// Items de la colección sin ningún chunk procesado.
    pub fn items_sin_procesar(&self) -> i64 {
        self.items - self.items_con_chunks
    }
}

/// Cobertura del recorte consultado (PLAN §6.5).
///
/// Todo informe abre con esta tabla: con 255 de ~418 items reales sin chunks,
/// un informe que no declara la cobertura es engañoso aunque cada afirmación
/// esté verificada.
#[derive(Debug, Clone)]
pub struct Cobertura {
    pub items_total: i64,
    pub items_con_chunks: i64,
    pub items_sin_procesar: i64,
    pub colecciones: Vec<ColeccionInfo>,
}

/// Repositorio SQLite sobre la base común de la app EntropIA.
///
/// Se abre en modo solo lectura para no interferir con la base activa de la
/// aplicación. Lleva la denylist de colecciones de prueba como estado de
/// instancia: todas las consultas la aplican de forma consistente.
pub struct RepositorioSqlite {
    conn: Connection,
    denylist: Vec<String>,
}

impl RepositorioSqlite {
    /// Abre la base en modo solo lectura con la denylist por defecto.
    pub fn abrir(path: &str) -> Result<Self, String> {
        Self::abrir_con_denylist(path, colecciones_excluidas())
    }

    /// Abre la base con una denylist explícita (para tests y configuración).
    pub fn abrir_con_denylist(path: &str, denylist: Vec<String>) -> Result<Self, String> {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY;
        let conn = Connection::open_with_flags(path, flags).map_err(|e| e.to_string())?;
        Ok(Self { conn, denylist })
    }

    /// La denylist vigente (para el snapshot de reproducibilidad del job).
    pub fn denylist(&self) -> &[String] {
        &self.denylist
    }

    /// Cláusula `c.name NOT IN (?, …)` con sus parámetros (uso del gateway).
    pub fn clausula_no_excluidas_pub(&self) -> (String, Vec<String>) {
        self.clausula_no_excluidas()
    }

    /// Prepara una consulta sobre la conexión read-only (uso del gateway).
    pub fn prepare_pub(&self, sql: &str) -> Result<rusqlite::Statement<'_>, rusqlite::Error> {
        self.conn.prepare(sql)
    }

    /// Cláusula `c.name NOT IN (?, …)` con sus parámetros.
    fn clausula_no_excluidas(&self) -> (String, Vec<String>) {
        let placeholders = std::iter::repeat_n("?", self.denylist.len())
            .collect::<Vec<_>>()
            .join(", ");
        (
            format!("c.name NOT IN ({placeholders})"),
            self.denylist.clone(),
        )
    }

    /// Carga los chunks con su embedding y metadatos, excluyendo las
    /// colecciones de la denylist.
    pub fn cargar_chunks(&self) -> Result<Vec<ChunkRag>, String> {
        let (clausula, nombres) = self.clausula_no_excluidas();
        let sql = format!(
            "\
SELECT rc.id, rc.text_content, rc.embedding, i.title, COALESCE(c.name, ''), \
       rc.item_id, COALESCE(i.collection_id, ''), rc.asset_id, \
       rc.start_char, rc.end_char \
FROM rag_chunks rc \
LEFT JOIN items i ON i.id = rc.item_id \
LEFT JOIN collections c ON c.id = i.collection_id \
WHERE {clausula}"
        );
        let mut stmt = self.conn.prepare(&sql).map_err(|e| e.to_string())?;
        let parametros: Vec<&dyn ToSql> = nombres.iter().map(|n| n as &dyn ToSql).collect();
        let rows = stmt
            .query_map(params_from_iter(parametros), |row| {
                let blob: Vec<u8> = row.get(2)?;
                Ok(ChunkRag {
                    id: row.get(0)?,
                    text_content: row.get(1)?,
                    embedding: crate::vector::decodificar_embedding(&blob),
                    item_titulo: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    coleccion: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    item_id: row.get(5)?,
                    collection_id: row.get(6)?,
                    asset_id: row.get(7)?,
                    start_char: row.get(8)?,
                    end_char: row.get(9)?,
                })
            })
            .map_err(|e| e.to_string())?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Busca identificadores de chunks por texto con FTS5 (BM25), excluyendo
    /// las colecciones de la denylist.
    pub fn buscar_fts5(&self, consulta: &str, limite: usize) -> Vec<String> {
        let q = fts5_query(consulta);
        if q.is_empty() {
            return Vec::new();
        }
        let (clausula, nombres) = self.clausula_no_excluidas();
        // ?1 es la consulta MATCH; los `?` de la denylist son ?2..?N+1; el
        // LIMIT lleva el número explícito siguiente para no colisionar.
        let sql = format!(
            "\
SELECT f.chunk_id \
FROM rag_chunks_fts f \
JOIN rag_chunks rc ON rc.id = f.chunk_id \
JOIN items i ON i.id = rc.item_id \
JOIN collections c ON c.id = i.collection_id \
WHERE f.text_content MATCH ?1 AND {clausula} \
ORDER BY bm25(rag_chunks_fts) LIMIT ?{}",
            nombres.len() + 2
        );
        let Ok(mut stmt) = self.conn.prepare(&sql) else {
            return Vec::new();
        };
        let mut parametros: Vec<&dyn ToSql> = Vec::with_capacity(nombres.len() + 2);
        parametros.push(&q);
        parametros.extend(nombres.iter().map(|n| n as &dyn ToSql));
        parametros.push(&limite);
        let rows = stmt.query_map(params_from_iter(parametros), |row| row.get::<_, String>(0));
        let Ok(iter) = rows else {
            return Vec::new();
        };
        iter.filter_map(|r| r.ok()).collect()
    }

    /// Devuelve el texto completo de un chunk por su identificador.
    pub fn texto_chunk(&self, id: &str) -> Option<String> {
        let Ok(mut stmt) = self
            .conn
            .prepare("SELECT text_content FROM rag_chunks WHERE id = ?1")
        else {
            return None;
        };
        let Ok(mut rows) = stmt.query_map(params![id], |row| row.get::<_, String>(0)) else {
            return None;
        };
        rows.next().and_then(|r| r.ok())
    }

    /// Snapshot lógico del corpus para reproducibilidad (PLAN §6.9): versión
    /// de esquema, conteos por tabla, `max(updated_at)` y hash agregado de
    /// `source_text_hash` de chunks. Barato (no fila por fila) y estable: dos
    /// jobs «idénticos» ven el mismo corpus si y solo si el snapshot coincide.
    pub fn snapshot_corpus(&self) -> Result<String, String> {
        let schema = self
            .conn
            .query_row("SELECT COALESCE(MAX(id), 0) FROM _migrations", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap_or(0);

        let tablas = [
            "items",
            "collections",
            "rag_chunks",
            "entities",
            "triples",
            "transcriptions",
            "extractions",
            "annotations",
            "assets",
        ];
        let mut conteos: Vec<(String, i64)> = Vec::new();
        for t in tablas {
            let n = self
                .conn
                .query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| {
                    r.get::<_, i64>(0)
                })
                .unwrap_or(0);
            conteos.push((t.to_string(), n));
        }

        let max_updated = self
            .conn
            .query_row(
                "SELECT MAX(u) FROM (SELECT updated_at AS u FROM items \
                 UNION ALL SELECT updated_at FROM collections)",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0);

        // Hash agregado de source_text_hash: FNV-1a 64 incremental.
        let mut hash = fnv1a_inicio();
        if let Ok(mut stmt) = self
            .conn
            .prepare("SELECT source_text_hash FROM rag_chunks ORDER BY id")
        {
            if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) {
                for texto in rows.flatten() {
                    hash = fnv1a_actualizar(hash, texto.as_bytes());
                }
            }
        }

        serde_json::to_string(&serde_json::json!({
            "schema": schema,
            "conteos": conteos,
            "max_updated_at": max_updated,
            "hash_chunks": format!("{hash:016x}"),
        }))
        .map_err(|e| e.to_string())
    }
}

/// Semilla FNV-1a de 64 bits.
pub fn fnv1a_inicio() -> u64 {
    0xcbf2_9ce4_8422_2325
}

/// Actualiza un hash FNV-1a de 64 bits con un bloque de datos.
pub fn fnv1a_actualizar(mut hash: u64, datos: &[u8]) -> u64 {
    for b in datos {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Hash FNV-1a de 64 bits de un bloque completo.
pub fn fnv1a_64(datos: &[u8]) -> u64 {
    fnv1a_actualizar(fnv1a_inicio(), datos)
}

impl RepositorioSqlite {
    /// Nombre de la colección a la que pertenece un chunk (trazabilidad
    /// afirmación → evidencia → fuente).
    pub fn coleccion_de_chunk(&self, chunk_id: &str) -> Option<String> {
        let Ok(mut stmt) = self.conn.prepare(
            "SELECT c.name FROM rag_chunks rc \
             JOIN items i ON i.id = rc.item_id \
             JOIN collections c ON c.id = i.collection_id \
             WHERE rc.id = ?1",
        ) else {
            return None;
        };
        let Ok(mut rows) = stmt.query_map(params![chunk_id], |row| row.get::<_, String>(0)) else {
            return None;
        };
        rows.next().and_then(|r| r.ok())
    }

    /// Lista las colecciones reales (fuera de la denylist) con su cobertura:
    /// items, items con chunks y chunks.
    pub fn listar_colecciones(&self) -> Vec<ColeccionInfo> {
        self.listar_colecciones_con(true)
    }

    /// Mismo recuento que Colecciones en el desktop: sin ocultar pruebas.
    pub fn listar_todas_las_colecciones(&self) -> Vec<ColeccionInfo> {
        self.listar_colecciones_con(false)
    }

    fn listar_colecciones_con(&self, filtrar_denylist: bool) -> Vec<ColeccionInfo> {
        let (filtro, nombres) = if filtrar_denylist {
            let (clausula, nombres) = self.clausula_no_excluidas();
            (format!("WHERE {clausula}"), nombres)
        } else {
            (String::new(), Vec::new())
        };
        let sql = format!(
            "\
SELECT c.id, c.name, \
       COUNT(DISTINCT i.id) AS items, \
       COUNT(DISTINCT CASE WHEN rc.id IS NOT NULL THEN i.id END) AS items_con_chunks, \
       COUNT(rc.id) AS chunks \
FROM collections c \
LEFT JOIN items i ON i.collection_id = c.id \
LEFT JOIN rag_chunks rc ON rc.item_id = i.id \
{filtro} \
GROUP BY c.id, c.name \
ORDER BY items DESC"
        );
        let Ok(mut stmt) = self.conn.prepare(&sql) else {
            return Vec::new();
        };
        let parametros: Vec<&dyn ToSql> = nombres.iter().map(|n| n as &dyn ToSql).collect();
        let Ok(rows) = stmt.query_map(params_from_iter(parametros), |row| {
            Ok(ColeccionInfo {
                id: row.get(0)?,
                nombre: row.get(1)?,
                items: row.get(2)?,
                items_con_chunks: row.get(3)?,
                chunks: row.get(4)?,
            })
        }) else {
            return Vec::new();
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// Cobertura agregada del recorte consultado (PLAN §6.5): todo informe
    /// abre con esta tabla.
    pub fn cobertura(&self) -> Cobertura {
        let (clausula, nombres) = self.clausula_no_excluidas();
        // COUNT(DISTINCT): un item con varios chunks no debe duplicarse.
        let sql_total = format!(
            "SELECT COUNT(DISTINCT i.id) FROM items i JOIN collections c ON c.id = i.collection_id \
             WHERE {clausula}"
        );
        let sql_con_chunks = format!(
            "SELECT COUNT(DISTINCT i.id) FROM items i \
             JOIN collections c ON c.id = i.collection_id \
             JOIN rag_chunks rc ON rc.item_id = i.id \
             WHERE {clausula}"
        );
        let parametros: Vec<&dyn ToSql> = nombres.iter().map(|n| n as &dyn ToSql).collect();

        let total = self
            .conn
            .prepare(&sql_total)
            .and_then(|mut stmt| {
                stmt.query_row(params_from_iter(parametros.iter().copied()), |r| {
                    r.get::<_, i64>(0)
                })
            })
            .unwrap_or(0);
        let con_chunks = self
            .conn
            .prepare(&sql_con_chunks)
            .and_then(|mut stmt| {
                stmt.query_row(params_from_iter(parametros.iter().copied()), |r| {
                    r.get::<_, i64>(0)
                })
            })
            .unwrap_or(0);
        let colecciones = self.listar_colecciones();
        Cobertura {
            items_total: total,
            items_con_chunks: con_chunks,
            items_sin_procesar: total - con_chunks,
            colecciones,
        }
    }
}

/// Convierte un texto en una expresión FTS5 segura (tokens entre comillas,
/// unidos con OR para maximizar la cobertura).
///
/// Fase 0 (PLAN §9): se conservan los tokens numéricos de 2 caracteres (p. ej.
/// `65` y `17` en consultas por fecha `65-03-17`) que antes se descartaban.
///
/// Las palabras vacías se descartan antes del tope: si ocupan lugar, en una
/// pregunta larga los términos que discriminan quedan afuera.
pub fn fts5_query(texto: &str) -> String {
    let texto = texto.to_lowercase();
    let mut vistos = std::collections::HashSet::new();
    let tokens: Vec<String> = texto
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| es_token_valido(t) && !PALABRAS_VACIAS.contains(t) && vistos.insert(*t))
        .take(MAX_TOKENS_FTS)
        .map(|t| format!("\"{}\"", t.replace('"', "")))
        .collect();
    tokens.join(" OR ")
}

/// Tope de términos de una consulta FTS5.
const MAX_TOKENS_FTS: usize = 12;

/// Palabras vacías del español de más de dos letras (las de una o dos ya las
/// descarta `es_token_valido`), con y sin tilde, más los términos del propio
/// sistema que las preguntas arrastran («ítem», «colección»).
#[rustfmt::skip]
const PALABRAS_VACIAS: &[&str] = &[
    "qué", "que", "quién", "quien", "quiénes", "quienes", "cuál", "cual", "cuáles", "cuales",
    "cuándo", "cuando", "cuánto", "cuanto", "cuántos", "cuantos", "cuánta", "cuanta", "cuántas",
    "cuantas", "dónde", "donde", "cómo", "como", "los", "las", "del", "una", "uno", "unos", "unas",
    "para", "por", "con", "sin", "según", "segun", "sobre", "entre", "desde", "hasta", "hacia",
    "durante", "tras", "ante", "bajo", "contra", "este", "esta", "estos", "estas", "ese", "esa",
    "esos", "esas", "aquel", "aquella", "aquellos", "aquellas", "sus", "les", "fue", "fueron",
    "era", "eran", "ser", "son", "está", "están", "estan", "había", "habia", "hay",
    "tenía", "tenia", "tenían", "tenian", "más", "mas", "muy", "pero", "sino", "también",
    "tambien", "item", "ítem", "items", "ítems", "colección", "coleccion",
];

/// Un token es válido si tiene más de 2 caracteres, o si es numérico con al
/// menos 2 (las fechas `65-03-17` no deben perderse en la consulta).
fn es_token_valido(t: &str) -> bool {
    let n = t.chars().count();
    n > 2 || (n >= 2 && t.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts5_minusculea_y_une_con_or() {
        assert_eq!(fts5_query("huelga SOIP"), "\"huelga\" OR \"soip\"");
    }

    #[test]
    fn fts5_ignora_tokens_cortos_alfabeticos() {
        assert_eq!(fts5_query("la de y huelga"), "\"huelga\"");
    }

    #[test]
    fn fts5_descarta_las_palabras_vacias_de_la_pregunta() {
        // Interrogativos, artículos y conectores no discriminan documentos:
        // solo ocupan lugar en la consulta.
        assert_eq!(
            fts5_query("¿Qué reclamaban los obreros según una nota del diario?"),
            "\"reclamaban\" OR \"obreros\" OR \"nota\" OR \"diario\""
        );
    }

    #[test]
    fn fts5_no_repite_terminos() {
        // Un término repetido no suma cobertura: solo gastaría el tope.
        assert_eq!(
            fts5_query("65-03-03 huelga huelga"),
            "\"65\" OR \"03\" OR \"huelga\""
        );
    }

    #[test]
    fn fts5_conserva_tokens_numericos_de_fecha() {
        assert_eq!(fts5_query("65-03-17"), "\"65\" OR \"03\" OR \"17\"");
    }

    #[test]
    fn fts5_quita_comillas_internas() {
        assert_eq!(fts5_query("huelga \"citada\""), "\"huelga\" OR \"citada\"");
    }

    #[test]
    fn fts5_vacio_devuelve_vacio() {
        assert_eq!(fts5_query(""), "");
    }

    #[test]
    fn fts5_limita_a_doce_tokens() {
        let muchos: String = (1..=20)
            .map(|i| format!("palabra{}", i))
            .collect::<Vec<_>>()
            .join(" ");
        let q = fts5_query(&muchos);
        // 12 tokens producen 11 separadores " OR ".
        assert_eq!(q.matches(" OR ").count(), 11);
    }

    #[test]
    fn coleccion_info_calcula_sin_procesar() {
        let info = ColeccionInfo {
            id: "a".into(),
            nombre: "Conflicto SOIP 1965-66".into(),
            items: 148,
            items_con_chunks: 12,
            chunks: 40,
        };
        assert_eq!(info.items_sin_procesar(), 136);
    }
}

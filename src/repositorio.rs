//! Acceso a la base común de la app EntropIA (modo solo lectura).
//!
//! Expone la carga de chunks con embeddings, la búsqueda léxica FTS5 y la
//! lectura de fragmentos y colecciones para el agente.

use rusqlite::{params, Connection, OpenFlags};

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
    pub item_titulo: String,
    pub coleccion: String,
    pub text_content: String,
    pub embedding: Vec<f32>,
}

/// Repositorio SQLite sobre la base común de la app EntropIA.
///
/// Se abre en modo solo lectura para no interferir con la base activa de la
/// aplicación.
pub struct RepositorioSqlite {
    conn: Connection,
}

impl RepositorioSqlite {
    /// Abre la base en modo solo lectura.
    pub fn abrir(path: &str) -> Result<Self, String> {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY;
        let conn = Connection::open_with_flags(path, flags).map_err(|e| e.to_string())?;
        Ok(Self { conn })
    }

    /// Carga todos los chunks con su embedding y metadatos.
    pub fn cargar_chunks(&self) -> Result<Vec<ChunkRag>, String> {
        const SQL: &str = "\
SELECT rc.id, rc.text_content, rc.embedding, i.title, COALESCE(c.name, '') \
FROM rag_chunks rc \
LEFT JOIN items i ON i.id = rc.item_id \
LEFT JOIN collections c ON c.id = i.collection_id";
        let mut stmt = self.conn.prepare(SQL).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                let blob: Vec<u8> = row.get(2)?;
                Ok(ChunkRag {
                    id: row.get(0)?,
                    text_content: row.get(1)?,
                    embedding: crate::vector::decodificar_embedding(&blob),
                    item_titulo: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    coleccion: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                })
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            if let Ok(c) = r {
                out.push(c);
            }
        }
        Ok(out)
    }

    /// Busca identificadores de chunks por texto con FTS5 (BM25).
    pub fn buscar_fts5(&self, consulta: &str, limite: usize) -> Vec<String> {
        let q = fts5_query(consulta);
        if q.is_empty() {
            return Vec::new();
        }
        let sql = format!(
            "SELECT chunk_id FROM rag_chunks_fts WHERE text_content MATCH ?1 ORDER BY bm25(rag_chunks_fts) LIMIT {limite}"
        );
        let Ok(mut stmt) = self.conn.prepare(&sql) else {
            return Vec::new();
        };
        let rows = stmt.query_map(params![q], |row| row.get::<_, String>(0));
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

    /// Lista las colecciones con la cantidad de documentos (top 20).
    pub fn listar_colecciones(&self) -> Vec<(String, i64)> {
        let Ok(mut stmt) = self.conn.prepare(
            "SELECT c.name, COUNT(i.id) FROM collections c \
             LEFT JOIN items i ON i.collection_id = c.id \
             GROUP BY c.name ORDER BY 2 DESC LIMIT 20",
        ) else {
            return Vec::new();
        };
        let Ok(rows) = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))
        else {
            return Vec::new();
        };
        rows.filter_map(|r| r.ok()).collect()
    }
}

/// Convierte un texto en una expresión FTS5 segura (tokens entre comillas,
/// unidos con OR para maximizar la cobertura).
pub fn fts5_query(texto: &str) -> String {
    let tokens: Vec<String> = texto
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() > 2)
        .take(12)
        .map(|t| format!("\"{}\"", t.replace('"', "")))
        .collect();
    tokens.join(" OR ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts5_minusculea_y_une_con_or() {
        assert_eq!(fts5_query("huelga SOIP"), "\"huelga\" OR \"soip\"");
    }

    #[test]
    fn fts5_ignora_tokens_cortos() {
        assert_eq!(fts5_query("la de y huelga"), "\"huelga\"");
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
}

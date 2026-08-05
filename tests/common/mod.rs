//! Helpers de integración: construye una copia del esquema del corpus en una
//! base temporal para tests herméticos (PLAN §9: «tests/ — integración con una
//! copia de la base»).
//!
//! El esquema replica el de `entropia.sqlite` (colecciones, items, rag_chunks,
//! rag_chunks_fts) para que las consultas SQL del código publicado corran
//! contra datos realistas y deterministas.

use rusqlite::Connection;
use std::sync::atomic::{AtomicUsize, Ordering};

static CONTADOR: AtomicUsize = AtomicUsize::new(0);

/// Crea una base sintética con el esquema del corpus y datos de prueba.
/// Devuelve la ruta del archivo temporal. Cada llamada usa un archivo único
/// (los tests corren en paralelo).
pub fn crear_corpus_sintetico() -> std::path::PathBuf {
    let n = CONTADOR.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!(
        "entropia-corpus-test-{}-{n}.sqlite",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "\
CREATE TABLE collections (
  id          TEXT    PRIMARY KEY,
  name        TEXT    NOT NULL,
  description TEXT,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL
);
CREATE TABLE items (
  id            TEXT    PRIMARY KEY,
  title         TEXT    NOT NULL,
  collection_id TEXT    NOT NULL REFERENCES collections(id),
  metadata      TEXT,
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL
);
CREATE TABLE rag_chunks (
  id TEXT PRIMARY KEY,
  asset_id TEXT NOT NULL,
  item_id TEXT NOT NULL REFERENCES items(id),
  source_kind TEXT NOT NULL,
  source_id TEXT NOT NULL,
  chunk_ordinal INTEGER NOT NULL,
  text_content TEXT NOT NULL,
  start_char INTEGER NOT NULL,
  end_char INTEGER NOT NULL,
  source_text_hash TEXT NOT NULL,
  chunking_contract TEXT NOT NULL,
  embedding BLOB NOT NULL,
  embedding_model TEXT NOT NULL,
  embedding_contract TEXT NOT NULL,
  dimensions INTEGER NOT NULL
);
CREATE VIRTUAL TABLE rag_chunks_fts USING fts5(
  chunk_id UNINDEXED,
  text_content,
  tokenize = 'unicode61 remove_diacritics 1'
);
",
    )
    .unwrap();

    // Colecciones: 3 reales + las 6 de la denylist.
    let ahora = 1_700_000_000i64;
    let colecciones = [
        ("c-conflicto", "Conflicto SOIP 1965-66"),
        ("c-volantes", "Volantes, Panfletos, etc."),
        ("c-soip1961", "SOIP 1961"),
        ("c-stress222", "stress222"),
        ("c-stress01", "stress-test-01"),
        ("c-validacion", "validacioneventos2222"),
        ("c-nuevo", "nuevo proyecto"),
        ("c-probar2", "probar2"),
        ("c-prueba3", "prueba3"),
    ];
    for (id, nombre) in colecciones {
        conn.execute(
            "INSERT INTO collections (id, name, description, created_at, updated_at) \
             VALUES (?1, ?2, '', ?3, ?3)",
            rusqlite::params![id, nombre, ahora],
        )
        .unwrap();
    }

    // Items: 3 reales (2 con chunks, 1 sin) + 2 de prueba (con chunks).
    let items = [
        ("item-1", "65-03-17-a", "c-conflicto", true),
        ("item-2", "65-03-20-b", "c-conflicto", true),
        ("item-3", "sin fecha", "c-conflicto", false),
        ("item-stress-1", "stresstest-1", "c-stress222", true),
        ("item-stress-2", "stresstest-2", "c-stress01", true),
    ];
    for (id, titulo, col, _) in items {
        conn.execute(
            "INSERT INTO items (id, title, collection_id, metadata, created_at, updated_at) \
             VALUES (?1, ?2, ?3, '{}', ?4, ?4)",
            rusqlite::params![id, titulo, col, ahora],
        )
        .unwrap();
    }

    // Embedding f32 LE de [1.0, 0.0].
    let emb: Vec<u8> = vec![0x00, 0x00, 0x80, 0x3F, 0x00, 0x00, 0x00, 0x00];

    // Chunks: 2 en colección real, 2 en colecciones de prueba.
    let chunks = [
        (
            "chunk-1",
            "item-1",
            "huelga general de la pesca en marzo",
            0,
        ),
        (
            "chunk-2",
            "item-2",
            "la asamblea resolvió continuar el paro",
            0,
        ),
        (
            "chunk-stress-1",
            "item-stress-1",
            "huelga simulada de stress",
            0,
        ),
        ("chunk-stress-2", "item-stress-2", "paro de prueba", 0),
    ];
    for (id, item, texto, ord) in chunks {
        conn.execute(
            "INSERT INTO rag_chunks (id, asset_id, item_id, source_kind, source_id, \
             chunk_ordinal, text_content, start_char, end_char, source_text_hash, \
             chunking_contract, embedding, embedding_model, embedding_contract, dimensions) \
             VALUES (?1, ?1, ?2, 'transcription', 'src', ?3, ?4, 0, 100, 'hash', \
                     'test', ?5, 'bge-m3', 'test', 1024)",
            rusqlite::params![id, item, ord, texto, emb],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO rag_chunks_fts (chunk_id, text_content) VALUES (?1, ?2)",
            rusqlite::params![id, texto],
        )
        .unwrap();
    }

    path
}

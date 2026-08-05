//! Helpers de tests compartidos dentro de la lib (solo se compilan con
//! `cargo test`). Los tests de integración tienen su propia copia en
//! `tests/common/`.

use rusqlite::Connection;

/// Crea una base sintética con el esquema del corpus y datos de prueba
/// (idéntica a `tests/common/mod.rs`, que usan los tests de integración).
pub fn corpus_sintetico() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CONTADOR: AtomicUsize = AtomicUsize::new(0);
    let n = CONTADOR.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!(
        "entropia-corpus-unit-{}-{n}.sqlite",
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
CREATE TABLE entities (
  id TEXT PRIMARY KEY NOT NULL,
  item_id TEXT NOT NULL,
  entity_type TEXT NOT NULL,
  value TEXT NOT NULL,
  start_offset INTEGER NOT NULL DEFAULT 0,
  end_offset INTEGER NOT NULL DEFAULT 0,
  confidence REAL NOT NULL DEFAULT 1.0,
  source TEXT,
  model_name TEXT,
  created_at INTEGER NOT NULL DEFAULT 0,
  latitude REAL, longitude REAL,
  geo_status TEXT NOT NULL DEFAULT 'pending',
  asset_id TEXT,
  manual_lat REAL, manual_lon REAL
);
CREATE TABLE triples (
  id TEXT PRIMARY KEY NOT NULL,
  item_id TEXT NOT NULL,
  subject TEXT NOT NULL,
  predicate TEXT NOT NULL,
  object TEXT NOT NULL,
  created_at INTEGER NOT NULL DEFAULT 0,
  asset_id TEXT
);
CREATE TABLE assets (
  id TEXT PRIMARY KEY,
  item_id TEXT NOT NULL,
  path TEXT NOT NULL,
  type TEXT NOT NULL,
  size INTEGER,
  created_at INTEGER NOT NULL,
  sort_index INTEGER NOT NULL DEFAULT 0,
  parent_asset_id TEXT,
  page_number INTEGER
);
CREATE TABLE _migrations (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL UNIQUE,
  applied_at INTEGER NOT NULL
);
",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO _migrations (name, applied_at) VALUES ('0001_base', 1)",
        [],
    )
    .unwrap();

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

    let emb: Vec<u8> = vec![0x00, 0x00, 0x80, 0x3F, 0x00, 0x00, 0x00, 0x00];
    let chunks = [
        (
            "chunk-1",
            "item-1",
            "huelga general de la pesca en marzo",
            0,
            "asset-1",
        ),
        (
            "chunk-2",
            "item-2",
            "la asamblea resolvió continuar el paro",
            0,
            "asset-2",
        ),
        (
            "chunk-3",
            "item-1",
            "segundo fragmento del mismo item",
            1,
            "asset-1",
        ),
        (
            "chunk-stress-1",
            "item-stress-1",
            "huelga simulada de stress",
            0,
            "asset-s1",
        ),
        (
            "chunk-stress-2",
            "item-stress-2",
            "paro de prueba",
            0,
            "asset-s2",
        ),
    ];
    for (id, item, texto, ord, asset) in chunks {
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
        let _ = asset;
    }

    // Entidades y triples de prueba.
    conn.execute(
        "INSERT INTO entities (id, item_id, entity_type, value, confidence) \
         VALUES ('ent-1', 'item-1', 'organization', 'Sindicato Obrero de la Industria del Pescado', 0.95)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO triples (id, item_id, subject, predicate, object) \
         VALUES ('trip-1', 'item-1', 'el Sindicato Obrero de la Industria del Pescado', 'denuncia', 'atropellos')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO assets (id, item_id, path, type, created_at, sort_index, page_number) \
         VALUES ('asset-1', 'item-1', 'escaneos/65-03-17-a.pdf', 'pdf', 1, 0, 3)",
        [],
    )
    .unwrap();

    path
}

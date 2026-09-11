//! Helpers de integración: construye una copia del esquema del corpus en una
//! base temporal para tests herméticos (PLAN §9: «tests/ — integración con una
//! copia de la base»).
//!
//! El esquema replica el de `entropia.sqlite` (colecciones, items, rag_chunks,
//! rag_chunks_fts) para que las consultas SQL del código publicado corran
//! contra datos realistas y deterministas.

// Cada binario de test compila este módulo por separado y usa solo una
// parte de los helpers: lo que otro test necesita no es código muerto.
#![allow(dead_code)]

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

    // Entidades, triples y assets de prueba (traversal del gateway, §6.5).
    conn.execute(
        "INSERT INTO entities (id, item_id, entity_type, value, confidence) \
         VALUES ('ent-1', 'item-1', 'organization', \
                 'Sindicato Obrero de la Industria del Pescado', 0.95)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO entities (id, item_id, entity_type, value, confidence) \
         VALUES ('ent-2', 'item-2', 'place', 'Mar del Plata', 0.9)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO triples (id, item_id, subject, predicate, object) \
         VALUES ('trip-1', 'item-1', 'el Sindicato Obrero de la Industria del Pescado', \
                 'denuncia', 'atropellos')",
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

/// Agrega a una colección del corpus sintético `n` ítems con un chunk cada
/// uno, todos con el mismo texto. Sirve para tener más coincidencias que el
/// techo de recuperación.
pub fn agregar_chunks(path: &std::path::Path, coleccion: &str, n: usize, texto: &str) {
    let conn = Connection::open(path).unwrap();
    let emb: Vec<u8> = vec![0x00, 0x00, 0x80, 0x3F, 0x00, 0x00, 0x00, 0x00];
    for i in 0..n {
        let item = format!("item-{coleccion}-{i}");
        let chunk = format!("chunk-{coleccion}-{i}");
        conn.execute(
            "INSERT INTO items (id, title, collection_id, metadata, created_at, updated_at) \
             VALUES (?1, ?2, ?3, '{}', 1700000000, 1700000000)",
            rusqlite::params![item, format!("documento {i}"), coleccion],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO rag_chunks (id, asset_id, item_id, source_kind, source_id, \
             chunk_ordinal, text_content, start_char, end_char, source_text_hash, \
             chunking_contract, embedding, embedding_model, embedding_contract, dimensions) \
             VALUES (?1, ?1, ?2, 'transcription', 'src', 0, ?3, 0, 100, 'hash', \
                     'test', ?4, 'bge-m3', 'test', 1024)",
            rusqlite::params![chunk, item, texto, emb],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO rag_chunks_fts (chunk_id, text_content) VALUES (?1, ?2)",
            rusqlite::params![chunk, texto],
        )
        .unwrap();
    }
}

/// Doble de LLM que responde los contratos JSON de cada rol del workflow.
///
/// Existe para ejercitar el motor completo sin red y sin depender de la
/// conducta de ningún proveedor: lo que se prueba con él es la maquinaria
/// —recuperación, lotes, span check, citas, cobertura—, no el juicio del
/// modelo.
pub struct LlmGuionado {
    pub consultas: Vec<String>,
}

impl entropia_agent::cliente_llm::ClienteLlm for LlmGuionado {
    fn modelo(&self) -> &str {
        "fake/guionado"
    }

    fn ultimo_costo(&self) -> Option<f64> {
        Some(0.001)
    }

    fn turno_agente(
        &self,
        mensajes: &[serde_json::Value],
        _: &[serde_json::Value],
    ) -> Result<entropia_agent::cliente_llm::TurnoAgente, String> {
        use serde_json::json;
        let sistema = mensajes[0]["content"].as_str().unwrap_or_default();
        let usuario = mensajes[1]["content"].as_str().unwrap_or_default();
        // El verificador manda prosa, no JSON.
        let datos: serde_json::Value =
            serde_json::from_str(usuario).unwrap_or(serde_json::Value::Null);

        let salida = if sistema.contains("Rol: prospeccion.") {
            json!({"sufficient":true,"rationale":"El recorte tiene material procesado","gaps":[]})
        } else if sistema.contains("{hypothesis:") {
            json!({"hypothesis":"Hubo conflictividad gremial","scope":"recorte seleccionado","closing_criteria":["Agotar la evidencia recuperada"]})
        } else if sistema.contains("{questions:") {
            json!({"questions":(1..=4).map(|i| json!({
                "id": format!("q{i}"), "axis": "Período",
                "text": format!("Pregunta {i}"), "rationale": "Cambia el plan"
            })).collect::<Vec<_>>()})
        } else if sistema.contains("{queries:") {
            json!({"queries":self.consultas,"bibliography_queries":[],"retrieval_limit":16})
        } else if sistema.contains("Rol: asistente_archivo.") {
            // Un claim por ítem de evidencia, citando un pasaje literal.
            let claims: Vec<serde_json::Value> = datos["evidence"]
                .as_array()
                .map(|e| e.as_slice())
                .unwrap_or_default()
                .iter()
                .take(3)
                .enumerate()
                .map(|(i, e)| {
                    let texto = e["text"].as_str().unwrap_or_default();
                    // Pasaje literal: una ventana en frontera de carácter.
                    let pasaje: String = texto.chars().skip(10).take(40).collect();
                    json!({
                        "id": format!("c{}", i + 1),
                        "text": format!("El documento {} registra actividad gremial", i + 1),
                        "evidence_ids": [e["id"].clone()],
                        "quotes": [{"evidence_id": e["id"].clone(), "quote": pasaje}],
                        "interpretative": false
                    })
                })
                .collect();
            json!({"summary":"Síntesis del lote","claims":claims,"limitations":[]})
        } else if sistema.contains("Rol: asistente_bibliografia.") {
            json!({"references":[],"synthesis":"Sin consultas bibliográficas"})
        } else if sistema.contains("Sos el Verificador de EntropIA.") {
            json!({"estado":"supported","rationale":"El pasaje sostiene la afirmación","error_kind":null})
        } else {
            let ids: Vec<serde_json::Value> = datos["claims"]
                .as_array()
                .map(|c| c.iter().map(|c| c["id"].clone()).collect())
                .unwrap_or_default();
            json!({"title":"Informe","sections":[{"title":"Hechos","text":"Síntesis de la evidencia verificada.","claim_ids":ids}]})
        };
        Ok(entropia_agent::cliente_llm::TurnoAgente::Texto(
            salida.to_string(),
        ))
    }
}
